use std::collections::BTreeSet;

use apc_core::{LocalScalarDomain, LocalScalarSnapshot};
use apc_sync::{
    commit_reconciled_outbox_batch, publish_staged_batch, BatchCommitError, BatchPublishError,
    DurableSyncRecord, OpaqueTransport, PublicationId, PublishOutcome, SyncRecordStore,
    TransportCursorCodec,
};

use crate::{
    catch_up_single_scalar_domain, decode_single_scalar_domain_object, ScalarCatchUpError,
    ScalarCatchUpOutcome, ScalarCatchUpSpec, ScalarObjectDecodeError, TrustedStateCodec,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScalarBatchResumeAction<R> {
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
pub struct ScalarBatchResumeReport<R> {
    pub catch_up: ScalarCatchUpOutcome<R>,
    /// Stale publications whose complete scalar causal closures were proven to
    /// have arrived in authenticated remote state during this exact catch-up and
    /// were therefore retired durably without another transport mutation.
    pub observed_reconciled: BTreeSet<PublicationId>,
    pub action: ScalarBatchResumeAction<R>,
}

#[derive(Debug)]
pub enum ScalarBatchResumeError<TransportError, TrustedError, StoreError, CursorError> {
    CatchUp(ScalarCatchUpError<TransportError, TrustedError, StoreError, CursorError>),
    PendingDecode {
        publication_id: PublicationId,
        error: ScalarObjectDecodeError,
    },
    Publish(BatchPublishError<TransportError, CursorError>),
    Reconcile(BatchCommitError<StoreError, CursorError>),
}

pub type ScalarBatchResumeResult<
    R,
    TransportError,
    TrustedError,
    StoreError,
    CursorError,
> = Result<
    ScalarBatchResumeReport<R>,
    ScalarBatchResumeError<TransportError, TrustedError, StoreError, CursorError>,
>;

/// Resume one scalar runtime recovery record containing any number of pending
/// publications without deriving retry priority from `PublicationId` order.
///
/// The outbox is treated as sets classified only by recovery facts:
///
/// - entries whose `expected_cursor` equals the durable cursor after catch-up are
///   one current transport cursor class and may be coalesced into one exact-byte
///   batch publication;
/// - entries targeting any other cursor are stale as a set. Authenticated catch-up
///   evidence may reconcile members of that set; any unresolved stale member stops
///   the cycle at an explicit `NeedsRebase` result before newer/current entries are
///   published.
///
/// The function never compares transport cursor bytes and never interprets the
/// canonical order of publication IDs as time or priority.
pub fn resume_scalar_outbox_set<T, S, TC, CC>(
    domain: &mut LocalScalarDomain<Vec<u8>>,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    transport: &mut T,
    catch_up_spec: ScalarCatchUpSpec<'_>,
) -> ScalarBatchResumeResult<T::Revision, T::Error, TC::Error, S::Error, CC::Error>
where
    T: OpaqueTransport,
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarSnapshot<Vec<u8>>>,
    CC: TransportCursorCodec<T::Revision>,
{
    let initial_pending: BTreeSet<PublicationId> = record.outbox().keys().copied().collect();

    let catch_up = catch_up_single_scalar_domain(
        domain,
        record,
        store,
        trusted_codec,
        cursor_codec,
        transport,
        catch_up_spec,
    )
    .map_err(ScalarBatchResumeError::CatchUp)?;

    if matches!(catch_up, ScalarCatchUpOutcome::BaselineUnavailable { .. }) {
        return Ok(ScalarBatchResumeReport {
            catch_up,
            observed_reconciled: BTreeSet::new(),
            action: ScalarBatchResumeAction::BlockedByBaseline,
        });
    }

    if initial_pending.is_empty() {
        return Ok(ScalarBatchResumeReport {
            catch_up,
            observed_reconciled: BTreeSet::new(),
            action: ScalarBatchResumeAction::None,
        });
    }

    let current_cursor = record.applied_cursor().cloned();
    let mut current = BTreeSet::new();
    let mut stale = BTreeSet::new();

    for publication_id in initial_pending {
        let entry = record
            .outbox()
            .get(&publication_id)
            .expect("catch-up preserves pending outbox entries");
        if entry.expected_cursor() == current_cursor.as_ref() {
            current.insert(publication_id);
        } else {
            stale.insert(publication_id);
        }
    }

    let mut observed_reconciled = BTreeSet::new();

    if !stale.is_empty() {
        if let ScalarCatchUpOutcome::Applied {
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

                // The current scalar runtime can prove rediscovery only for one
                // complete single-part scalar object. Unknown/multipart shapes are
                // simply not proof and remain stale for higher-level reconciliation.
                if entry.objects().len() != 1 {
                    continue;
                }

                let pending = decode_single_scalar_domain_object(
                    catch_up_spec.key(),
                    catch_up_spec.continuum_id(),
                    catch_up_spec.domain_key(),
                    &entry.objects()[0],
                )
                .map_err(|error| ScalarBatchResumeError::PendingDecode {
                    publication_id: *publication_id,
                    error,
                })?;
                let pending_revision_ids: BTreeSet<_> =
                    pending.revisions().map(|revision| revision.id).collect();

                if pending_revision_ids.is_subset(observed_revision_ids) {
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
                .map_err(ScalarBatchResumeError::Reconcile)?;

                stale.retain(|publication_id| !observed_reconciled.contains(publication_id));
            }
        }

        if !stale.is_empty() {
            return Ok(ScalarBatchResumeReport {
                catch_up,
                observed_reconciled,
                action: ScalarBatchResumeAction::NeedsRebase {
                    publication_ids: stale,
                },
            });
        }
    }

    if current.is_empty() {
        return Ok(ScalarBatchResumeReport {
            catch_up,
            observed_reconciled,
            action: ScalarBatchResumeAction::None,
        });
    }

    match publish_staged_batch(record, current.iter().copied(), transport, cursor_codec)
        .map_err(ScalarBatchResumeError::Publish)?
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
            .map_err(ScalarBatchResumeError::Reconcile)?;

            Ok(ScalarBatchResumeReport {
                catch_up,
                observed_reconciled,
                action: ScalarBatchResumeAction::PublishedAndReconciled {
                    publication_ids: current,
                    head,
                },
            })
        }
        PublishOutcome::Conflict { current_head } => Ok(ScalarBatchResumeReport {
            catch_up,
            observed_reconciled,
            action: ScalarBatchResumeAction::Conflict {
                publication_ids: current,
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
    use apc_sync::{
        stage_outbound, DomainKey, FetchOutcome, SyncRecordStore, TransportCursor,
    };

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

    struct BatchResumeTransport {
        head: Revision,
        fetched_objects: Vec<Vec<u8>>,
        fetch_calls: usize,
        publish_calls: usize,
        published_objects: Vec<Vec<u8>>,
    }

    impl OpaqueTransport for BatchResumeTransport {
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

    fn domain() -> LocalScalarDomain<Vec<u8>> {
        LocalScalarDomain::from_causal(base()).unwrap()
    }

    #[test]
    fn same_cursor_pending_set_is_published_once_without_id_priority() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xD1; 32]);
        let semantic_key = domain_key();
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

        let mut transport = BatchResumeTransport {
            head: Revision(1),
            fetched_objects: Vec::new(),
            fetch_calls: 0,
            publish_calls: 0,
            published_objects: Vec::new(),
        };

        let report = resume_scalar_outbox_set(
            &mut domain,
            &mut record,
            &mut store,
            &trusted_codec,
            &cursor_codec,
            &mut transport,
            ScalarCatchUpSpec::new(&key, cid(1), &semantic_key, None),
        )
        .unwrap();

        assert!(report.observed_reconciled.is_empty());
        assert_eq!(
            report.action,
            ScalarBatchResumeAction::PublishedAndReconciled {
                publication_ids: BTreeSet::from([pid(1), pid(2)]),
                head: Revision(2)
            }
        );
        assert_eq!(transport.fetch_calls, 1);
        assert_eq!(transport.publish_calls, 1);
        assert_eq!(
            transport.published_objects,
            vec![b"wire-1".to_vec(), b"wire-2".to_vec()]
        );
        assert!(record.outbox().is_empty());
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
    }

    #[test]
    fn unresolved_stale_set_blocks_current_cursor_set_without_id_ordering() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xD2; 32]);
        let semantic_key = domain_key();
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
            pid(1),
            vec![b"old-wire".to_vec()],
        )
        .unwrap();
        record.apply_received(
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            cursor_codec.encode(&Revision(2)).unwrap(),
        );
        stage_outbound(
            &mut record,
            &mut store,
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            pid(2),
            vec![b"current-wire".to_vec()],
        )
        .unwrap();

        let mut transport = BatchResumeTransport {
            head: Revision(2),
            fetched_objects: Vec::new(),
            fetch_calls: 0,
            publish_calls: 0,
            published_objects: Vec::new(),
        };

        let report = resume_scalar_outbox_set(
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
            report.action,
            ScalarBatchResumeAction::NeedsRebase {
                publication_ids: BTreeSet::from([pid(1)])
            }
        );
        assert_eq!(transport.publish_calls, 0);
        assert!(record.outbox().contains_key(&pid(1)));
        assert!(record.outbox().contains_key(&pid(2)));
    }

    #[test]
    fn authenticated_catch_up_reconciles_entire_stale_lost_ack_set() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xD3; 32]);
        let semantic_key = domain_key();
        let mut domain = domain();
        let mut record = DurableSyncRecord::new(
            trusted_codec.encode(&domain.snapshot()).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();

        domain.begin_epoch(wid(1), b"local-1".to_vec()).unwrap();
        domain.seal_local(rid(200)).unwrap();
        domain.finalize(rid(200)).unwrap();
        let prepared_1 = prepare_scalar_handoff(
            &domain,
            semantic_key.clone(),
            cid(1),
            pid(1),
            &key,
            [rid(200)],
        )
        .unwrap();
        let wire_1 = prepared_1.publication().objects()[0].clone();
        stage_prepared_scalar_handoff(
            &mut domain,
            &mut record,
            &mut store,
            &trusted_codec,
            prepared_1,
        )
        .unwrap();

        domain.begin_epoch(wid(2), b"local-2".to_vec()).unwrap();
        domain.seal_local(rid(300)).unwrap();
        domain.finalize(rid(300)).unwrap();
        let prepared_2 = prepare_scalar_handoff(
            &domain,
            semantic_key.clone(),
            cid(1),
            pid(2),
            &key,
            [rid(300)],
        )
        .unwrap();
        let wire_2 = prepared_2.publication().objects()[0].clone();
        stage_prepared_scalar_handoff(
            &mut domain,
            &mut record,
            &mut store,
            &trusted_codec,
            prepared_2,
        )
        .unwrap();

        let mut transport = BatchResumeTransport {
            head: Revision(2),
            fetched_objects: vec![wire_1, wire_2],
            fetch_calls: 0,
            publish_calls: 0,
            published_objects: Vec::new(),
        };

        let report = resume_scalar_outbox_set(
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
            report.observed_reconciled,
            BTreeSet::from([pid(1), pid(2)])
        );
        assert_eq!(report.action, ScalarBatchResumeAction::None);
        assert_eq!(transport.fetch_calls, 1);
        assert_eq!(transport.publish_calls, 0);
        assert!(record.outbox().is_empty());
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
        assert!(domain.causal().revision(rid(200)).is_some());
        assert!(domain.causal().revision(rid(300)).is_some());
    }
}
