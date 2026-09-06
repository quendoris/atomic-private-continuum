use apc_core::{ContinuumId, CoreError, LocalScalarDomain, LocalScalarSnapshot, RevisionId, ScalarRevision};
use apc_crypto::ContentKey;
use apc_sync::{
    commit_received, fetch_from_durable_cursor, DomainKey, DurableSyncRecord, FetchOutcome,
    OpaqueTransport, SessionCommitError, SessionIoError, SyncRecordStore, TransportCursorCodec,
};

use crate::{
    commit_received_scalar_domain, decode_single_scalar_domain_object, ReceivedScalarState,
    ScalarObjectDecodeError, ScalarReceiveCommitError, TrustedStateCodec,
};

/// Immutable parameters for one single-domain scalar catch-up pass.
///
/// The pre-observation revision is consumed only if authenticated semantic state
/// is actually present and the local domain is dirty. A transport-head advance
/// containing no A.P.C. objects therefore does not manufacture a local causal
/// revision.
pub struct ScalarCatchUpSpec<'a> {
    key: &'a ContentKey,
    continuum_id: ContinuumId,
    domain_key: &'a DomainKey,
    pre_observation_revision_id: Option<RevisionId>,
}

impl<'a> ScalarCatchUpSpec<'a> {
    pub fn new(
        key: &'a ContentKey,
        continuum_id: ContinuumId,
        domain_key: &'a DomainKey,
        pre_observation_revision_id: Option<RevisionId>,
    ) -> Self {
        Self {
            key,
            continuum_id,
            domain_key,
            pre_observation_revision_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScalarCatchUpOutcome<R> {
    UpToDate {
        head: Option<R>,
    },
    Applied {
        head: R,
        object_count: usize,
        sealed_local: Option<ScalarRevision<Vec<u8>>>,
    },
    CursorAdvancedWithoutSemanticObjects {
        head: R,
    },
    BaselineUnavailable {
        head: Option<R>,
    },
}

#[derive(Debug)]
pub enum ScalarCatchUpError<TransportError, TrustedError, StoreError, CursorError> {
    Io(SessionIoError<TransportError, CursorError>),
    Decode(ScalarObjectDecodeError),
    Core(CoreError),
    TrustedState(TrustedError),
    Commit(SessionCommitError<StoreError, CursorError>),
    Receive(ScalarReceiveCommitError<TrustedError, StoreError, CursorError>),
}

pub type ScalarCatchUpResult<R, TransportError, TrustedError, StoreError, CursorError> = Result<
    ScalarCatchUpOutcome<R>,
    ScalarCatchUpError<TransportError, TrustedError, StoreError, CursorError>,
>;

/// Execute one durable-cursor catch-up pass for the current single-domain scalar
/// runtime path.
///
/// The transport may be wrapped in `ForegroundTransport`; in that case invoking
/// this function while backgrounded fails before transport I/O and leaves all
/// semantic/durable state untouched.
///
/// Every returned protected object must decode as one complete authenticated
/// single-part publication for `spec.domain_key`. All decoded scalar states are
/// merged before a single semantic observation boundary is crossed. Dirty local
/// work is therefore sealed once against the frontier it actually observed, not
/// once per fetched transport object.
pub fn catch_up_single_scalar_domain<T, S, TC, CC>(
    domain: &mut LocalScalarDomain<Vec<u8>>,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    transport: &mut T,
    spec: ScalarCatchUpSpec<'_>,
) -> ScalarCatchUpResult<T::Revision, T::Error, TC::Error, S::Error, CC::Error>
where
    T: OpaqueTransport,
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarSnapshot<Vec<u8>>>,
    CC: TransportCursorCodec<T::Revision>,
{
    let fetched = fetch_from_durable_cursor(record, transport, cursor_codec)
        .map_err(ScalarCatchUpError::Io)?;

    match fetched {
        FetchOutcome::UpToDate { head } => Ok(ScalarCatchUpOutcome::UpToDate { head }),
        FetchOutcome::BaselineUnavailable { head } => {
            Ok(ScalarCatchUpOutcome::BaselineUnavailable { head })
        }
        FetchOutcome::Changed { head, objects } => {
            if objects.is_empty() {
                let trusted_state = trusted_codec
                    .encode(&domain.snapshot())
                    .map_err(ScalarCatchUpError::TrustedState)?;
                commit_received(record, store, cursor_codec, trusted_state, &head)
                    .map_err(ScalarCatchUpError::Commit)?;
                return Ok(ScalarCatchUpOutcome::CursorAdvancedWithoutSemanticObjects {
                    head,
                });
            }

            let object_count = objects.len();
            let mut combined_remote = None;
            for encoded in objects {
                let decoded = decode_single_scalar_domain_object(
                    spec.key,
                    spec.continuum_id,
                    spec.domain_key,
                    &encoded,
                )
                .map_err(ScalarCatchUpError::Decode)?;

                combined_remote = Some(match combined_remote {
                    None => decoded,
                    Some(current) => current
                        .merge(&decoded)
                        .map_err(ScalarCatchUpError::Core)?,
                });
            }

            let remote = combined_remote.expect("non-empty object set produces remote state");
            let sealed_local = commit_received_scalar_domain(
                domain,
                record,
                store,
                trusted_codec,
                cursor_codec,
                ReceivedScalarState::new(
                    &remote,
                    spec.pre_observation_revision_id,
                    &head,
                ),
            )
            .map_err(ScalarCatchUpError::Receive)?;

            Ok(ScalarCatchUpOutcome::Applied {
                head,
                object_count,
                sealed_local,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, ScalarRegister, WorkingEpochId};
    use apc_sync::{
        ForegroundSyncLifecycle, ForegroundTransportError, PublicationId, PublishOutcome,
        SyncRecordStore, TransportCursor,
    };

    use crate::{prepare_scalar_handoff, DevelopmentScalarTrustedStateCodec};

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

    struct CatchUpTransport {
        head: Revision,
        objects: Vec<Vec<u8>>,
        fetch_calls: usize,
    }

    impl OpaqueTransport for CatchUpTransport {
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

    fn atom(value: u64) -> AtomId {
        AtomId::from_bytes(bytes(value))
    }

    fn domain_key() -> DomainKey {
        DomainKey::new(atom(1), b"body".to_vec()).unwrap()
    }

    fn base() -> ScalarRegister<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), b"base".to_vec()).unwrap();
        causal
    }

    fn sender() -> LocalScalarDomain<Vec<u8>> {
        let mut domain = LocalScalarDomain::from_causal(base()).unwrap();
        domain.begin_epoch(wid(9), b"remote".to_vec()).unwrap();
        domain.seal_local(rid(900)).unwrap();
        domain.finalize(rid(900)).unwrap();
        domain
    }

    fn dirty_receiver() -> LocalScalarDomain<Vec<u8>> {
        let mut domain = LocalScalarDomain::from_causal(base()).unwrap();
        domain.begin_epoch(wid(1), b"local".to_vec()).unwrap();
        domain
    }

    #[test]
    fn background_blocks_resume_catch_up_then_foreground_applies_once() {
        let key = ContentKey::from_bytes([0x91; 32]);
        let semantic_key = domain_key();
        let prepared = prepare_scalar_handoff(
            &sender(),
            semantic_key.clone(),
            cid(1),
            pid(1),
            &key,
            [rid(900)],
        )
        .unwrap();

        let lifecycle = ForegroundSyncLifecycle::new();
        let mut transport = lifecycle.guard(CatchUpTransport {
            head: Revision(2),
            objects: prepared.publication().objects().to_vec(),
            fetch_calls: 0,
        });
        let cursor_codec = RevisionCodec;
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let mut domain = dirty_receiver();
        let mut record = DurableSyncRecord::new(
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let before_domain = domain.clone();
        let before_record = record.clone();
        let mut store = MemoryStore::default();

        let error = catch_up_single_scalar_domain(
            &mut domain,
            &mut record,
            &mut store,
            &trusted_codec,
            &cursor_codec,
            &mut transport,
            ScalarCatchUpSpec::new(&key, cid(1), &semantic_key, Some(rid(200))),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ScalarCatchUpError::Io(SessionIoError::Transport(
                ForegroundTransportError::Backgrounded
            ))
        ));
        assert_eq!(domain, before_domain);
        assert_eq!(record, before_record);
        assert_eq!(transport.inner().fetch_calls, 0);

        lifecycle.enter_foreground();
        let outcome = catch_up_single_scalar_domain(
            &mut domain,
            &mut record,
            &mut store,
            &trusted_codec,
            &cursor_codec,
            &mut transport,
            ScalarCatchUpSpec::new(&key, cid(1), &semantic_key, Some(rid(200))),
        )
        .unwrap();

        let ScalarCatchUpOutcome::Applied {
            head,
            object_count,
            sealed_local,
        } = outcome
        else {
            panic!("expected applied catch-up")
        };
        assert_eq!(head, Revision(2));
        assert_eq!(object_count, 1);
        assert_eq!(
            sealed_local.unwrap().parents,
            BTreeSet::from([rid(100)])
        );
        assert_eq!(
            domain.causal().frontier_ids(),
            BTreeSet::from([rid(200), rid(900)])
        );
        assert_eq!(
            cursor_codec.decode(record.applied_cursor().unwrap()).unwrap(),
            Revision(2)
        );
        assert_eq!(transport.inner().fetch_calls, 1);
        assert_eq!(store.committed.as_ref(), Some(&record));
    }

    #[test]
    fn empty_transport_change_advances_cursor_without_sealing_dirty_work() {
        let key = ContentKey::from_bytes([0x92; 32]);
        let semantic_key = domain_key();
        let cursor_codec = RevisionCodec;
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let mut domain = dirty_receiver();
        let before_domain = domain.clone();
        let mut record = DurableSyncRecord::new(
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        let mut transport = CatchUpTransport {
            head: Revision(2),
            objects: Vec::new(),
            fetch_calls: 0,
        };

        let outcome = catch_up_single_scalar_domain(
            &mut domain,
            &mut record,
            &mut store,
            &trusted_codec,
            &cursor_codec,
            &mut transport,
            ScalarCatchUpSpec::new(&key, cid(1), &semantic_key, Some(rid(200))),
        )
        .unwrap();

        assert_eq!(
            outcome,
            ScalarCatchUpOutcome::CursorAdvancedWithoutSemanticObjects {
                head: Revision(2)
            }
        );
        assert_eq!(domain, before_domain);
        assert!(domain.pending().is_some());
        assert!(domain.causal().revision(rid(200)).is_none());
        assert_eq!(
            cursor_codec.decode(record.applied_cursor().unwrap()).unwrap(),
            Revision(2)
        );
    }
}
