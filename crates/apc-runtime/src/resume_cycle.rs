use apc_core::{LocalScalarDomain, LocalScalarSnapshot};
use apc_sync::{DurableSyncRecord, OpaqueTransport, SyncRecordStore, TransportCursorCodec};

use crate::{
    resume_single_scalar_domain, ScalarCatchUpSpec, ScalarResumeError, ScalarResumeOutboxOutcome,
    ScalarResumeReport, TrustedStateCodec,
};

/// Result of one bounded foreground resume cycle.
///
/// `initial` is always present. `post_conflict` is present only when the initial
/// pass reached a transport CAS conflict and the runtime immediately performed
/// one additional durable-cursor catch-up/reconciliation pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarResumeCycleReport<R> {
    pub initial: ScalarResumeReport<R>,
    pub post_conflict: Option<ScalarResumeReport<R>>,
}

/// Run the current scalar foreground-resume path with one hard-bounded follow-up
/// after a transport publication conflict.
///
/// This function deliberately does not spin until success. A single initial pass
/// may publish exact durable outbox bytes. If that publish loses the expected-head
/// race, one second pass catches up from the still-durable cursor, authenticates
/// and merges the winner, then lets the ordinary stale-outbox logic decide among
/// observed/reconciled, `NeedsRebase`, baseline-unavailable or another conflict.
///
/// Therefore a continuously moving transport cannot create an unbounded retry
/// loop inside one foreground lifecycle callback. Another platform/user-driven
/// sync opportunity may start a fresh bounded cycle later.
#[allow(clippy::type_complexity)]
pub fn resume_single_scalar_domain_bounded<T, S, TC, CC>(
    domain: &mut LocalScalarDomain<Vec<u8>>,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    transport: &mut T,
    catch_up_spec: ScalarCatchUpSpec<'_>,
) -> Result<
    ScalarResumeCycleReport<T::Revision>,
    ScalarResumeError<T::Error, TC::Error, S::Error, CC::Error>,
>
where
    T: OpaqueTransport,
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarSnapshot<Vec<u8>>>,
    CC: TransportCursorCodec<T::Revision>,
{
    let initial = resume_single_scalar_domain(
        domain,
        record,
        store,
        trusted_codec,
        cursor_codec,
        transport,
        catch_up_spec,
    )?;

    if !matches!(
        &initial.outbox,
        ScalarResumeOutboxOutcome::Conflict { .. }
    ) {
        return Ok(ScalarResumeCycleReport {
            initial,
            post_conflict: None,
        });
    }

    let post_conflict = resume_single_scalar_domain(
        domain,
        record,
        store,
        trusted_codec,
        cursor_codec,
        transport,
        catch_up_spec,
    )?;

    Ok(ScalarResumeCycleReport {
        initial,
        post_conflict: Some(post_conflict),
    })
}

#[cfg(test)]
mod tests {
    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, ContinuumId, RevisionId, ScalarRegister, WorkingEpochId};
    use apc_crypto::ContentKey;
    use apc_sync::{DomainKey, FetchOutcome, PublicationId, PublishOutcome, TransportCursor};

    use crate::{
        prepare_scalar_handoff, stage_prepared_scalar_handoff,
        DevelopmentScalarTrustedStateCodec, ScalarCatchUpOutcome,
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

    struct ConflictThenRemoteTransport {
        head: Revision,
        conflict_objects: Vec<Vec<u8>>,
        fetch_calls: usize,
        publish_calls: usize,
        conflict_injected: bool,
    }

    impl OpaqueTransport for ConflictThenRemoteTransport {
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
                    objects: self.conflict_objects.clone(),
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
                return Ok(PublishOutcome::Conflict {
                    current_head: Some(self.head),
                });
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
        let mut register = ScalarRegister::new();
        register.assign(rid(100), b"base".to_vec()).unwrap();
        register
    }

    #[test]
    fn publish_conflict_triggers_one_catch_up_then_stops_at_explicit_rebase_decision() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xC1; 32]);
        let semantic_key = domain_key();

        let mut local = LocalScalarDomain::from_causal(base()).unwrap();
        local.begin_epoch(wid(1), b"local".to_vec()).unwrap();
        local.seal_local(rid(200)).unwrap();
        local.finalize(rid(200)).unwrap();

        let mut record = DurableSyncRecord::new(
            trusted_codec.encode(&local.snapshot()).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        let local_prepared = prepare_scalar_handoff(
            &local,
            semantic_key.clone(),
            cid(1),
            pid(7),
            &key,
            [rid(200)],
        )
        .unwrap();
        stage_prepared_scalar_handoff(
            &mut local,
            &mut record,
            &mut store,
            &trusted_codec,
            local_prepared,
        )
        .unwrap();

        let mut remote = LocalScalarDomain::from_causal(base()).unwrap();
        remote.begin_epoch(wid(9), b"remote".to_vec()).unwrap();
        remote.seal_local(rid(900)).unwrap();
        remote.finalize(rid(900)).unwrap();
        let remote_prepared = prepare_scalar_handoff(
            &remote,
            semantic_key.clone(),
            cid(1),
            pid(90),
            &key,
            [rid(900)],
        )
        .unwrap();

        let mut transport = ConflictThenRemoteTransport {
            head: Revision(1),
            conflict_objects: remote_prepared.publication().objects().to_vec(),
            fetch_calls: 0,
            publish_calls: 0,
            conflict_injected: false,
        };

        let report = resume_single_scalar_domain_bounded(
            &mut local,
            &mut record,
            &mut store,
            &trusted_codec,
            &cursor_codec,
            &mut transport,
            ScalarCatchUpSpec::new(&key, cid(1), &semantic_key, None),
        )
        .unwrap();

        assert_eq!(
            report.initial.catch_up,
            ScalarCatchUpOutcome::UpToDate {
                head: Some(Revision(1))
            }
        );
        assert_eq!(
            report.initial.outbox,
            ScalarResumeOutboxOutcome::Conflict {
                publication_id: pid(7),
                current_head: Some(Revision(2))
            }
        );

        let post = report.post_conflict.expect("one bounded follow-up pass");
        let ScalarCatchUpOutcome::Applied {
            head,
            observed_revision_ids,
            ..
        } = post.catch_up
        else {
            panic!("expected authenticated conflict catch-up")
        };
        assert_eq!(head, Revision(2));
        assert!(observed_revision_ids.contains(&rid(900)));
        assert!(!observed_revision_ids.contains(&rid(200)));
        assert_eq!(
            post.outbox,
            ScalarResumeOutboxOutcome::NeedsRebase {
                publication_id: pid(7)
            }
        );

        assert_eq!(transport.fetch_calls, 2);
        assert_eq!(transport.publish_calls, 1);
        assert!(record.outbox().contains_key(&pid(7)));
        assert!(local.causal().revision(rid(200)).is_some());
        assert!(local.causal().revision(rid(900)).is_some());
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
        assert_eq!(store.committed.as_ref(), Some(&record));
    }
}
