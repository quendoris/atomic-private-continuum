use std::collections::{BTreeMap, BTreeSet};

use apc_core::{ContinuumId, CoreError, LocalScalarDomain, RevisionId, ScalarRegister};
use apc_crypto::ContentKey;
use apc_sync::{
    encode_protected_sync_part, protect_scalar_part, stage_outbound, DomainKey, DurableSyncRecord,
    PersistTransitionError, ProtectedPartCodecError, PublicationId, ScalarSyncProjection,
    SyncPartError, SyncProjection, SyncRecordStore,
};

use crate::{LocalScalarRecoveryState, LocalScalarRecoveryStateError, TrustedStateCodec};

/// One already-protected publication built from selected causal closures in
/// several independent scalar merge domains.
///
/// Grouping domains into one transport publication is packing only. It does not
/// create a semantic transaction between those domains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRecoveryHandoff {
    revision_ids: BTreeMap<DomainKey, BTreeSet<RevisionId>>,
    publication_id: PublicationId,
    objects: Vec<Vec<u8>>,
}

impl PreparedRecoveryHandoff {
    pub fn revision_ids(&self) -> &BTreeMap<DomainKey, BTreeSet<RevisionId>> {
        &self.revision_ids
    }

    pub fn publication_id(&self) -> PublicationId {
        self.publication_id
    }

    pub fn objects(&self) -> &[Vec<u8>] {
        &self.objects
    }
}

#[derive(Debug)]
pub enum RecoveryPublicationPrepareError {
    EmptySelection,
    EmptyRevisionSet { domain_key: DomainKey },
    MissingDomain { domain_key: DomainKey },
    Core(CoreError),
    Protection(SyncPartError),
    Wire(ProtectedPartCodecError),
}

impl core::fmt::Display for RecoveryPublicationPrepareError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptySelection => write!(f, "multi-domain publication must select at least one domain"),
            Self::EmptyRevisionSet { domain_key } => {
                write!(f, "publication domain {domain_key:?} has no selected revisions")
            }
            Self::MissingDomain { domain_key } => {
                write!(f, "publication references missing local domain {domain_key:?}")
            }
            Self::Core(error) => write!(f, "multi-domain publication semantic error: {error}"),
            Self::Protection(error) => {
                write!(f, "multi-domain publication protection error: {error}")
            }
            Self::Wire(error) => write!(f, "multi-domain publication wire error: {error}"),
        }
    }
}

impl std::error::Error for RecoveryPublicationPrepareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Protection(error) => Some(error),
            Self::Wire(error) => Some(error),
            Self::EmptySelection | Self::EmptyRevisionSet { .. } | Self::MissingDomain { .. } => {
                None
            }
        }
    }
}

impl From<CoreError> for RecoveryPublicationPrepareError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

impl From<SyncPartError> for RecoveryPublicationPrepareError {
    fn from(value: SyncPartError) -> Self {
        Self::Protection(value)
    }
}

impl From<ProtectedPartCodecError> for RecoveryPublicationPrepareError {
    fn from(value: ProtectedPartCodecError) -> Self {
        Self::Wire(value)
    }
}

#[derive(Debug)]
pub enum RecoveryHandoffStageError<CodecError, StoreError> {
    MissingDomain { domain_key: DomainKey },
    Core(CoreError),
    RecoveryState(LocalScalarRecoveryStateError),
    Codec(CodecError),
    Sync(PersistTransitionError<StoreError>),
}

