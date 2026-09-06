use std::collections::BTreeSet;

use crate::{
    DurableSyncRecord, OpaqueTransport, PublicationId, PublishOutcome, SyncRecordStore,
    SyncRecoveryError, TransportCursor, TransportCursorCodec,
};

/// Failure while publishing a set of already-staged durable publications as one
/// transport mutation.
#[derive(Debug)]
pub enum BatchPublishError<TransportError, CursorError> {
    EmptyBatch,
    Recovery(SyncRecoveryError),
    MixedExpectedCursor,
    Cursor(CursorError),
    Transport(TransportError),
}

/// Failure while durably reconciling a set of outbound publications together.
#[derive(Debug)]
pub enum BatchCommitError<StoreError, CursorError> {
    EmptyBatch,
    Recovery(SyncRecoveryError),
    Cursor(CursorError),
    Store(StoreError),
}

/// Publish a non-empty set of staged publications that all target the same
/// durable transport cursor.
///
/// No publication is selected as "first" for retry policy. The supplied IDs are
/// treated as a mathematical set. Their canonical byte order is used only to
/// produce deterministic transport input; it has no temporal, causal or priority
/// meaning.
///
/// Every protected object is reused byte-for-byte from durable outbox state. If
/// any selected publication is unknown or targets a different expected cursor,
/// the function fails before transport I/O.
pub fn publish_staged_batch<T, C, I>(
    record: &DurableSyncRecord,
    publication_ids: I,
    transport: &mut T,
    codec: &C,
) -> Result<PublishOutcome<T::Revision>, BatchPublishError<T::Error, C::Error>>
where
    T: OpaqueTransport,
    C: TransportCursorCodec<T::Revision>,
    I: IntoIterator<Item = PublicationId>,
{
    let publication_ids: BTreeSet<PublicationId> = publication_ids.into_iter().collect();
    if publication_ids.is_empty() {
        return Err(BatchPublishError::EmptyBatch);
    }

    let mut common_expected: Option<Option<TransportCursor>> = None;
    let mut objects = Vec::new();

    for publication_id in publication_ids {
        let entry = record
            .outbox()
            .get(&publication_id)
            .ok_or(BatchPublishError::Recovery(
                SyncRecoveryError::UnknownOutboxPublication,
            ))?;
        let entry_expected = entry.expected_cursor().cloned();

        match &common_expected {
            None => common_expected = Some(entry_expected.clone()),
            Some(expected) if expected != &entry_expected => {
                return Err(BatchPublishError::MixedExpectedCursor);
            }
            Some(_) => {}
        }

        objects.extend(entry.objects().iter().cloned());
    }

    let expected_cursor = common_expected.expect("non-empty publication set has a cursor class");
    let expected = expected_cursor
        .as_ref()
        .map(|cursor| codec.decode(cursor))
        .transpose()
        .map_err(BatchPublishError::Cursor)?;

    transport
        .publish(expected.as_ref(), &objects)
        .map_err(BatchPublishError::Transport)
}

