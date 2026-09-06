use std::collections::{BTreeMap, BTreeSet};

use apc_core::{ContinuumId, CoreError, LocalScalarDomain, RevisionId, ScalarRevision};
use apc_crypto::ContentKey;
use apc_sync::{
    commit_received, decode_protected_sync_part, fetch_from_durable_cursor, DurableSyncRecord,
    FetchOutcome, MultipartInbox, OpaqueTransport, ProtectedPartCodecError, ScalarSyncProjection,
    SessionCommitError, SessionIoError, SyncPartError, SyncRecordStore, TransportCursorCodec,
};

use crate::{LocalScalarRecoveryState, LocalScalarRecoveryStateError, TrustedStateCodec};

/// Immutable parameters for one complete multi-domain recovery catch-up pass.
///
/// Pre-observation revision identities are keyed by semantic merge domain. They
/// are consumed only when authenticated remote state actually touches that domain
/// and the local domain has a pending working epoch. There is deliberately no
/// global pre-observation identity: remote state in one domain must never seal or
/// create causality in another domain.
pub struct ScalarRecoveryCatchUpSpec<'a> {
    key: &'a ContentKey,
    continuum_id: ContinuumId,
    pre_observation_revision_ids: &'a BTreeMap<apc_sync::DomainKey, RevisionId>,
}

