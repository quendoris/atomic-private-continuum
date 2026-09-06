use std::collections::{BTreeMap, BTreeSet};

use apc_core::{ContinuumId, RevisionId};
use apc_crypto::ContentKey;
use apc_sync::{
    commit_reconciled_outbox_batch, decode_protected_sync_part, publish_staged_batch,
    BatchCommitError, BatchPublishError, DurableSyncRecord, MultipartInbox, OpaqueTransport,
    ProtectedPartCodecError, PublicationId, PublishOutcome, ScalarSyncProjection, SyncPartError,
    SyncRecordStore, TransportCursorCodec,
};

use crate::{
    catch_up_scalar_recovery_state, LocalScalarRecoveryState, ScalarRecoveryCatchUpError,
    ScalarRecoveryCatchUpOutcome, ScalarRecoveryCatchUpSpec, TrustedStateCodec,
};

/// Immutable inputs for one foreground recovery/outbox resume pass.
///
/// Pre-observation revision identities remain keyed by semantic merge domain.
/// Neither publication IDs nor transport cursor bytes are used as clocks or retry
/// priorities.
pub struct ScalarRecoveryResumeSpec<'a> {
    key: &'a ContentKey,
    continuum_id: ContinuumId,
    pre_observation_revision_ids: &'a BTreeMap<apc_sync::DomainKey, RevisionId>,
}

impl<'a> ScalarRecoveryResumeSpec<'a> {
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
pub enum ScalarRecoveryResumeAction<R> {
    None,
    BlockedByBaseline,
    NeedsRebase {
        publication_ids: BTreeSet<PublicationId>,
    },
    PublishedAndReconciled {
        publication_ids: BTreeSet<PublicationId>,
        head: R,
    },
    Conflict {
        publication_ids: BTreeSet<PublicationId>,
        current_head: Option<R>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarRecoveryResumeReport<R> {
    pub catch_up: ScalarRecoveryCatchUpOutcome<R>,
    /// Stale publications proven to have arrived by authenticated per-domain
    /// revision evidence in this exact catch-up pass.
    pub observed_reconciled: BTreeSet<PublicationId>,
    pub action: ScalarRecoveryResumeAction<R>,
}

#[derive(Debug)]
pub enum PendingRecoveryPublicationDecodeError {
    EmptyObjectSet,
    Wire(ProtectedPartCodecError),
    PublicationIdMismatch {
        expected: PublicationId,
        actual: PublicationId,
    },
    Protection(SyncPartError),
    IncompleteMultipartPublication,
    MultipleCompletedPublications,
}

impl core::fmt::Display for PendingRecoveryPublicationDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyObjectSet => write!(f, "pending publication contains no protected objects"),
            Self::Wire(error) => write!(f, "pending publication wire error: {error}"),
            Self::PublicationIdMismatch { expected, actual } => write!(
                f,
                "pending publication wire identity mismatch: expected {expected:?}, got {actual:?}"
            ),
            Self::Protection(error) => {
                write!(f, "pending publication protection error: {error}")
            }
            Self::IncompleteMultipartPublication => {
                write!(f, "pending publication does not contain a complete multipart set")
            }
            Self::MultipleCompletedPublications => write!(
                f,
                "pending outbox entry contains more than one completed publication"
            ),
        }
    }
}

