use crate::{
    DurableSyncRecord, FetchOutcome, OpaqueTransport, PublicationId, PublishOutcome,
    SyncRecoveryError, TransportCursor,
};

/// Durable storage boundary for the complete local synchronization recovery unit.
///
/// A production implementation is expected to encode, authenticate/protect and
/// durably commit the complete `DurableSyncRecord` before returning success.
/// The semantic sync layer does not depend on a particular filesystem layout.
pub trait SyncRecordStore {
    type Error;

    fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error>;
}

/// Reversible adapter between a concrete transport revision and local opaque
/// crash-recovery cursor bytes.
///
/// The encoded bytes are bookkeeping only. Implementations must not derive A.P.C.
/// causal or temporal meaning from their lexical/numeric value.
pub trait TransportCursorCodec<R> {
    type Error;

    fn encode(&self, revision: &R) -> Result<TransportCursor, Self::Error>;
    fn decode(&self, cursor: &TransportCursor) -> Result<R, Self::Error>;
}

#[derive(Debug)]
pub enum PersistTransitionError<E> {
    Recovery(SyncRecoveryError),
    Store(E),
}

#[derive(Debug)]
pub enum SessionIoError<TransportError, CursorError> {
    Recovery(SyncRecoveryError),
    Transport(TransportError),
    Cursor(CursorError),
}

#[derive(Debug)]
pub enum SessionCommitError<StoreError, CursorError> {
    Recovery(SyncRecoveryError),
    Store(StoreError),
    Cursor(CursorError),
}

/// Stage an outbound publication in durable local recovery state before network
/// I/O is allowed.
///
/// The publication automatically targets the currently durable applied cursor.
/// The caller supplies `exposed_trusted_state`, which must already contain the
/// semantic handoff/exposure bookkeeping required by the core finalization layer.
/// This function clones first and only updates the caller's in-memory record after
/// the complete next record has been persisted successfully.
pub fn stage_outbound<S: SyncRecordStore>(
    record: &mut DurableSyncRecord,
    store: &mut S,
    exposed_trusted_state: Vec<u8>,
    publication_id: PublicationId,
    protected_objects: Vec<Vec<u8>>,
) -> Result<(), PersistTransitionError<S::Error>> {
    let mut next = record.clone();
    next.prepare_outbox(
        exposed_trusted_state,
        publication_id,
        record.applied_cursor().cloned(),
        protected_objects,
    )
    .map_err(PersistTransitionError::Recovery)?;

    store
        .persist(&next)
        .map_err(PersistTransitionError::Store)?;
    *record = next;
    Ok(())
}

/// Attempt one already-staged outbound publication using its exact durable wire
/// bytes. The durable outbox is deliberately not modified on success or conflict.
/// A lost acknowledgement therefore cannot make a restart forget that the bytes
/// may already be externally observable.
pub fn publish_staged<T, C>(
    record: &DurableSyncRecord,
    publication_id: PublicationId,
    transport: &mut T,
    codec: &C,
) -> Result<PublishOutcome<T::Revision>, SessionIoError<T::Error, C::Error>>
where
    T: OpaqueTransport,
    C: TransportCursorCodec<T::Revision>,
{
    let entry = record
        .outbox()
        .get(&publication_id)
        .ok_or(SessionIoError::Recovery(
            SyncRecoveryError::UnknownOutboxPublication,
        ))?;

    let expected = entry
        .expected_cursor()
        .map(|cursor| codec.decode(cursor))
        .transpose()
        .map_err(SessionIoError::Cursor)?;

    transport
        .publish(expected.as_ref(), entry.objects())
        .map_err(SessionIoError::Transport)
}

/// Fetch from exactly the transport cursor paired with the current durable local
/// state. This function does not mutate that cursor. The caller must first merge
/// and durably persist the resulting trusted state through `commit_received()`.
pub fn fetch_from_durable_cursor<T, C>(
    record: &DurableSyncRecord,
    transport: &mut T,
    codec: &C,
) -> Result<FetchOutcome<T::Revision>, SessionIoError<T::Error, C::Error>>
where
    T: OpaqueTransport,
    C: TransportCursorCodec<T::Revision>,
{
    let known = record
        .applied_cursor()
        .map(|cursor| codec.decode(cursor))
        .transpose()
        .map_err(SessionIoError::Cursor)?;

    transport
        .fetch_since(known.as_ref())
        .map_err(SessionIoError::Transport)
}