impl<'a> ScalarRecoveryCatchUpSpec<'a> {
    pub fn new(
        key: &'a ContentKey,
        continuum_id: ContinuumId,
        pre_observation_revision_ids: &'a BTreeMap<apc_sync::DomainKey, RevisionId>,
    ) -> Self {
        Self {
            key,
            continuum_id,
            pre_observation_revision_ids,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScalarRecoveryCatchUpOutcome<R> {
    UpToDate {
        head: Option<R>,
    },
    Applied {
        head: R,
        /// Number of protected transport wire objects consumed by this pass.
        object_count: usize,
        /// Authenticated remote revision identities grouped by their semantic
        /// merge domain. This is observation evidence, not an ordering source.
        observed_revision_ids: BTreeMap<apc_sync::DomainKey, BTreeSet<RevisionId>>,
        /// Local working epochs sealed because the same domain became
        /// semantically observable during this pass.
        sealed_local: BTreeMap<apc_sync::DomainKey, ScalarRevision<Vec<u8>>>,
    },
    CursorAdvancedWithoutSemanticObjects {
        head: R,
    },
    BaselineUnavailable {
        head: Option<R>,
    },
}

#[derive(Debug)]
pub enum ScalarRecoveryCatchUpError<TransportError, TrustedError, StoreError, CursorError> {
    Io(SessionIoError<TransportError, CursorError>),
    Wire(ProtectedPartCodecError),
    Protection(SyncPartError),
    IncompleteMultipartPublications { count: usize },
    Core(CoreError),
    RecoveryState(LocalScalarRecoveryStateError),
    TrustedState(TrustedError),
    Commit(SessionCommitError<StoreError, CursorError>),
}

pub type ScalarRecoveryCatchUpResult<R, TransportError, TrustedError, StoreError, CursorError> =
    Result<
        ScalarRecoveryCatchUpOutcome<R>,
        ScalarRecoveryCatchUpError<TransportError, TrustedError, StoreError, CursorError>,
    >;

/// Execute one durable-cursor catch-up pass for a complete local scalar recovery
/// container.
///
/// The fetched transport range is first decoded, authenticated and fully
/// multipart-assembled without exposing any semantic state. If even one declared
/// publication remains incomplete, the whole pass fails before semantic mutation
/// and before cursor advancement, allowing the same durable range to be fetched
/// again safely.
///
/// Once the range is complete, all projections are merged into one state-based
/// projection. Each touched merge domain then crosses its own semantic observation
/// boundary on a cloned recovery container. Dirty work is sealed only in that
/// exact domain, using the revision identity supplied for that domain. Untouched
/// domains remain byte/logically unchanged.
///
/// Finally the complete recovery container and the new transport cursor are
/// encoded and persisted through one `commit_received()` durability boundary.
/// Physical crash atomicity of that wrapper does not create cross-domain semantic
/// atomicity or causality.
#[allow(clippy::type_complexity)]
pub fn catch_up_scalar_recovery_state<T, S, TC, CC>(
    recovery: &mut LocalScalarRecoveryState,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    transport: &mut T,
    spec: ScalarRecoveryCatchUpSpec<'_>,
) -> ScalarRecoveryCatchUpResult<T::Revision, T::Error, TC::Error, S::Error, CC::Error>
where
    T: OpaqueTransport,
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarRecoveryState>,
    CC: TransportCursorCodec<T::Revision>,
{
    let fetched = fetch_from_durable_cursor(record, transport, cursor_codec)
        .map_err(ScalarRecoveryCatchUpError::Io)?;

    match fetched {
        FetchOutcome::UpToDate { head } => Ok(ScalarRecoveryCatchUpOutcome::UpToDate { head }),
        FetchOutcome::BaselineUnavailable { head } => {
            Ok(ScalarRecoveryCatchUpOutcome::BaselineUnavailable { head })
        }
        FetchOutcome::Changed { head, objects } => {
            if objects.is_empty() {
                let trusted_state = trusted_codec
                    .encode(recovery)
                    .map_err(ScalarRecoveryCatchUpError::TrustedState)?;
                commit_received(record, store, cursor_codec, trusted_state, &head)
                    .map_err(ScalarRecoveryCatchUpError::Commit)?;
                return Ok(
                    ScalarRecoveryCatchUpOutcome::CursorAdvancedWithoutSemanticObjects { head },
                );
            }

            let object_count = objects.len();
            let projections =
                decode_complete_scalar_projections(spec.key, spec.continuum_id, &objects)?;

            let mut combined = ScalarSyncProjection::new();
            for projection in projections {
                combined = combined
                    .merge(&projection)
                    .map_err(ScalarRecoveryCatchUpError::Core)?;
            }

            let mut candidate = recovery.clone();
            let mut observed_revision_ids = BTreeMap::new();
            let mut sealed_local = BTreeMap::new();

            for (domain_key, remote) in combined.domains() {
                observed_revision_ids.insert(
                    domain_key.clone(),
                    remote.revisions().map(|revision| revision.id).collect(),
                );

                match candidate
                    .restore_domain(domain_key)
                    .map_err(ScalarRecoveryCatchUpError::Core)?
                {
                    Some(mut local) => {
                        let pre_observation_revision_id =
                            spec.pre_observation_revision_ids.get(domain_key).copied();
                        if let Some(sealed) = local
                            .observe_remote(remote, pre_observation_revision_id)
                            .map_err(ScalarRecoveryCatchUpError::Core)?
                        {
                            sealed_local.insert(domain_key.clone(), sealed);
                        }
                        candidate
                            .replace_domain(domain_key.clone(), local.snapshot())
                            .map_err(ScalarRecoveryCatchUpError::RecoveryState)?;
                    }
                    None => {
                        let local = LocalScalarDomain::from_causal(remote.clone())
                            .map_err(ScalarRecoveryCatchUpError::Core)?;
                        candidate
                            .insert_new(domain_key.clone(), local.snapshot())
                            .map_err(ScalarRecoveryCatchUpError::RecoveryState)?;
                    }
                }
            }

            let trusted_state = trusted_codec
                .encode(&candidate)
                .map_err(ScalarRecoveryCatchUpError::TrustedState)?;
            commit_received(record, store, cursor_codec, trusted_state, &head)
                .map_err(ScalarRecoveryCatchUpError::Commit)?;
            *recovery = candidate;

            Ok(ScalarRecoveryCatchUpOutcome::Applied {
                head,
                object_count,
                observed_revision_ids,
                sealed_local,
            })
        }
    }
}

fn decode_complete_scalar_projections<TransportError, TrustedError, StoreError, CursorError>(
    key: &ContentKey,
    continuum_id: ContinuumId,
    encoded_objects: &[Vec<u8>],
) -> Result<
    Vec<ScalarSyncProjection>,
    ScalarRecoveryCatchUpError<TransportError, TrustedError, StoreError, CursorError>,
> {
    let mut inbox = MultipartInbox::new();
    let mut completed = Vec::new();
    let mut seen_wire_objects: BTreeSet<&[u8]> = BTreeSet::new();

    for encoded in encoded_objects {
        if !seen_wire_objects.insert(encoded.as_slice()) {
            continue;
        }

        let part = decode_protected_sync_part(encoded).map_err(ScalarRecoveryCatchUpError::Wire)?;
        if let Some(projection) = inbox
            .ingest(key, continuum_id, part)
            .map_err(ScalarRecoveryCatchUpError::Protection)?
        {
            completed.push(projection);
        }
    }

    let incomplete = inbox.pending_publications();
    if incomplete != 0 {
        return Err(
            ScalarRecoveryCatchUpError::IncompleteMultipartPublications { count: incomplete },
        );
    }

    Ok(completed)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, ScalarRegister, WorkingEpochId};
    use apc_sync::{
        encode_protected_sync_part, protect_scalar_part, DomainKey, PublicationId, PublishOutcome,
        SyncProjection, SyncRecordStore, TransportCursor,
    };

    use crate::DevelopmentMultiScalarTrustedStateCodec;

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Revision(u64);

    struct RevisionCodec;

    impl TransportCursorCodec<Revision> for RevisionCodec {
        type Error = &'static str;

        fn encode(&self, revision: &Revision) -> Result<TransportCursor, Self::Error> {
            TransportCursor::new(revision.0.to_be_bytes().to_vec()).map_err(|_| "encode")
        }

        fn decode(&self, cursor: &TransportCursor) -> Result<Revision, Self::Error> {
            let bytes: [u8; 8] = cursor.as_bytes().try_into().map_err(|_| "decode")?;
            Ok(Revision(u64::from_be_bytes(bytes)))
        }
    }

    #[derive(Default)]
    struct MemoryStore {
        committed: Option<DurableSyncRecord>,
    }

    impl SyncRecordStore for MemoryStore {
        type Error = &'static str;

        fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
            self.committed = Some(record.clone());
            Ok(())
        }
    }

    struct RangeTransport {
        head: Revision,
        objects: Vec<Vec<u8>>,
    }

    impl OpaqueTransport for RangeTransport {
        type Revision = Revision;
        type Error = &'static str;

        fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
            Ok(Some(self.head))
        }

        fn fetch_since(
            &mut self,
            known_head: Option<&Self::Revision>,
        ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
            if known_head == Some(&self.head) {
                Ok(FetchOutcome::UpToDate {
                    head: Some(self.head),
                })
            } else {
                Ok(FetchOutcome::Changed {
                    head: self.head,
                    objects: self.objects.clone(),
                })
            }
        }

        fn publish(
            &mut self,
            _expected_head: Option<&Self::Revision>,
            _objects: &[Vec<u8>],
        ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
            Err("publish unused")
        }
    }

