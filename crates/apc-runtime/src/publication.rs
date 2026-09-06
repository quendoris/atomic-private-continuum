use apc_core::{CoreError, LocalScalarDomain, LocalScalarSnapshot, RevisionId};
use apc_sync::{
    stage_outbound, DurableSyncRecord, PersistTransitionError, PublicationId, SyncRecordStore,
};

/// Runtime codec for trusted local recovery state.
///
/// The trait is intentionally separate from the portable `.apc` format and from
/// sync-projection encoding. Implementations may evolve while the local recovery
/// representation is still pre-format, but encode/decode must be mutually
/// consistent for every durable record they create.
pub trait TrustedStateCodec<S> {
    type Error;

    fn encode(&self, state: &S) -> Result<Vec<u8>, Self::Error>;
    fn decode(&self, bytes: &[u8]) -> Result<S, Self::Error>;
}

/// One already-protected publication ready to cross the durable exposure gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedPublication {
    publication_id: PublicationId,
    objects: Vec<Vec<u8>>,
}

impl ProtectedPublication {
    pub fn new(publication_id: PublicationId, objects: Vec<Vec<u8>>) -> Self {
        Self {
            publication_id,
            objects,
        }
    }

    pub fn publication_id(&self) -> PublicationId {
        self.publication_id
    }

    pub fn objects(&self) -> &[Vec<u8>] {
        &self.objects
    }
}

#[derive(Debug)]
pub enum ScalarHandoffStageError<CodecError, StoreError> {
    Core(CoreError),
    Codec(CodecError),
    Sync(PersistTransitionError<StoreError>),
}

#[derive(Debug)]
pub enum ScalarRecoveryError<CodecError> {
    Codec(CodecError),
    Core(CoreError),
}

/// Record semantic handoff/exposure and durable retry material as one runtime
/// transition for the first scalar-domain implementation path.
///
/// The candidate semantic domain is cloned first. `handoff()` therefore marks
/// local causal identities exposed only on the candidate. The candidate snapshot
/// is encoded into `DurableSyncRecord::trusted_state`, then `stage_outbound()`
/// persists that exposed trusted state together with the exact protected bytes.
/// Only after persistence succeeds is the caller's in-memory semantic domain
/// replaced by the exposed candidate.
///
/// A crash after persistence but before the in-memory assignment is safe: restart
/// recovers the already-exposed candidate from the durable trusted state. A
/// persistence failure leaves both caller-visible domain and sync record
/// unchanged.
pub fn stage_scalar_handoff<T, S, C, I>(
    domain: &mut LocalScalarDomain<T>,
    record: &mut DurableSyncRecord,
    store: &mut S,
    codec: &C,
    revision_ids: I,
    publication: ProtectedPublication,
) -> Result<(), ScalarHandoffStageError<C::Error, S::Error>>
where
    T: Clone + Eq,
    S: SyncRecordStore,
    C: TrustedStateCodec<LocalScalarSnapshot<T>>,
    I: IntoIterator<Item = RevisionId>,
{
    let mut candidate = domain.clone();
    candidate
        .handoff(revision_ids)
        .map_err(ScalarHandoffStageError::Core)?;

    let exposed_trusted_state = codec
        .encode(&candidate.snapshot())
        .map_err(ScalarHandoffStageError::Codec)?;

    stage_outbound(
        record,
        store,
        exposed_trusted_state,
        publication.publication_id,
        publication.objects,
    )
    .map_err(ScalarHandoffStageError::Sync)?;

    *domain = candidate;
    Ok(())
}