/// Build one protected transport publication from selected causal closures across
/// several independent scalar domains.
///
/// Each selected domain is first validated through the actual semantic handoff
/// rule on a throwaway clone, proving that every locally-owned member of that
/// domain's selected dependency closure is finalized. Only the exact closure of
/// the selected revisions is placed in the clear projection before protection;
/// unrelated concurrent state in the same domain is not exposed accidentally.
///
/// The resulting one-part publication is still development framing. Outbound
/// multipart packing remains a separate transport concern and this function does
/// not freeze a portable format.
pub fn prepare_recovery_handoff(
    recovery: &LocalScalarRecoveryState,
    continuum_id: ContinuumId,
    publication_id: PublicationId,
    key: &ContentKey,
    revision_ids: BTreeMap<DomainKey, BTreeSet<RevisionId>>,
) -> Result<PreparedRecoveryHandoff, RecoveryPublicationPrepareError> {
    if revision_ids.is_empty() {
        return Err(RecoveryPublicationPrepareError::EmptySelection);
    }

    let mut projected_domains = BTreeMap::new();

    for (domain_key, selected) in &revision_ids {
        if selected.is_empty() {
            return Err(RecoveryPublicationPrepareError::EmptyRevisionSet {
                domain_key: domain_key.clone(),
            });
        }

        let domain = recovery
            .restore_domain(domain_key)?
            .ok_or_else(|| RecoveryPublicationPrepareError::MissingDomain {
                domain_key: domain_key.clone(),
            })?;

        let mut eligibility = domain.clone();
        eligibility.handoff(selected.iter().copied())?;

        let closure = dependency_closure(domain.causal(), selected)?;
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
        projected_domains.insert(domain_key.clone(), register);
    }

    let projection: ScalarSyncProjection = SyncProjection::from_domains(projected_domains);
    let part = protect_scalar_part(key, continuum_id, publication_id, 0, 1, &projection)?;
    let wire = encode_protected_sync_part(&part)?;

    Ok(PreparedRecoveryHandoff {
        revision_ids,
        publication_id,
        objects: vec![wire],
    })
}

