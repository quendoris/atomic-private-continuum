use std::collections::BTreeSet;

use apc_core::{LocalScalarDomain, LocalScalarSnapshot};
use apc_sync::{
    commit_reconciled_outbox, publish_staged, DurableSyncRecord, OpaqueTransport, PublicationId,
    PublishOutcome, SessionCommitError, SessionIoError, SyncRecordStore, TransportCursorCodec,
};

use crate::{
    catch_up_single_scalar_domain, decode_single_scalar_domain_object, ScalarCatchUpError,
    ScalarCatchUpOutcome, ScalarCatchUpSpec, ScalarObjectDecodeError, TrustedStateCodec,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScalarResumeOutboxOutcome<R> {
    None,
    BlockedByBaseline,
    NeedsRebase {
        publication_id: PublicationId,
    },
    /// Catch-up authenticated remote scalar state that already contains the full
    /// causal closure carried by the stale pending publication. The publication
    /// is therefore reconciled without sending another transport mutation.
    ObservedAndReconciled {
        publication_id: PublicationId,
        head: R,
    },
    PublishedAndReconciled {
        publication_id: PublicationId,
        head: R,
    },
    Conflict {
        publication_id: PublicationId,
        current_head: Option<R>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarResumeReport<R> {
    pub catch_up: ScalarCatchUpOutcome<R>,
    pub outbox: ScalarResumeOutboxOutcome<R>,
}

#[derive(Debug)]
pub enum ScalarResumeError<TransportError, TrustedError, StoreError, CursorError> {
    MultiplePendingOutboxUnsupported { count: usize },
    CatchUp(ScalarCatchUpError<TransportError, TrustedError, StoreError, CursorError>),
    PendingDecode(ScalarObjectDecodeError),
    Publish(SessionIoError<TransportError, CursorError>),
    Reconcile(SessionCommitError<StoreError, CursorError>),
}

/// Execute the first foreground-resume synchronization slice for one scalar
/// merge domain and at most one durable outbound publication.
///
/// The ordering is deliberately recovery-first:
///
/// 1. fetch from the cursor paired with durable trusted state;
/// 2. authenticate/merge and durably advance that cursor if needed;
/// 3. inspect the still-durable outbox against the resulting cursor;
/// 4. if catch-up proves that a stale pending scalar publication is already
///    represented in authenticated remote causal state, retire it durably without
///    another transport mutation;
/// 5. otherwise retry exact staged bytes only when their expected cursor is still
///    current;
/// 6. on a positive transport acknowledgement, retire the outbox only through a
///    second durability barrier paired with the acknowledged head.
///
/// The lost-ack proof in step 4 is intentionally specific to the current
/// single-part pre-format scalar publication: every revision identity carried by
/// the pending dependency closure must also have arrived in authenticated remote
/// state during this exact catch-up pass. Merely finding those identities in the
/// already-local domain is not evidence of remote observation.
///
/// If catch-up makes a staged publication stale without that proof, this function
/// does not mutate or re-encrypt it. It reports `NeedsRebase`; the semantic layer
/// must decide whether the contribution is redundant or must be re-exported under
/// a fresh `PublicationId`.
///
/// More than one pending publication is intentionally rejected before network
/// I/O in this first slice. Overlapping outbox entries require an explicit
/// deterministic reconciliation policy rather than an accidental iteration order.
#[allow(clippy::type_complexity)]
pub fn resume_single_scalar_domain<T, S, TC, CC>(
    domain: &mut LocalScalarDomain<Vec<u8>>,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    transport: &mut T,
    catch_up_spec: ScalarCatchUpSpec<'_>,
) -> Result<
    ScalarResumeReport<T::Revision>,
    ScalarResumeError<T::Error, TC::Error, S::Error, CC::Error>,
>
where
    T: OpaqueTransport,
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarSnapshot<Vec<u8>>>,
    CC: TransportCursorCodec<T::Revision>,
{
    let pending_count = record.outbox().len();
    if pending_count > 1 {
        return Err(ScalarResumeError::MultiplePendingOutboxUnsupported {
            count: pending_count,
        });
    }
    let pending_publication = record.outbox().keys().next().copied();

    let catch_up = catch_up_single_scalar_domain(
        domain,
        record,
        store,
        trusted_codec,
        cursor_codec,
        transport,
        catch_up_spec,
    )
    .map_err(ScalarResumeError::CatchUp)?;

    if matches!(catch_up, ScalarCatchUpOutcome::BaselineUnavailable { .. }) {
        return Ok(ScalarResumeReport {
            catch_up,
            outbox: ScalarResumeOutboxOutcome::BlockedByBaseline,
        });
    }

    let Some(publication_id) = pending_publication else {
        return Ok(ScalarResumeReport {
            catch_up,
            outbox: ScalarResumeOutboxOutcome::None,
        });
    };

    let entry = record
        .outbox()
        .get(&publication_id)
        .expect("catch-up preserves pending outbox entries");
    if entry.expected_cursor() != record.applied_cursor() {
        if let ScalarCatchUpOutcome::Applied {
            head,
            observed_revision_ids,
            ..
        } = &catch_up
        {
            if entry.objects().len() == 1 {
                let pending = decode_single_scalar_domain_object(
                    catch_up_spec.key(),
                    catch_up_spec.continuum_id(),
                    catch_up_spec.domain_key(),
                    &entry.objects()[0],
                )
                .map_err(ScalarResumeError::PendingDecode)?;
                let pending_revision_ids: BTreeSet<_> =
                    pending.revisions().map(|revision| revision.id).collect();

                if pending_revision_ids.is_subset(observed_revision_ids) {
                    let reconciled_head = head.clone();
                    let trusted_state = record.trusted_state().to_vec();
                    commit_reconciled_outbox(
                        record,
                        store,
                        cursor_codec,
                        publication_id,
                        trusted_state,
                        &reconciled_head,
                    )
                    .map_err(ScalarResumeError::Reconcile)?;

                    return Ok(ScalarResumeReport {
                        catch_up,
                        outbox: ScalarResumeOutboxOutcome::ObservedAndReconciled {
                            publication_id,
                            head: reconciled_head,
                        },
                    });
                }
            }
        }

        return Ok(ScalarResumeReport {
            catch_up,
            outbox: ScalarResumeOutboxOutcome::NeedsRebase { publication_id },
        });
    }

    match publish_staged(record, publication_id, transport, cursor_codec)
        .map_err(ScalarResumeError::Publish)?
    {
        PublishOutcome::Published { head } => {
            let trusted_state = record.trusted_state().to_vec();
            commit_reconciled_outbox(
                record,
                store,
                cursor_codec,
                publication_id,
                trusted_state,
                &head,
            )
            .map_err(ScalarResumeError::Reconcile)?;

            Ok(ScalarResumeReport {
                catch_up,
                outbox: ScalarResumeOutboxOutcome::PublishedAndReconciled {
                    publication_id,
                    head,
                },
            })
        }
        PublishOutcome::Conflict { current_head } => Ok(ScalarResumeReport {
            catch_up,
            outbox: ScalarResumeOutboxOutcome::Conflict {
                publication_id,
                current_head,
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, ContinuumId, RevisionId, ScalarRegister, WorkingEpochId};
    use apc_crypto::ContentKey;
    use apc_sync::{stage_outbound, DomainKey, FetchOutcome, SyncRecordStore, TransportCursor};

    use crate::{
        prepare_scalar_handoff, stage_prepared_scalar_handoff,
        DevelopmentScalarTrustedStateCodec,
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
            _objects: &[Vec<u8>],
        ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
            self.publish_calls += 1;
            if expected_head != Some(&self.head) {
                return Ok(PublishOutcome::Conflict {
                    current_head: Some(self.head),
                });
            }
            self.head = Revision(self.head.0 + 1);
            Ok(PublishOutcome::Published { head: self.head })
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

    fn domain() -> LocalScalarDomain<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), b"base".to_vec()).unwrap();
        LocalScalarDomain::from_causal(causal).unwrap()
    }

    #[test]
    fn resume_retries_current_exact_outbox_then_retires_it_durably() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let mut domain = domain();
        let mut record = DurableSyncRecord::new(
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        stage_outbound(
            &mut record,
            &mut store,
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            pid(7),
            vec![b"exact-protected-wire".to_vec()],
        )
        .unwrap();
        let mut transport = ResumeTransport {
            head: Revision(1),
            fetched_objects: Vec::new(),
            fetch_calls: 0,
            publish_calls: 0,
        };
        let key = ContentKey::from_bytes([0xA1; 32]);
        let semantic_key = domain_key();

        let report = resume_single_scalar_domain(
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
            report.catch_up,
            ScalarCatchUpOutcome::UpToDate {
                head: Some(Revision(1))
            }
        );
        assert_eq!(
            report.outbox,
            ScalarResumeOutboxOutcome::PublishedAndReconciled {
                publication_id: pid(7),
                head: Revision(2)
            }
        );
        assert!(record.outbox().is_empty());
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
        assert_eq!(transport.fetch_calls, 1);
        assert_eq!(transport.publish_calls, 1);
        assert_eq!(store.committed.as_ref(), Some(&record));
    }

    #[test]
    fn catch_up_that_advances_cursor_marks_old_outbox_for_rebase_without_publish() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let mut domain = domain();
        let mut record = DurableSyncRecord::new(
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        stage_outbound(
            &mut record,
            &mut store,
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            pid(8),
            vec![b"stale-exact-wire".to_vec()],
        )
        .unwrap();
        let mut transport = ResumeTransport {
            head: Revision(2),
            fetched_objects: Vec::new(),
            fetch_calls: 0,
            publish_calls: 0,
        };
        let key = ContentKey::from_bytes([0xA2; 32]);
        let semantic_key = domain_key();

        let report = resume_single_scalar_domain(
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
            report.catch_up,
            ScalarCatchUpOutcome::CursorAdvancedWithoutSemanticObjects { head: Revision(2) }
        );
        assert_eq!(
            report.outbox,
            ScalarResumeOutboxOutcome::NeedsRebase {
                publication_id: pid(8)
            }
        );
        assert!(record.outbox().contains_key(&pid(8)));
        assert_eq!(transport.fetch_calls, 1);
        assert_eq!(transport.publish_calls, 0);
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
    }

    #[test]
    fn lost_ack_is_reconciled_from_authenticated_catch_up_without_second_publish() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xA3; 32]);
        let semantic_key = domain_key();
        let mut domain = domain();
        domain.begin_epoch(wid(1), b"local-exposed".to_vec()).unwrap();
        domain.seal_local(rid(200)).unwrap();
        domain.finalize(rid(200)).unwrap();

        let mut record = DurableSyncRecord::new(
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        let prepared = prepare_scalar_handoff(
            &domain,
            semantic_key.clone(),
            cid(1),
            pid(9),
            &key,
            [rid(200)],
        )
        .unwrap();
        let remotely_accepted_objects = prepared.publication().objects().to_vec();
        stage_prepared_scalar_handoff(
            &mut domain,
            &mut record,
            &mut store,
            &trusted_codec,
            prepared,
        )
        .unwrap();

        // The transport accepted the exact protected publication and advanced to
        // R2, but the client died before its acknowledgement could retire pid(9).
        let mut transport = ResumeTransport {
            head: Revision(2),
            fetched_objects: remotely_accepted_objects,
            fetch_calls: 0,
            publish_calls: 0,
        };

        let report = resume_single_scalar_domain(
            &mut domain,
            &mut record,
            &mut store,
            &trusted_codec,
            &cursor_codec,
            &mut transport,
            ScalarCatchUpSpec::new(&key, cid(1), &semantic_key, None),
        )
        .unwrap();

        let ScalarCatchUpOutcome::Applied {
            head,
            observed_revision_ids,
            ..
        } = &report.catch_up
        else {
            panic!("expected authenticated catch-up")
        };
        assert_eq!(*head, Revision(2));
        assert_eq!(observed_revision_ids, &BTreeSet::from([rid(100), rid(200)]));
        assert_eq!(
            report.outbox,
            ScalarResumeOutboxOutcome::ObservedAndReconciled {
                publication_id: pid(9),
                head: Revision(2)
            }
        );
        assert!(record.outbox().is_empty());
        assert_eq!(transport.fetch_calls, 1);
        assert_eq!(transport.publish_calls, 0);
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
        assert_eq!(store.committed.as_ref(), Some(&record));
    }
}