    fn bytes(value: u64) -> [u8; LOGICAL_ID_BYTES] {
        let mut bytes = [0_u8; LOGICAL_ID_BYTES];
        bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn atom(value: u64) -> AtomId {
        AtomId::from_bytes(bytes(value))
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(bytes(value))
    }

    fn wid(value: u64) -> WorkingEpochId {
        WorkingEpochId::from_bytes(bytes(value))
    }

    fn pid(value: u64) -> PublicationId {
        PublicationId::from_bytes(bytes(value))
    }

    fn cid(value: u64) -> ContinuumId {
        ContinuumId::from_bytes(bytes(value))
    }

    fn domain(name: &str) -> DomainKey {
        DomainKey::new(atom(1), name.as_bytes()).unwrap()
    }

    fn dirty_domain(
        base_revision: u64,
        epoch: u64,
        base: &str,
        draft: &str,
    ) -> LocalScalarDomain<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal
            .assign(rid(base_revision), base.as_bytes().to_vec())
            .unwrap();
        let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
        domain
            .begin_epoch(wid(epoch), draft.as_bytes().to_vec())
            .unwrap();
        domain
    }

    fn remote_from_base(
        base_revision: u64,
        remote_revision: u64,
        base: &str,
        value: &str,
    ) -> ScalarRegister<Vec<u8>> {
        let mut remote = ScalarRegister::new();
        remote
            .assign(rid(base_revision), base.as_bytes().to_vec())
            .unwrap();
        remote
            .assign(rid(remote_revision), value.as_bytes().to_vec())
            .unwrap();
        remote
    }

