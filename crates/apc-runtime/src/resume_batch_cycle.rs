use apc_core::{LocalScalarDomain, LocalScalarSnapshot};
use apc_sync::{DurableSyncRecord, OpaqueTransport, SyncRecordStore, TransportCursorCodec};

use crate::{
    resume_scalar_outbox_set, ScalarBatchResumeAction, ScalarBatchResumeError,
    ScalarBatchResumeReport, ScalarCatchUpSpec, TrustedStateCodec,
};

/// Result of one hard-bounded foreground resume cycle for a scalar outbox set.
///
/// `post_conflict` exists only when the first pass attempted one equal-cursor
/// publication set and lost the transport compare-and-swap race. The second pass
/// catches up once from the still-durable cursor and then stops at whatever
/// explicit state the set-based resume logic reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarBatchResumeCycleReport<R> {
    pub initial: ScalarBatchResumeReport<R>,
    pub post_conflict: Option<ScalarBatchResumeReport<R>>,
}

/// Run at most two set-based scalar resume passes in one foreground cycle.
///
/// The first pass may publish one set of exact durable bytes whose members share
/// the current expected transport cursor. If that single transport mutation
/// conflicts, exactly one additional pass is allowed to fetch/authenticate the
/// winner and reclassify all still-pending publications from durable facts.
///
/// There is deliberately no `while conflict` loop. A moving remote head or a
/// stale publication set cannot create unbounded work inside one platform resume
/// callback, and `PublicationId` order is never used to choose retry priority.
#[allow(clippy::type_complexity)]
pub fn resume_scalar_outbox_set_bounded<T, S, TC, CC>(
    domain: &mut LocalScalarDomain<Vec<u8>>,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    transport: &mut T,
    catch_up_spec: ScalarCatchUpSpec<'_>,
) -> Result<
    ScalarBatchResumeCycleReport<T::Revision>,
    ScalarBatchResumeError<T::Error, TC::Error, S::Error, CC::Error>,
>
where
    T: OpaqueTransport,
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarSnapshot<Vec<u8>>>,
    CC: TransportCursorCodec<T::Revision>,
{
    let initial = resume_scalar_outbox_set(
        domain,
        record,
        store,
        trusted_codec,
        cursor_codec,
        transport,
        catch_up_spec,
    )?;

    if !matches!(&initial.action, ScalarBatchResumeAction::Conflict { .. }) {
        return Ok(ScalarBatchResumeCycleReport {
            initial,
            post_conflict: None,
        });
    }

    let post_conflict = resume_scalar_outbox_set(
        domain,
        record,
        store,
        trusted_codec,
        cursor_codec,
        transport,
        catch_up_spec,
    )?;

    Ok(ScalarBatchResumeCycleReport {
        initial,
        post_conflict: Some(post_conflict),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, ContinuumId, ScalarRegister};
    use apc_crypto::ContentKey;
    use apc_sync::{
        stage_outbound, DomainKey, FetchOutcome, PublicationId, PublishOutcome, TransportCursor,
    };

    use crate::{DevelopmentScalarTrustedStateCodec, ScalarCatchUpOutcome};

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

    fn domain() -> LocalScalarDomain<Vec<u8>> {
        let mut register = ScalarRegister::new();
        register.assign(apc_core::RevisionId::from_bytes(bytes(100)), b"base".to_vec()).unwrap();
        LocalScalarDomain::from_causal(register).unwrap()
    }

    #[test]
    fn batch_conflict_runs_one_catch_up_then_stops_at_set_rebase_boundary() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xE1; 32]);
        let semantic_key = domain_key();
        let mut domain = domain();
        let mut record = DurableSyncRecord::new(
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();

        // Intentionally stage in reverse ID order. The bounded runtime must treat
        // these as one equal-cursor set, not as an ordered retry queue.
        stage_outbound(
            &mut record,
            &mut store,
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            pid(2),
            vec![b"wire-2".to_vec()],
        )
        .unwrap();
        stage_outbound(
            &mut record,
            &mut store,
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            pid(1),
            vec![b"wire-1".to_vec()],
        )
        .unwrap();

        let mut transport = ConflictOnceTransport {
            head: Revision(1),
            fetch_calls: 0,
            publish_calls: 0,
            conflict_injected: false,
        };

        let report = resume_scalar_outbox_set_bounded(
            &mut domain,
            &mut record,
            &mut store,
            &trusted_codec,
            &cursor_codec,
            &mut transport,
            ScalarCatchUpSpec::new(&key, cid(1), &semantic_key, None),
        )
        .unwrap();

        assert_eq!(
            report.initial.action,
            ScalarBatchResumeAction::Conflict {
                publication_ids: BTreeSet::from([pid(1), pid(2)]),
                current_head: Some(Revision(2))
            }
        );

        let post = report.post_conflict.expect("exactly one follow-up pass");
        assert_eq!(
            post.catch_up,
            ScalarCatchUpOutcome::CursorAdvancedWithoutSemanticObjects { head: Revision(2) }
        );
        assert_eq!(
            post.action,
            ScalarBatchResumeAction::NeedsRebase {
                publication_ids: BTreeSet::from([pid(1), pid(2)])
            }
        );
        assert_eq!(transport.fetch_calls, 2);
        assert_eq!(transport.publish_calls, 1);
        assert_eq!(record.outbox().len(), 2);
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
        assert_eq!(store.committed.as_ref(), Some(&record));
    }
}
