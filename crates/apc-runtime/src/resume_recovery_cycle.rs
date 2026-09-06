use std::collections::BTreeMap;

use apc_core::{ContinuumId, RevisionId};
use apc_crypto::ContentKey;
use apc_sync::{
    DomainKey, DurableSyncRecord, OpaqueTransport, SyncRecordStore, TransportCursorCodec,
};

use crate::{
    resume_scalar_recovery_outbox_set, LocalScalarRecoveryState, ScalarRecoveryResumeAction,
    ScalarRecoveryResumeError, ScalarRecoveryResumeReport, ScalarRecoveryResumeSpec,
    TrustedStateCodec,
};

/// Result of one hard-bounded foreground resume cycle for the complete
/// multi-domain scalar recovery state.
///
/// `post_conflict` exists only when the first pass attempted one equal-cursor
/// publication set and lost the transport compare-and-swap race.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarRecoveryResumeCycleReport<R> {
    pub initial: ScalarRecoveryResumeReport<R>,
    pub post_conflict: Option<ScalarRecoveryResumeReport<R>>,
}

/// Run at most two multi-domain recovery resume passes in one foreground cycle.
///
/// The first pass may perform one exact-byte batch publish. If that mutation
/// conflicts, exactly one additional pass may fetch/authenticate the competing
/// transport state and reclassify the still-durable outbox from facts. There is
/// deliberately no conflict loop and no publication-ID priority.
#[allow(clippy::type_complexity)]
pub fn resume_scalar_recovery_outbox_set_bounded<T, S, TC, CC>(
    recovery: &mut LocalScalarRecoveryState,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    transport: &mut T,
    key: &ContentKey,
    continuum_id: ContinuumId,
    pre_observation_revision_ids: &BTreeMap<DomainKey, RevisionId>,
) -> Result<
    ScalarRecoveryResumeCycleReport<T::Revision>,
    ScalarRecoveryResumeError<T::Error, TC::Error, S::Error, CC::Error>,
>
where
    T: OpaqueTransport,
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarRecoveryState>,
    CC: TransportCursorCodec<T::Revision>,
{
    let initial = resume_scalar_recovery_outbox_set(
        recovery,
        record,
        store,
        trusted_codec,
        cursor_codec,
        transport,
        ScalarRecoveryResumeSpec::new(key, continuum_id, pre_observation_revision_ids),
    )?;

    if !matches!(&initial.action, ScalarRecoveryResumeAction::Conflict { .. }) {
        return Ok(ScalarRecoveryResumeCycleReport {
            initial,
            post_conflict: None,
        });
    }

    let post_conflict = resume_scalar_recovery_outbox_set(
        recovery,
        record,
        store,
        trusted_codec,
        cursor_codec,
        transport,
        ScalarRecoveryResumeSpec::new(key, continuum_id, pre_observation_revision_ids),
    )?;

    Ok(ScalarRecoveryResumeCycleReport {
        initial,
        post_conflict: Some(post_conflict),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{
        AtomId, ContinuumId, LocalScalarDomain, RevisionId, ScalarRegister, WorkingEpochId,
    };
    use apc_crypto::ContentKey;
    use apc_sync::{
        FetchOutcome, PublicationId, PublishOutcome, SyncRecordStore, TransportCursor,
    };

    use crate::{
        prepare_recovery_handoff, stage_prepared_recovery_handoff,
        DevelopmentMultiScalarTrustedStateCodec, ScalarRecoveryCatchUpOutcome,
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

    struct ConflictOnceTransport {
        head: Revision,
        fetch_calls: usize,
        publish_calls: usize,
        conflict_injected: bool,
    }

    impl OpaqueTransport for ConflictOnceTransport {
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
                    objects: Vec::new(),
                })
            }
        }

        fn publish(
            &mut self,
            expected_head: Option<&Self::Revision>,
            _objects: &[Vec<u8>],
        ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
            self.publish_calls += 1;
            if !self.conflict_injected && expected_head == Some(&self.head) {
                self.conflict_injected = true;
                self.head = Revision(self.head.0 + 1);
            }
            Ok(PublishOutcome::Conflict {
                current_head: Some(self.head),
            })
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
    fn multi_domain_conflict_runs_one_follow_up_then_stops_at_rebase_boundary() {
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
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xF6; 32]);
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
            BTreeMap::from([
                (body_key.clone(), BTreeSet::from([rid(200)])),
                (title_key.clone(), BTreeSet::from([rid(400)])),
            ]),
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

        let mut transport = ConflictOnceTransport {
            head: Revision(1),
            fetch_calls: 0,
            publish_calls: 0,
            conflict_injected: false,
        };
        let pre_observation = BTreeMap::new();

        let report = resume_scalar_recovery_outbox_set_bounded(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            &mut transport,
            &key,
            cid(1),
            &pre_observation,
        )
        .unwrap();

        assert_eq!(
            report.initial.action,
            ScalarRecoveryResumeAction::Conflict {
                publication_ids: BTreeSet::from([pid(7)]),
                current_head: Some(Revision(2)),
            }
        );
        let post = report.post_conflict.expect("exactly one follow-up pass");
        assert_eq!(
            post.catch_up,
            ScalarRecoveryCatchUpOutcome::CursorAdvancedWithoutSemanticObjects { head: Revision(2) }
        );
        assert_eq!(
            post.action,
            ScalarRecoveryResumeAction::NeedsRebase {
                publication_ids: BTreeSet::from([pid(7)]),
            }
        );
        assert_eq!(transport.fetch_calls, 2);
        assert_eq!(transport.publish_calls, 1);
        assert!(record.outbox().contains_key(&pid(7)));
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
    }
}