impl std::error::Error for PendingRecoveryPublicationDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Wire(error) => Some(error),
            Self::Protection(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum ScalarRecoveryResumeError<TransportError, TrustedError, StoreError, CursorError> {
    CatchUp(ScalarRecoveryCatchUpError<TransportError, TrustedError, StoreError, CursorError>),
    PendingDecode {
        publication_id: PublicationId,
        error: PendingRecoveryPublicationDecodeError,
    },
    Publish(BatchPublishError<TransportError, CursorError>),
    Reconcile(BatchCommitError<StoreError, CursorError>),
}

pub type ScalarRecoveryResumeResult<R, TransportError, TrustedError, StoreError, CursorError> =
    Result<
        ScalarRecoveryResumeReport<R>,
        ScalarRecoveryResumeError<TransportError, TrustedError, StoreError, CursorError>,
    >;

/// Resume a complete multi-domain scalar recovery record containing any number of
/// durable pending publications.
///
/// Pending publications are classified only by equality with the durable cursor
/// after catch-up. IDs are mathematical set members; their canonical order is
/// used only for deterministic batch input and never as time or priority.
///
/// A stale publication is retired without retransmission only if its exact
/// durable protected object set authenticates as one complete publication and,
/// for every semantic merge domain carried by that publication, all carried
/// revision identities are present in the authenticated observation evidence from
/// this exact catch-up pass. Partial per-domain evidence is insufficient.
///
/// Any unresolved stale set blocks publication of the current-cursor set and
/// returns `NeedsRebase`. The function performs at most one transport publish
/// mutation; conflict looping belongs to a separately bounded orchestration layer.
#[allow(clippy::type_complexity)]
pub fn resume_scalar_recovery_outbox_set<T, S, TC, CC>(
    recovery: &mut LocalScalarRecoveryState,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    transport: &mut T,
    spec: ScalarRecoveryResumeSpec<'_>,
) -> ScalarRecoveryResumeResult<T::Revision, T::Error, TC::Error, S::Error, CC::Error>
where
    T: OpaqueTransport,
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarRecoveryState>,
    CC: TransportCursorCodec<T::Revision>,
{
    let initial_pending: BTreeSet<PublicationId> = record.outbox().keys().copied().collect();

    let catch_up = catch_up_scalar_recovery_state(
        recovery,
        record,
        store,
        trusted_codec,
        cursor_codec,
        transport,
        ScalarRecoveryCatchUpSpec::new(
            spec.key,
            spec.continuum_id,
            spec.pre_observation_revision_ids,
        ),
    )
    .map_err(ScalarRecoveryResumeError::CatchUp)?;

    if matches!(
        catch_up,
        ScalarRecoveryCatchUpOutcome::BaselineUnavailable { .. }
    ) {
        return Ok(ScalarRecoveryResumeReport {
            catch_up,
            observed_reconciled: BTreeSet::new(),
            action: ScalarRecoveryResumeAction::BlockedByBaseline,
        });
    }

    if initial_pending.is_empty() {
        return Ok(ScalarRecoveryResumeReport {
            catch_up,
            observed_reconciled: BTreeSet::new(),
            action: ScalarRecoveryResumeAction::None,
        });
    }

    let current_cursor = record.applied_cursor().cloned();
    let mut current = BTreeSet::new();
    let mut stale = BTreeSet::new();

    for publication_id in initial_pending {
        let entry = record
            .outbox()
            .get(&publication_id)
            .expect("catch-up preserves durable pending outbox entries");
        if entry.expected_cursor() == current_cursor.as_ref() {
            current.insert(publication_id);
        } else {
            stale.insert(publication_id);
        }
    }

    let mut observed_reconciled = BTreeSet::new();

    if !stale.is_empty() {
        if let ScalarRecoveryCatchUpOutcome::Applied {
            head,
            observed_revision_ids,
            ..
        } = &catch_up
        {
            for publication_id in &stale {
                let entry = record
                    .outbox()
                    .get(publication_id)
                    .expect("stale classification references durable outbox");
                let pending = decode_pending_recovery_publication(
                    spec.key,
                    spec.continuum_id,
                    *publication_id,
                    entry.objects(),
                )
                .map_err(|error| ScalarRecoveryResumeError::PendingDecode {
                    publication_id: *publication_id,
                    error,
                })?;

                if projection_is_observed(&pending, observed_revision_ids) {
                    observed_reconciled.insert(*publication_id);
                }
            }

            if !observed_reconciled.is_empty() {
                let trusted_state = record.trusted_state().to_vec();
                commit_reconciled_outbox_batch(
                    record,
                    store,
                    cursor_codec,
                    observed_reconciled.iter().copied(),
                    trusted_state,
                    head,
                )
                .map_err(ScalarRecoveryResumeError::Reconcile)?;
                stale.retain(|publication_id| !observed_reconciled.contains(publication_id));
            }
        }

        if !stale.is_empty() {
            return Ok(ScalarRecoveryResumeReport {
                catch_up,
                observed_reconciled,
                action: ScalarRecoveryResumeAction::NeedsRebase {
                    publication_ids: stale,
                },
            });
        }
    }

    if current.is_empty() {
        return Ok(ScalarRecoveryResumeReport {
            catch_up,
            observed_reconciled,
            action: ScalarRecoveryResumeAction::None,
        });
    }

    match publish_staged_batch(record, current.iter().copied(), transport, cursor_codec)
        .map_err(ScalarRecoveryResumeError::Publish)?
    {
        PublishOutcome::Published { head } => {
            let trusted_state = record.trusted_state().to_vec();
            commit_reconciled_outbox_batch(
                record,
                store,
                cursor_codec,
                current.iter().copied(),
                trusted_state,
                &head,
            )
            .map_err(ScalarRecoveryResumeError::Reconcile)?;

            Ok(ScalarRecoveryResumeReport {
                catch_up,
                observed_reconciled,
                action: ScalarRecoveryResumeAction::PublishedAndReconciled {
                    publication_ids: current,
                    head,
                },
            })
        }
        PublishOutcome::Conflict { current_head } => Ok(ScalarRecoveryResumeReport {
            catch_up,
            observed_reconciled,
            action: ScalarRecoveryResumeAction::Conflict {
                publication_ids: current,
                current_head,
            },
        }),
    }
}

fn projection_is_observed(
    projection: &ScalarSyncProjection,
    observed: &BTreeMap<apc_sync::DomainKey, BTreeSet<RevisionId>>,
) -> bool {
    projection.domains().iter().all(|(domain_key, register)| {
        let Some(observed_ids) = observed.get(domain_key) else {
            return false;
        };
        register
            .revisions()
            .all(|revision| observed_ids.contains(&revision.id))
    })
}

fn decode_pending_recovery_publication(
    key: &ContentKey,
    continuum_id: ContinuumId,
    expected_publication_id: PublicationId,
    encoded_objects: &[Vec<u8>],
) -> Result<ScalarSyncProjection, PendingRecoveryPublicationDecodeError> {
    if encoded_objects.is_empty() {
        return Err(PendingRecoveryPublicationDecodeError::EmptyObjectSet);
    }

    let mut inbox = MultipartInbox::new();
    let mut completed = None;
    let mut seen_wire_objects: BTreeSet<&[u8]> = BTreeSet::new();

    for encoded in encoded_objects {
        if !seen_wire_objects.insert(encoded.as_slice()) {
            continue;
        }

        let part = decode_protected_sync_part(encoded)
            .map_err(PendingRecoveryPublicationDecodeError::Wire)?;
        if part.publication_id != expected_publication_id {
            return Err(
                PendingRecoveryPublicationDecodeError::PublicationIdMismatch {
                    expected: expected_publication_id,
                    actual: part.publication_id,
                },
            );
        }

        if let Some(projection) = inbox
            .ingest(key, continuum_id, part)
            .map_err(PendingRecoveryPublicationDecodeError::Protection)?
        {
            if completed.replace(projection).is_some() {
                return Err(PendingRecoveryPublicationDecodeError::MultipleCompletedPublications);
            }
        }
    }

    if inbox.pending_publications() != 0 {
        return Err(PendingRecoveryPublicationDecodeError::IncompleteMultipartPublication);
    }

    completed.ok_or(PendingRecoveryPublicationDecodeError::IncompleteMultipartPublication)
}

#[cfg(test)]
mod tests {
    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, LocalScalarDomain, ScalarRegister, WorkingEpochId};
    use apc_sync::{
        DomainKey, FetchOutcome, SyncRecordStore, TransportCursor,
    };

    use crate::{
        prepare_recovery_handoff, stage_prepared_recovery_handoff,
        DevelopmentMultiScalarTrustedStateCodec,
    };

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

    struct ResumeTransport {
        head: Revision,
        fetched_objects: Vec<Vec<u8>>,
        fetch_calls: usize,
        publish_calls: usize,
        published_objects: Vec<Vec<u8>>,
    }

    impl OpaqueTransport for ResumeTransport {
        type Revision = Revision;
        type Error = &'static str;

        fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
            Ok(Some(self.head))
        }

        fn fetch_since(
            &mut self,
            known_head: Option<&Self::Revision>,
        ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
            self.fetch_calls += 1;
            if known_head == Some(&self.head) {
                Ok(FetchOutcome::UpToDate {
                    head: Some(self.head),
                })
            } else {
                Ok(FetchOutcome::Changed {
                    head: self.head,
                    objects: self.fetched_objects.clone(),
                })
            }
        }

        fn publish(
            &mut self,
            expected_head: Option<&Self::Revision>,
            objects: &[Vec<u8>],
        ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
            self.publish_calls += 1;
            if expected_head != Some(&self.head) {
                return Ok(PublishOutcome::Conflict {
                    current_head: Some(self.head),
                });
            }
            self.published_objects = objects.to_vec();
            self.head = Revision(self.head.0 + 1);
            Ok(PublishOutcome::Published { head: self.head })
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

    fn finalized_domain(
        base_revision: u64,
        local_revision: u64,
        epoch: u64,
        base: &str,
        value: &str,
    ) -> LocalScalarDomain<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal
            .assign(rid(base_revision), base.as_bytes().to_vec())
            .unwrap();
        let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
        domain
            .begin_epoch(wid(epoch), value.as_bytes().to_vec())
            .unwrap();
        domain.seal_local(rid(local_revision)).unwrap();
        domain.finalize(rid(local_revision)).unwrap();
        domain
    }

    fn prepared_recovery() -> (LocalScalarRecoveryState, DomainKey, DomainKey) {
        let body_key = domain("body");
        let title_key = domain("title");
        let body = finalized_domain(100, 200, 1, "body-base", "body-local");
        let title = finalized_domain(300, 400, 2, "title-base", "title-local");
        let recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (body_key.clone(), body.snapshot()),
            (title_key.clone(), title.snapshot()),
        ]))
        .unwrap();
        (recovery, body_key, title_key)
    }

    fn selections(
        body_key: &DomainKey,
        title_key: &DomainKey,
    ) -> BTreeMap<DomainKey, BTreeSet<RevisionId>> {
        BTreeMap::from([
            (body_key.clone(), BTreeSet::from([rid(200)])),
            (title_key.clone(), BTreeSet::from([rid(400)])),
        ])
    }

    #[test]
    fn authenticated_lost_ack_reconciles_multi_domain_publication_without_republish() {
        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xF1; 32]);
        let (mut recovery, body_key, title_key) = prepared_recovery();
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        let prepared = prepare_recovery_handoff(
            &recovery,
            cid(1),
            pid(7),
            &key,
            selections(&body_key, &title_key),
        )
        .unwrap();
        let accepted_objects = prepared.objects().to_vec();
        stage_prepared_recovery_handoff(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            prepared,
        )
        .unwrap();

        let mut transport = ResumeTransport {
            head: Revision(2),
            fetched_objects: accepted_objects,
            fetch_calls: 0,
            publish_calls: 0,
            published_objects: Vec::new(),
        };
        let pre_observation = BTreeMap::new();

        let report = resume_scalar_recovery_outbox_set(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            &mut transport,
            ScalarRecoveryResumeSpec::new(&key, cid(1), &pre_observation),
        )
        .unwrap();

        assert_eq!(report.observed_reconciled, BTreeSet::from([pid(7)]));
        assert_eq!(report.action, ScalarRecoveryResumeAction::None);
        assert_eq!(transport.fetch_calls, 1);
        assert_eq!(transport.publish_calls, 0);
        assert!(record.outbox().is_empty());
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
    }

    #[test]
    fn same_cursor_multi_domain_publications_publish_as_one_exact_batch() {
        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xF2; 32]);
        let (mut recovery, body_key, title_key) = prepared_recovery();
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();

        for publication_id in [pid(2), pid(1)] {
            let prepared = prepare_recovery_handoff(
                &recovery,
                cid(1),
                publication_id,
                &key,
                selections(&body_key, &title_key),
            )
            .unwrap();
            stage_prepared_recovery_handoff(
                &mut recovery,
                &mut record,
                &mut store,
                &codec,
                prepared,
            )
            .unwrap();
        }
        let expected_object_count: usize = record.outbox().values().map(|e| e.objects().len()).sum();

        let mut transport = ResumeTransport {
            head: Revision(1),
            fetched_objects: Vec::new(),
            fetch_calls: 0,
            publish_calls: 0,
            published_objects: Vec::new(),
        };
        let pre_observation = BTreeMap::new();

        let report = resume_scalar_recovery_outbox_set(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            &mut transport,
            ScalarRecoveryResumeSpec::new(&key, cid(1), &pre_observation),
        )
        .unwrap();

        assert_eq!(
            report.action,
            ScalarRecoveryResumeAction::PublishedAndReconciled {
                publication_ids: BTreeSet::from([pid(1), pid(2)]),
                head: Revision(2),
            }
        );
        assert_eq!(transport.publish_calls, 1);
        assert_eq!(transport.published_objects.len(), expected_object_count);
        assert!(record.outbox().is_empty());
    }

    #[test]
    fn stale_multi_domain_publication_without_complete_evidence_needs_rebase() {
        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xF3; 32]);
        let (mut recovery, body_key, title_key) = prepared_recovery();
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        let prepared = prepare_recovery_handoff(
            &recovery,
            cid(1),
            pid(9),
            &key,
            selections(&body_key, &title_key),
        )
        .unwrap();
        stage_prepared_recovery_handoff(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            prepared,
        )
        .unwrap();

        let mut transport = ResumeTransport {
            head: Revision(2),
            fetched_objects: Vec::new(),
            fetch_calls: 0,
            publish_calls: 0,
            published_objects: Vec::new(),
        };
        let pre_observation = BTreeMap::new();

        let report = resume_scalar_recovery_outbox_set(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            &mut transport,
            ScalarRecoveryResumeSpec::new(&key, cid(1), &pre_observation),
        )
        .unwrap();

        assert_eq!(
            report.action,
            ScalarRecoveryResumeAction::NeedsRebase {
                publication_ids: BTreeSet::from([pid(9)])
            }
        );
        assert!(record.outbox().contains_key(&pid(9)));
        assert_eq!(transport.publish_calls, 0);
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
    }
}