/// Restore the scalar semantic domain paired with a durable sync record.
///
/// This is a development scalar path, not a claim that one scalar snapshot is the
/// final continuum recovery image. The important boundary is that restart uses
/// the exact `trusted_state` committed with the durable cursor/outbox.
pub fn recover_scalar_domain<T, C>(
    record: &DurableSyncRecord,
    codec: &C,
) -> Result<LocalScalarDomain<T>, ScalarRecoveryError<C::Error>>
where
    T: Clone + Eq,
    C: TrustedStateCodec<LocalScalarSnapshot<T>>,
{
    let snapshot = codec
        .decode(record.trusted_state())
        .map_err(ScalarRecoveryError::Codec)?;
    LocalScalarDomain::restore(snapshot).map_err(ScalarRecoveryError::Core)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{ScalarRegister, WorkingEpochId};
    use apc_sync::{SyncRecoveryError, TransportCursor};

    use super::*;

    struct SnapshotVault<T> {
        next: Cell<u64>,
        snapshots: RefCell<BTreeMap<Vec<u8>, LocalScalarSnapshot<T>>>,
    }

    impl<T> Default for SnapshotVault<T> {
        fn default() -> Self {
            Self {
                next: Cell::new(0),
                snapshots: RefCell::new(BTreeMap::new()),
            }
        }
    }

    impl<T: Clone> TrustedStateCodec<LocalScalarSnapshot<T>> for SnapshotVault<T> {
        type Error = &'static str;

        fn encode(&self, state: &LocalScalarSnapshot<T>) -> Result<Vec<u8>, Self::Error> {
            let next = self.next.get() + 1;
            self.next.set(next);
            let token = next.to_be_bytes().to_vec();
            self.snapshots.borrow_mut().insert(token.clone(), state.clone());
            Ok(token)
        }

        fn decode(&self, bytes: &[u8]) -> Result<LocalScalarSnapshot<T>, Self::Error> {
            self.snapshots
                .borrow()
                .get(bytes)
                .cloned()
                .ok_or("unknown trusted-state token")
        }
    }

    #[derive(Default)]
    struct MemoryStore {
        committed: Option<DurableSyncRecord>,
        persist_calls: usize,
        fail: bool,
    }

    impl SyncRecordStore for MemoryStore {
        type Error = &'static str;

        fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
            self.persist_calls += 1;
            if self.fail {
                return Err("durability failure");
            }
            self.committed = Some(record.clone());
            Ok(())
        }
    }

    fn logical_bytes(value: u64) -> [u8; LOGICAL_ID_BYTES] {
        let mut bytes = [0_u8; LOGICAL_ID_BYTES];
        bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(logical_bytes(value))
    }

    fn wid(value: u64) -> WorkingEpochId {
        WorkingEpochId::from_bytes(logical_bytes(value))
    }

    fn pid(value: u64) -> PublicationId {
        let mut bytes = [0_u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        PublicationId::from_bytes(bytes)
    }

    fn base_domain() -> LocalScalarDomain<String> {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), "base".to_owned()).unwrap();
        LocalScalarDomain::from_causal(causal).unwrap()
    }

    fn prepared_local(finalize: bool) -> LocalScalarDomain<String> {
        let mut domain = base_domain();
        domain.begin_epoch(wid(1), "local".to_owned()).unwrap();
        domain.seal_local(rid(200)).unwrap();
        if finalize {
            domain.finalize(rid(200)).unwrap();
        }
        domain
    }

    #[test]
    fn handoff_exposure_and_outbox_become_durable_together() {
        let codec = SnapshotVault::default();
        let mut domain = prepared_local(true);
        let initial_state = codec.encode(&domain.snapshot()).unwrap();
        let mut record = DurableSyncRecord::new(initial_state, None);
        let mut store = MemoryStore::default();

        stage_scalar_handoff(
            &mut domain,
            &mut record,
            &mut store,
            &codec,
            [rid(200)],
            ProtectedPublication::new(pid(1), vec![b"already-protected".to_vec()]),
        )
        .unwrap();

        assert!(domain
            .finalization()
            .exposed_local_ids()
            .contains(&rid(200)));
        assert!(record.outbox().contains_key(&pid(1)));
        assert_eq!(store.committed.as_ref(), Some(&record));

        let recovered = recover_scalar_domain(&record, &codec).unwrap();
        assert_eq!(recovered, domain);
        assert!(recovered
            .finalization()
            .exposed_local_ids()
            .contains(&rid(200)));
    }

    #[test]
    fn persistence_failure_cannot_expose_only_the_in_memory_domain() {
        let codec = SnapshotVault::default();
        let mut domain = prepared_local(true);
        let initial_state = codec.encode(&domain.snapshot()).unwrap();
        let mut record = DurableSyncRecord::new(initial_state, None);
        let before_domain = domain.clone();
        let before_record = record.clone();
        let mut store = MemoryStore {
            fail: true,
            ..MemoryStore::default()
        };

        let error = stage_scalar_handoff(
            &mut domain,
            &mut record,
            &mut store,
            &codec,
            [rid(200)],
            ProtectedPublication::new(pid(2), vec![b"already-protected".to_vec()]),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ScalarHandoffStageError::Sync(PersistTransitionError::Store(
                "durability failure"
            ))
        ));
        assert_eq!(domain, before_domain);
        assert_eq!(record, before_record);
        assert!(domain.finalization().exposed_local_ids().is_empty());
    }

    #[test]
    fn unfinalized_local_dependency_cannot_cross_runtime_handoff() {
        let codec = SnapshotVault::default();
        let mut domain = prepared_local(false);
        let initial_state = codec.encode(&domain.snapshot()).unwrap();
        let mut record = DurableSyncRecord::new(initial_state, None);
        let before_domain = domain.clone();
        let before_record = record.clone();
        let mut store = MemoryStore::default();

        let error = stage_scalar_handoff(
            &mut domain,
            &mut record,
            &mut store,
            &codec,
            [rid(200)],
            ProtectedPublication::new(pid(3), vec![b"already-protected".to_vec()]),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ScalarHandoffStageError::Core(CoreError::HandoffRequiresFinalizedRevision {
                revision_id
            }) if revision_id == rid(200)
        ));
        assert_eq!(domain, before_domain);
        assert_eq!(record, before_record);
        assert_eq!(store.persist_calls, 0);
    }

    #[test]
    fn malformed_publication_still_fails_before_exposure_is_committed() {
        let codec = SnapshotVault::default();
        let mut domain = prepared_local(true);
        let initial_state = codec.encode(&domain.snapshot()).unwrap();
        let mut record = DurableSyncRecord::new(initial_state, None);
        let before_domain = domain.clone();
        let before_record = record.clone();
        let mut store = MemoryStore::default();

        let error = stage_scalar_handoff(
            &mut domain,
            &mut record,
            &mut store,
            &codec,
            [rid(200)],
            ProtectedPublication::new(pid(4), Vec::new()),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ScalarHandoffStageError::Sync(PersistTransitionError::Recovery(
                SyncRecoveryError::EmptyOutbox
            ))
        ));
        assert_eq!(domain, before_domain);
        assert_eq!(record, before_record);
        assert_eq!(store.persist_calls, 0);
    }

    #[test]
    fn trusted_state_cursor_shape_can_remain_transport_opaque() {
        let cursor = TransportCursor::new(b"opaque-head".to_vec()).unwrap();
        let codec = SnapshotVault::default();
        let domain = prepared_local(true);
        let trusted = codec.encode(&domain.snapshot()).unwrap();
        let record = DurableSyncRecord::new(trusted, Some(cursor.clone()));

        assert_eq!(record.applied_cursor(), Some(&cursor));
        assert_eq!(recover_scalar_domain(&record, &codec).unwrap(), domain);
    }
}