/// Durably couple multi-domain semantic exposure with the exact protected retry
/// bytes prepared from the same selected causal closures.
///
/// The complete recovery container is cloned first. Handoff/exposure bookkeeping
/// is applied independently inside each selected merge domain; then the complete
/// container plus exact outbox bytes is committed through one durability barrier.
/// Physical co-persistence does not create cross-domain causal parents or strong
/// semantic transaction semantics.
pub fn stage_prepared_recovery_handoff<S, C>(
    recovery: &mut LocalScalarRecoveryState,
    record: &mut DurableSyncRecord,
    store: &mut S,
    codec: &C,
    prepared: PreparedRecoveryHandoff,
) -> Result<(), RecoveryHandoffStageError<C::Error, S::Error>>
where
    S: SyncRecordStore,
    C: TrustedStateCodec<LocalScalarRecoveryState>,
{
    let mut candidate = recovery.clone();

    for (domain_key, selected) in &prepared.revision_ids {
        let mut domain = candidate
            .restore_domain(domain_key)
            .map_err(RecoveryHandoffStageError::Core)?
            .ok_or_else(|| RecoveryHandoffStageError::MissingDomain {
                domain_key: domain_key.clone(),
            })?;
        domain
            .handoff(selected.iter().copied())
            .map_err(RecoveryHandoffStageError::Core)?;
        candidate
            .replace_domain(domain_key.clone(), domain.snapshot())
            .map_err(RecoveryHandoffStageError::RecoveryState)?;
    }

    let trusted_state = codec
        .encode(&candidate)
        .map_err(RecoveryHandoffStageError::Codec)?;

    stage_outbound(
        record,
        store,
        trusted_state,
        prepared.publication_id,
        prepared.objects,
    )
    .map_err(RecoveryHandoffStageError::Sync)?;

    *recovery = candidate;
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
    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, WorkingEpochId};
    use apc_sync::{
        decode_protected_sync_part, unprotect_scalar_part, SyncRecordStore, TransportCursor,
    };

    use crate::DevelopmentMultiScalarTrustedStateCodec;

    use super::*;

    #[derive(Default)]
    struct MemoryStore {
        committed: Option<DurableSyncRecord>,
        fail: bool,
    }

    impl SyncRecordStore for MemoryStore {
        type Error = &'static str;

        fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
            if self.fail {
                return Err("durability failure");
            }
            self.committed = Some(record.clone());
            Ok(())
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

    #[test]
    fn protected_multi_domain_projection_contains_only_selected_dependency_closures() {
        let body_key = domain("body");
        let title_key = domain("title");
        let body = finalized_domain(100, 200, 1, "body-base", "body-local");
        let title = finalized_domain(300, 400, 2, "title-base", "title-local");
        let recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (body_key.clone(), body.snapshot()),
            (title_key.clone(), title.snapshot()),
        ]))
        .unwrap();
        let selections = BTreeMap::from([
            (body_key.clone(), BTreeSet::from([rid(200)])),
            (title_key.clone(), BTreeSet::from([rid(400)])),
        ]);
        let key = ContentKey::from_bytes([0xE1; 32]);

        let prepared = prepare_recovery_handoff(&recovery, cid(1), pid(1), &key, selections).unwrap();
        assert_eq!(prepared.objects().len(), 1);

        let protected = decode_protected_sync_part(&prepared.objects()[0]).unwrap();
        let projection = unprotect_scalar_part(&key, cid(1), &protected).unwrap();
        assert_eq!(projection.len(), 2);
        assert_eq!(
            projection.get(&body_key).unwrap().revisions().map(|r| r.id).collect(),
            BTreeSet::from([rid(100), rid(200)])
        );
        assert_eq!(
            projection.get(&title_key).unwrap().revisions().map(|r| r.id).collect(),
            BTreeSet::from([rid(300), rid(400)])
        );
    }

    #[test]
    fn durable_stage_records_exposure_in_each_selected_domain_and_exact_outbox() {
        let body_key = domain("body");
        let title_key = domain("title");
        let body = finalized_domain(100, 200, 1, "body-base", "body-local");
        let title = finalized_domain(300, 400, 2, "title-base", "title-local");
        let mut recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (body_key.clone(), body.snapshot()),
            (title_key.clone(), title.snapshot()),
        ]))
        .unwrap();
        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(TransportCursor::new(b"R1".to_vec()).unwrap()),
        );
        let mut store = MemoryStore::default();
        let key = ContentKey::from_bytes([0xE2; 32]);
        let prepared = prepare_recovery_handoff(
            &recovery,
            cid(1),
            pid(7),
            &key,
            BTreeMap::from([
                (body_key.clone(), BTreeSet::from([rid(200)])),
                (title_key.clone(), BTreeSet::from([rid(400)])),
            ]),
        )
        .unwrap();
        let exact_objects = prepared.objects().to_vec();

        stage_prepared_recovery_handoff(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            prepared,
        )
        .unwrap();

        for (domain_key, revision_id) in [(&body_key, rid(200)), (&title_key, rid(400))] {
            let restored = recovery.restore_domain(domain_key).unwrap().unwrap();
            assert!(restored
                .finalization()
                .exposed_local_ids()
                .contains(&revision_id));
            assert!(restored
                .finalization()
                .handed_off_local_ids()
                .contains(&revision_id));
        }
        let outbox = record.outbox().get(&pid(7)).unwrap();
        assert_eq!(outbox.objects(), exact_objects.as_slice());
        assert_eq!(store.committed.as_ref(), Some(&record));
    }

    #[test]
    fn failed_durable_stage_leaves_every_domain_and_record_unchanged() {
        let body_key = domain("body");
        let title_key = domain("title");
        let body = finalized_domain(100, 200, 1, "body-base", "body-local");
        let title = finalized_domain(300, 400, 2, "title-base", "title-local");
        let mut recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (body_key.clone(), body.snapshot()),
            (title_key.clone(), title.snapshot()),
        ]))
        .unwrap();
        let before_recovery = recovery.clone();
        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let mut record = DurableSyncRecord::new(codec.encode(&recovery).unwrap(), None);
        let before_record = record.clone();
        let mut store = MemoryStore {
            fail: true,
            ..MemoryStore::default()
        };
        let key = ContentKey::from_bytes([0xE3; 32]);
        let prepared = prepare_recovery_handoff(
            &recovery,
            cid(1),
            pid(8),
            &key,
            BTreeMap::from([
                (body_key, BTreeSet::from([rid(200)])),
                (title_key, BTreeSet::from([rid(400)])),
            ]),
        )
        .unwrap();

        assert!(stage_prepared_recovery_handoff(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            prepared,
        )
        .is_err());
        assert_eq!(recovery, before_recovery);
        assert_eq!(record, before_record);
    }
}
