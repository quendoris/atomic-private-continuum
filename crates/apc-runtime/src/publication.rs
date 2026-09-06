use std::collections::{BTreeMap, BTreeSet};

use apc_core::{
    ContinuumId, CoreError, LocalScalarDomain, LocalScalarSnapshot, RevisionId, ScalarRegister,
};
use apc_crypto::ContentKey;
use apc_sync::{
    encode_protected_sync_part, protect_scalar_part, stage_outbound, DomainKey, DurableSyncRecord,
    PersistTransitionError, ProtectedPartCodecError, PublicationId, ScalarSyncProjection,
    SyncPartError, SyncProjection, SyncRecordStore,
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
    fn new(publication_id: PublicationId, objects: Vec<Vec<u8>>) -> Self {
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

/// A protected scalar publication coupled to the exact direct revisions whose
/// causal dependency closure it carries.
///
/// Keeping these values together prevents application glue from protecting one
/// semantic state while durably recording exposure for another. The wire object
/// is created from the selected dependency closure before this value exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedScalarHandoff {
    revision_ids: BTreeSet<RevisionId>,
    publication: ProtectedPublication,
}

impl PreparedScalarHandoff {
    pub fn revision_ids(&self) -> &BTreeSet<RevisionId> {
        &self.revision_ids
    }

    pub fn publication(&self) -> &ProtectedPublication {
        &self.publication
    }
}

#[derive(Debug)]
pub enum ScalarPublicationPrepareError {
    EmptyRevisionSet,
    Core(CoreError),
    Protection(SyncPartError),
    Wire(ProtectedPartCodecError),
}

impl core::fmt::Display for ScalarPublicationPrepareError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyRevisionSet => {
                write!(f, "scalar publication must name at least one revision")
            }
            Self::Core(error) => write!(f, "scalar publication semantic error: {error}"),
            Self::Protection(error) => write!(f, "scalar publication protection error: {error}"),
            Self::Wire(error) => write!(f, "scalar publication wire error: {error}"),
        }
    }
}

impl std::error::Error for ScalarPublicationPrepareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Protection(error) => Some(error),
            Self::Wire(error) => Some(error),
            Self::EmptyRevisionSet => None,
        }
    }
}

impl From<CoreError> for ScalarPublicationPrepareError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

impl From<SyncPartError> for ScalarPublicationPrepareError {
    fn from(value: SyncPartError) -> Self {
        Self::Protection(value)
    }
}