/// Atomically pair a newly merged trusted local state with the transport revision
/// from which that state was obtained.
///
/// If cursor encoding or durable persistence fails, the caller's in-memory record
/// remains unchanged, so it cannot accidentally outrun recovery state.
pub fn commit_received<S, C, R>(
    record: &mut DurableSyncRecord,
    store: &mut S,
    codec: &C,
    merged_trusted_state: Vec<u8>,
    new_head: &R,
) -> Result<(), SessionCommitError<S::Error, C::Error>>
where
    S: SyncRecordStore,
    C: TransportCursorCodec<R>,
{
    let cursor = codec.encode(new_head).map_err(SessionCommitError::Cursor)?;
    let mut next = record.clone();
    next.apply_received(merged_trusted_state, cursor);
    store.persist(&next).map_err(SessionCommitError::Store)?;
    *record = next;
    Ok(())
}

/// Reconcile and retire one named outbound publication while durably pairing the
/// resulting trusted state with the observed transport head.
///
/// Other staged/in-flight publications are preserved.
pub fn commit_reconciled_outbox<S, C, R>(
    record: &mut DurableSyncRecord,
    store: &mut S,
    codec: &C,
    publication_id: PublicationId,
    reconciled_trusted_state: Vec<u8>,
    new_head: &R,
) -> Result<(), SessionCommitError<S::Error, C::Error>>
where
    S: SyncRecordStore,
    C: TransportCursorCodec<R>,
{
    let cursor = codec.encode(new_head).map_err(SessionCommitError::Cursor)?;
    let mut next = record.clone();
    next.retire_outbox(publication_id, reconciled_trusted_state, cursor)
        .map_err(SessionCommitError::Recovery)?;
    store.persist(&next).map_err(SessionCommitError::Store)?;
    *record = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Revision(u64);

    #[derive(Default)]
    struct U64CursorCodec;

    impl TransportCursorCodec<Revision> for U64CursorCodec {
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
        durable: Option<DurableSyncRecord>,
        fail: bool,
    }

    impl SyncRecordStore for MemoryStore {
        type Error = &'static str;

        fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
            if self.fail {
                return Err("injected store failure");
            }
            self.durable = Some(record.clone());
            Ok(())
        }
    }

    #[derive(Clone, Debug)]
    struct Commit {
        revision: Revision,
        objects: Vec<Vec<u8>>,
    }

    #[derive(Default)]
    struct MemoryTransport {
        commits: Vec<Commit>,
        next_revision: u64,
    }

    impl OpaqueTransport for MemoryTransport {
        type Revision = Revision;
        type Error = &'static str;

        fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
            Ok(self.commits.last().map(|commit| commit.revision))
        }

        fn fetch_since(
            &mut self,
            known_head: Option<&Self::Revision>,
        ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
            let current = self.commits.last().map(|commit| commit.revision);
            if current.as_ref() == known_head {
                return Ok(FetchOutcome::UpToDate { head: current });
            }

            let start = match known_head {
                None => 0,
                Some(known) => match self
                    .commits
                    .iter()
                    .position(|commit| &commit.revision == known)
                {
                    Some(index) => index + 1,
                    None => {
                        return Ok(FetchOutcome::BaselineUnavailable { head: current });
                    }
                },
            };

            let Some(head) = current else {
                return Ok(FetchOutcome::UpToDate { head: None });
            };
            let objects = self.commits[start..]
                .iter()
                .flat_map(|commit| commit.objects.iter().cloned())
                .collect();
            Ok(FetchOutcome::Changed { head, objects })
        }

        fn publish(
            &mut self,
            expected_head: Option<&Self::Revision>,
            objects: &[Vec<u8>],
        ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
            let current = self.commits.last().map(|commit| commit.revision);
            if current.as_ref() != expected_head {
                return Ok(PublishOutcome::Conflict {
                    current_head: current,
                });
            }

            self.next_revision += 1;
            let revision = Revision(self.next_revision);
            self.commits.push(Commit {
                revision,
                objects: objects.to_vec(),
            });
            Ok(PublishOutcome::Published { head: revision })
        }
    }

    fn pid(value: u64) -> PublicationId {
        let mut bytes = [0_u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        PublicationId::from_bytes(bytes)
    }

    fn cursor(value: u64) -> TransportCursor {
        U64CursorCodec.encode(&Revision(value)).unwrap()
    }

    #[test]
    fn lost_ack_keeps_exact_outbox_until_fetch_and_durable_reconciliation() {
        let codec = U64CursorCodec;
        let mut transport = MemoryTransport::default();
        let seed = match transport.publish(None, &[b"baseline".to_vec()]).unwrap() {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected baseline result: {other:?}"),
        };

        let mut record = DurableSyncRecord::new(b"local".to_vec(), Some(cursor(seed.0)));
        let mut store = MemoryStore::default();
        stage_outbound(
            &mut record,
            &mut store,
            b"local-exposed".to_vec(),
            pid(7),
            vec![b"exact-protected-wire".to_vec()],
        )
        .unwrap();

        assert_eq!(store.durable.as_ref(), Some(&record));
        let first = publish_staged(&record, pid(7), &mut transport, &codec).unwrap();
        let accepted_head = match first {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected publication result: {other:?}"),
        };
        assert!(record.outbox().contains_key(&pid(7)));

        // Simulate process death after remote acceptance but before local ACK
        // handling: restart exclusively from the durable pre-network record.
        let mut restarted = store.durable.clone().unwrap();
        let retry = publish_staged(&restarted, pid(7), &mut transport, &codec).unwrap();
        assert_eq!(
            retry,
            PublishOutcome::Conflict {
                current_head: Some(accepted_head)
            }
        );
        assert!(restarted.outbox().contains_key(&pid(7)));

        let fetched = fetch_from_durable_cursor(&restarted, &mut transport, &codec).unwrap();
        let FetchOutcome::Changed { head, objects } = fetched else {
            panic!("expected changed transport state after lost ACK")
        };
        assert_eq!(head, accepted_head);
        assert_eq!(objects, vec![b"exact-protected-wire".to_vec()]);

        commit_reconciled_outbox(
            &mut restarted,
            &mut store,
            &codec,
            pid(7),
            b"reconciled".to_vec(),
            &head,
        )
        .unwrap();

        assert!(restarted.outbox().is_empty());
        assert_eq!(restarted.applied_cursor(), Some(&cursor(head.0)));
        assert_eq!(store.durable.as_ref(), Some(&restarted));
    }

    #[test]
    fn failed_durable_commit_cannot_advance_in_memory_state_or_cursor() {
        let codec = U64CursorCodec;
        let original = DurableSyncRecord::new(b"old".to_vec(), Some(cursor(1)));
        let mut record = original.clone();
        let mut store = MemoryStore {
            durable: Some(original.clone()),
            fail: true,
        };

        assert!(commit_received(
            &mut record,
            &mut store,
            &codec,
            b"merged".to_vec(),
            &Revision(2),
        )
        .is_err());

        assert_eq!(record, original);
        assert_eq!(store.durable, Some(original));
    }

    #[test]
    fn failed_outbox_persistence_prevents_network_eligible_state_transition() {
        let mut record = DurableSyncRecord::new(b"private".to_vec(), Some(cursor(1)));
        let original = record.clone();
        let mut store = MemoryStore {
            durable: Some(original.clone()),
            fail: true,
        };

        assert!(stage_outbound(
            &mut record,
            &mut store,
            b"exposed".to_vec(),
            pid(11),
            vec![b"protected".to_vec()],
        )
        .is_err());

        assert_eq!(record, original);
        assert!(record.outbox().is_empty());
        assert_eq!(store.durable, Some(original));
    }
}