/// Durably retire a non-empty set of outbound publications after one observed
/// transport outcome has reconciled all members of that set.
///
/// The whole transition is clone-persist-swap. An unknown publication, cursor
/// encoding failure or durability failure therefore leaves the caller's in-memory
/// recovery record unchanged. Other outbox entries are preserved.
pub fn commit_reconciled_outbox_batch<S, C, R, I>(
    record: &mut DurableSyncRecord,
    store: &mut S,
    codec: &C,
    publication_ids: I,
    reconciled_trusted_state: Vec<u8>,
    new_head: &R,
) -> Result<(), BatchCommitError<S::Error, C::Error>>
where
    S: SyncRecordStore,
    C: TransportCursorCodec<R>,
    I: IntoIterator<Item = PublicationId>,
{
    let publication_ids: BTreeSet<PublicationId> = publication_ids.into_iter().collect();
    if publication_ids.is_empty() {
        return Err(BatchCommitError::EmptyBatch);
    }

    let cursor = codec.encode(new_head).map_err(BatchCommitError::Cursor)?;
    let mut next = record.clone();

    for publication_id in publication_ids {
        next.retire_outbox(
            publication_id,
            reconciled_trusted_state.clone(),
            cursor.clone(),
        )
        .map_err(BatchCommitError::Recovery)?;
    }

    store.persist(&next).map_err(BatchCommitError::Store)?;
    *record = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FetchOutcome;

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
        fail: bool,
    }

    impl SyncRecordStore for MemoryStore {
        type Error = &'static str;

        fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
            if self.fail {
                return Err("injected durability failure");
            }
            self.committed = Some(record.clone());
            Ok(())
        }
    }

    struct MemoryTransport {
        head: Revision,
        publish_calls: usize,
        published_objects: Vec<Vec<u8>>,
    }

    impl OpaqueTransport for MemoryTransport {
        type Revision = Revision;
        type Error = &'static str;

        fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
            Ok(Some(self.head))
        }

        fn fetch_since(
            &mut self,
            _known_head: Option<&Self::Revision>,
        ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
            Ok(FetchOutcome::UpToDate {
                head: Some(self.head),
            })
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

    fn pid(value: u64) -> PublicationId {
        let mut bytes = [0_u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        PublicationId::from_bytes(bytes)
    }

    fn cursor(value: u64) -> TransportCursor {
        RevisionCodec.encode(&Revision(value)).unwrap()
    }

    fn two_at_same_cursor() -> DurableSyncRecord {
        let mut record = DurableSyncRecord::new(b"base".to_vec(), Some(cursor(1)));
        record
            .prepare_outbox(
                b"exposed-1".to_vec(),
                pid(1),
                Some(cursor(1)),
                vec![b"wire-1".to_vec()],
            )
            .unwrap();
        record
            .prepare_outbox(
                b"exposed-2".to_vec(),
                pid(2),
                Some(cursor(1)),
                vec![b"wire-2".to_vec()],
            )
            .unwrap();
        record
    }

    #[test]
    fn same_cursor_set_publishes_once_without_publication_priority() {
        let record = two_at_same_cursor();
        let before = record.clone();
        let mut transport = MemoryTransport {
            head: Revision(1),
            publish_calls: 0,
            published_objects: Vec::new(),
        };

        let outcome = publish_staged_batch(
            &record,
            [pid(2), pid(1)],
            &mut transport,
            &RevisionCodec,
        )
        .unwrap();

        assert_eq!(outcome, PublishOutcome::Published { head: Revision(2) });
        assert_eq!(transport.publish_calls, 1);
        assert_eq!(
            transport.published_objects,
            vec![b"wire-1".to_vec(), b"wire-2".to_vec()]
        );
        assert_eq!(record, before);
        assert_eq!(record.outbox().len(), 2);
    }

    #[test]
    fn mixed_cursor_set_fails_before_transport_io() {
        let mut record = DurableSyncRecord::new(b"base".to_vec(), Some(cursor(1)));
        record
            .prepare_outbox(
                b"exposed-old".to_vec(),
                pid(1),
                Some(cursor(1)),
                vec![b"wire-old".to_vec()],
            )
            .unwrap();
        record.apply_received(b"merged".to_vec(), cursor(2));
        record
            .prepare_outbox(
                b"exposed-new".to_vec(),
                pid(2),
                Some(cursor(2)),
                vec![b"wire-new".to_vec()],
            )
            .unwrap();

        let mut transport = MemoryTransport {
            head: Revision(2),
            publish_calls: 0,
            published_objects: Vec::new(),
        };

        let error = publish_staged_batch(
            &record,
            [pid(1), pid(2)],
            &mut transport,
            &RevisionCodec,
        )
        .unwrap_err();

        assert!(matches!(error, BatchPublishError::MixedExpectedCursor));
        assert_eq!(transport.publish_calls, 0);
        assert_eq!(record.outbox().len(), 2);
    }

    #[test]
    fn reconciled_set_retires_atomically_and_preserves_other_entries() {
        let mut record = two_at_same_cursor();
        record
            .prepare_outbox(
                b"exposed-3".to_vec(),
                pid(3),
                Some(cursor(1)),
                vec![b"wire-3".to_vec()],
            )
            .unwrap();
        let mut store = MemoryStore::default();

        commit_reconciled_outbox_batch(
            &mut record,
            &mut store,
            &RevisionCodec,
            [pid(2), pid(1)],
            b"reconciled".to_vec(),
            &Revision(2),
        )
        .unwrap();

        assert!(!record.outbox().contains_key(&pid(1)));
        assert!(!record.outbox().contains_key(&pid(2)));
        assert!(record.outbox().contains_key(&pid(3)));
        assert_eq!(record.trusted_state(), b"reconciled");
        assert_eq!(record.applied_cursor(), Some(&cursor(2)));
        assert_eq!(store.committed.as_ref(), Some(&record));
    }

    #[test]
    fn batch_reconciliation_durability_failure_keeps_every_entry() {
        let mut record = two_at_same_cursor();
        let before = record.clone();
        let mut store = MemoryStore {
            fail: true,
            ..MemoryStore::default()
        };

        assert!(commit_reconciled_outbox_batch(
            &mut record,
            &mut store,
            &RevisionCodec,
            [pid(1), pid(2)],
            b"reconciled".to_vec(),
            &Revision(2),
        )
        .is_err());

        assert_eq!(record, before);
        assert!(record.outbox().contains_key(&pid(1)));
        assert!(record.outbox().contains_key(&pid(2)));
    }
}