impl From<ProtectedPartCodecError> for ScalarPublicationPrepareError {
    fn from(value: ProtectedPartCodecError) -> Self {
        Self::Wire(value)
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

/// Construct the first complete semantic-to-protected-wire scalar publication.
///
/// Only the causal dependency closure of `revision_ids` is included. Unrelated
/// concurrent revisions in the same register are deliberately excluded. A
/// cloned semantic domain is asked to perform the same handoff first, which
/// proves that every locally-owned member of the closure is finalized before any
/// protected bytes are produced.
///
/// The resulting object contains an authenticated encrypted scalar projection
/// wrapped in the transport-facing protected-part encoding. It is still a
/// development/pre-format representation, not the native `.apc` format.
pub fn prepare_scalar_handoff<I>(
    domain: &LocalScalarDomain<Vec<u8>>,
    domain_key: DomainKey,
    continuum_id: ContinuumId,
    publication_id: PublicationId,
    key: &ContentKey,
    revision_ids: I,
) -> Result<PreparedScalarHandoff, ScalarPublicationPrepareError>
where
    I: IntoIterator<Item = RevisionId>,
{
    let revision_ids: BTreeSet<RevisionId> = revision_ids.into_iter().collect();
    if revision_ids.is_empty() {
        return Err(ScalarPublicationPrepareError::EmptyRevisionSet);
    }

    // Use the actual semantic handoff rule as the publication eligibility check.
    // This mutates only a throwaway clone and therefore cannot record exposure
    // before the later durable staging boundary.
    let mut candidate = domain.clone();
    candidate.handoff(revision_ids.iter().copied())?;

    let closure = dependency_closure(domain.causal(), &revision_ids)?;
    let revisions = closure
        .iter()
        .map(|revision_id| {
            domain
                .causal()
                .revision(*revision_id)
                .cloned()
                .ok_or(CoreError::UnknownRevision {
                    revision_id: *revision_id,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let register = ScalarRegister::from_revisions(revisions)?;
    let projection: ScalarSyncProjection =
        SyncProjection::from_domains(BTreeMap::from([(domain_key, register)]));

    let part = protect_scalar_part(key, continuum_id, publication_id, 0, 1, &projection)?;
    let wire = encode_protected_sync_part(&part)?;

    Ok(PreparedScalarHandoff {
        revision_ids,
        publication: ProtectedPublication::new(publication_id, vec![wire]),
    })
}

/// Record semantic handoff/exposure and the matching prepared wire bytes as one
/// durable runtime transition.
///
/// The caller cannot independently substitute revision IDs here: they travel in
/// the same `PreparedScalarHandoff` that was built from the protected semantic
/// dependency closure. The live semantic domain changes only after the durable
/// recovery record and exact retry bytes have been committed.
pub fn stage_prepared_scalar_handoff<T, S, C>(
    domain: &mut LocalScalarDomain<T>,
    record: &mut DurableSyncRecord,
    store: &mut S,
    codec: &C,
    prepared: PreparedScalarHandoff,
) -> Result<(), ScalarHandoffStageError<C::Error, S::Error>>
where
    T: Clone + Eq,
    S: SyncRecordStore,
    C: TrustedStateCodec<LocalScalarSnapshot<T>>,
{
    stage_scalar_handoff(
        domain,
        record,
        store,
        codec,
        prepared.revision_ids,
        prepared.publication,
    )
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

fn stage_scalar_handoff<T, S, C>(
    domain: &mut LocalScalarDomain<T>,
    record: &mut DurableSyncRecord,
    store: &mut S,
    codec: &C,
    revision_ids: BTreeSet<RevisionId>,
    publication: ProtectedPublication,
) -> Result<(), ScalarHandoffStageError<C::Error, S::Error>>
where
    T: Clone + Eq,
    S: SyncRecordStore,
    C: TrustedStateCodec<LocalScalarSnapshot<T>>,
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

fn dependency_closure<T: Clone + Eq>(
    causal: &ScalarRegister<T>,
    revision_ids: &BTreeSet<RevisionId>,
) -> Result<BTreeSet<RevisionId>, CoreError> {
    let mut closure = BTreeSet::new();
    let mut stack: Vec<RevisionId> = revision_ids.iter().copied().collect();

    while let Some(revision_id) = stack.pop() {
        if !closure.insert(revision_id) {
            continue;
        }
        let revision = causal
            .revision(revision_id)
            .ok_or(CoreError::UnknownRevision { revision_id })?;
        stack.extend(revision.parents.iter().copied());
    }

    Ok(closure)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, ScalarRegister, WorkingEpochId};
    use apc_sync::{
        decode_protected_sync_part, unprotect_scalar_part, SyncRecoveryError, TransportCursor,
    };

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
            self.snapshots
                .borrow_mut()
                .insert(token.clone(), state.clone());
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
        PublicationId::from_bytes(logical_bytes(value))
    }

    fn cid(value: u64) -> ContinuumId {
        ContinuumId::from_bytes(logical_bytes(value))
    }

    fn atom(value: u64) -> AtomId {
        AtomId::from_bytes(logical_bytes(value))
    }

    fn domain_key() -> DomainKey {
        DomainKey::new(atom(1), b"body".to_vec()).unwrap()
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

    fn prepared_bytes_local(finalize: bool) -> LocalScalarDomain<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), b"base".to_vec()).unwrap();
        let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
        domain.begin_epoch(wid(1), b"local".to_vec()).unwrap();
        domain.seal_local(rid(200)).unwrap();
        if finalize {
            domain.finalize(rid(200)).unwrap();
        }
        domain
    }

    #[test]
    fn protected_builder_carries_only_selected_dependency_closure() {
        let key = ContentKey::from_bytes([0x61; 32]);
        let mut domain = prepared_bytes_local(true);

        let mut remote = ScalarRegister::new();
        remote.assign(rid(100), b"base".to_vec()).unwrap();
        remote
            .assign(rid(900), b"unrelated-remote".to_vec())
            .unwrap();
        domain.observe_remote(&remote, None).unwrap();

        let key_name = domain_key();
        let prepared =
            prepare_scalar_handoff(&domain, key_name.clone(), cid(7), pid(8), &key, [rid(200)])
                .unwrap();

        assert_eq!(prepared.revision_ids(), &BTreeSet::from([rid(200)]));
        assert_eq!(prepared.publication().objects().len(), 1);

        let part = decode_protected_sync_part(&prepared.publication().objects()[0]).unwrap();
        let projection = unprotect_scalar_part(&key, cid(7), &part).unwrap();
        let published = projection.get(&key_name).unwrap();

        assert!(published.revision(rid(100)).is_some());
        assert!(published.revision(rid(200)).is_some());
        assert!(published.revision(rid(900)).is_none());
        assert_eq!(published.len(), 2);
    }

    #[test]
    fn protected_builder_rejects_unfinalized_local_dependency_before_encryption() {
        let key = ContentKey::from_bytes([0x62; 32]);
        let domain = prepared_bytes_local(false);

        let error = prepare_scalar_handoff(&domain, domain_key(), cid(7), pid(9), &key, [rid(200)])
            .unwrap_err();

        assert!(matches!(
            error,
            ScalarPublicationPrepareError::Core(
                CoreError::HandoffRequiresFinalizedRevision { revision_id }
            ) if revision_id == rid(200)
        ));
    }

    #[test]
    fn protected_builder_rejects_empty_revision_set() {
        let key = ContentKey::from_bytes([0x63; 32]);
        let domain = prepared_bytes_local(true);

        assert!(matches!(
            prepare_scalar_handoff(
                &domain,
                domain_key(),
                cid(7),
                pid(10),
                &key,
                std::iter::empty(),
            ),
            Err(ScalarPublicationPrepareError::EmptyRevisionSet)
        ));
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
            BTreeSet::from([rid(200)]),
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
            BTreeSet::from([rid(200)]),
            ProtectedPublication::new(pid(2), vec![b"already-protected".to_vec()]),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ScalarHandoffStageError::Sync(PersistTransitionError::Store("durability failure"))
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
            BTreeSet::from([rid(200)]),
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
            BTreeSet::from([rid(200)]),
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