    #[test]
    fn one_range_observes_each_touched_domain_independently_and_commits_one_cursor() {
        let body_key = domain("body");
        let title_key = domain("title");
        let notes_key = domain("notes");

        let body = dirty_domain(100, 1, "body-base", "body-draft");
        let title = dirty_domain(300, 2, "title-base", "title-draft");
        let notes = dirty_domain(500, 3, "notes-base", "notes-draft");
        let original_notes = notes.snapshot();

        let mut recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (body_key.clone(), body.snapshot()),
            (title_key.clone(), title.snapshot()),
            (notes_key.clone(), original_notes.clone()),
        ]))
        .unwrap();

        let remote_body = remote_from_base(100, 900, "body-base", "remote-body");
        let remote_title = remote_from_base(300, 901, "title-base", "remote-title");
        let projection = SyncProjection::from_domains(BTreeMap::from([
            (body_key.clone(), remote_body),
            (title_key.clone(), remote_title),
        ]));

        let key = ContentKey::from_bytes([0xC1; 32]);
        let protected = protect_scalar_part(&key, cid(1), pid(1), 0, 1, &projection).unwrap();
        let object = encode_protected_sync_part(&protected).unwrap();
        let mut transport = RangeTransport {
            head: Revision(2),
            objects: vec![object],
        };

        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        let pre_observation =
            BTreeMap::from([(body_key.clone(), rid(200)), (title_key.clone(), rid(400))]);

        let outcome = catch_up_scalar_recovery_state(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            &mut transport,
            ScalarRecoveryCatchUpSpec::new(&key, cid(1), &pre_observation),
        )
        .unwrap();

        let ScalarRecoveryCatchUpOutcome::Applied {
            head,
            object_count,
            observed_revision_ids,
            sealed_local,
        } = outcome
        else {
            panic!("expected applied catch-up")
        };

        assert_eq!(head, Revision(2));
        assert_eq!(object_count, 1);
        assert_eq!(sealed_local.get(&body_key).unwrap().id, rid(200));
        assert_eq!(sealed_local.get(&title_key).unwrap().id, rid(400));
        assert_eq!(
            observed_revision_ids.get(&body_key).unwrap(),
            &BTreeSet::from([rid(100), rid(900)])
        );
        assert_eq!(
            observed_revision_ids.get(&title_key).unwrap(),
            &BTreeSet::from([rid(300), rid(901)])
        );

        let recovered_body = recovery.restore_domain(&body_key).unwrap().unwrap();
        assert_eq!(
            recovered_body.causal().frontier_ids(),
            BTreeSet::from([rid(200), rid(900)])
        );
        assert!(recovered_body.causal().revision(rid(400)).is_none());
        assert!(recovered_body.causal().revision(rid(901)).is_none());

        let recovered_title = recovery.restore_domain(&title_key).unwrap().unwrap();
        assert_eq!(
            recovered_title.causal().frontier_ids(),
            BTreeSet::from([rid(400), rid(901)])
        );
        assert!(recovered_title.causal().revision(rid(200)).is_none());
        assert!(recovered_title.causal().revision(rid(900)).is_none());

        assert_eq!(recovery.get(&notes_key), Some(&original_notes));
        assert_eq!(
            recovery
                .restore_domain(&notes_key)
                .unwrap()
                .unwrap()
                .pending()
                .unwrap()
                .id,
            wid(3)
        );
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
        assert_eq!(store.committed.as_ref(), Some(&record));
    }

    #[test]
    fn incomplete_multipart_blocks_every_domain_and_cursor_advance() {
        let body_key = domain("body");
        let title_key = domain("title");
        let body = dirty_domain(100, 1, "body-base", "body-draft");
        let title = dirty_domain(300, 2, "title-base", "title-draft");
        let mut recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (body_key.clone(), body.snapshot()),
            (title_key.clone(), title.snapshot()),
        ]))
        .unwrap();
        let before_recovery = recovery.clone();

        let key = ContentKey::from_bytes([0xC2; 32]);
        let first_projection = SyncProjection::from_domains(BTreeMap::from([(
            body_key.clone(),
            remote_from_base(100, 900, "body-base", "remote-body"),
        )]));
        let first_part =
            protect_scalar_part(&key, cid(1), pid(7), 0, 2, &first_projection).unwrap();
        let object = encode_protected_sync_part(&first_part).unwrap();
        let mut transport = RangeTransport {
            head: Revision(2),
            objects: vec![object],
        };

        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let before_record = record.clone();
        let mut store = MemoryStore::default();
        let pre_observation = BTreeMap::from([(body_key, rid(200)), (title_key, rid(400))]);

        let error = catch_up_scalar_recovery_state(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            &mut transport,
            ScalarRecoveryCatchUpSpec::new(&key, cid(1), &pre_observation),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ScalarRecoveryCatchUpError::IncompleteMultipartPublications { count: 1 }
        ));
        assert_eq!(recovery, before_recovery);
        assert_eq!(record, before_record);
        assert!(store.committed.is_none());
    }

    #[test]
    fn previously_unknown_remote_domain_is_added_without_local_ownership() {
        let known_key = domain("body");
        let remote_key = domain("title");
        let known = dirty_domain(100, 1, "body-base", "body-draft");
        let original_known = known.snapshot();
        let mut recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([(
            known_key.clone(),
            original_known.clone(),
        )]))
        .unwrap();

        let projection = SyncProjection::from_domains(BTreeMap::from([(
            remote_key.clone(),
            remote_from_base(300, 900, "title-base", "remote-title"),
        )]));
        let key = ContentKey::from_bytes([0xC3; 32]);
        let protected = protect_scalar_part(&key, cid(1), pid(8), 0, 1, &projection).unwrap();
        let mut transport = RangeTransport {
            head: Revision(2),
            objects: vec![encode_protected_sync_part(&protected).unwrap()],
        };

        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        let pre_observation = BTreeMap::new();

        catch_up_scalar_recovery_state(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            &mut transport,
            ScalarRecoveryCatchUpSpec::new(&key, cid(1), &pre_observation),
        )
        .unwrap();

        assert_eq!(recovery.get(&known_key), Some(&original_known));
        let received = recovery.restore_domain(&remote_key).unwrap().unwrap();
        assert!(received.pending().is_none());
        assert!(received.finalization().local_revision_ids().is_empty());
        assert_eq!(received.causal().frontier_ids(), BTreeSet::from([rid(900)]));
    }
}
