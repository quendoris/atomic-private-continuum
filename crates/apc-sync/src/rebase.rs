use crate::{
    DurableSyncRecord, PublicationId, SessionCommitError, SyncRecordStore, SyncRecoveryError,
    TransportCursorCodec,
};

/// Complete replacement material for one stale durable outbound publication.
///
/// A rebase never mutates the old protected bytes and never reuses their
/// `PublicationId`. The replacement carries a fresh publication identity and
/// already-protected wire objects built from the reconciled trusted state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxRebase {
    stale_publication_id: PublicationId,
    rebased_trusted_state: Vec<u8>,
    new_publication_id: PublicationId,
    new_protected_objects: Vec<Vec<u8>>,
}

impl OutboxRebase {
    pub fn new(
        stale_publication_id: PublicationId,
        rebased_trusted_state: Vec<u8>,
        new_publication_id: PublicationId,
        new_protected_objects: Vec<Vec<u8>>,
    ) -> Self {
        Self {
            stale_publication_id,
            rebased_trusted_state,
            new_publication_id,
            new_protected_objects,
        }
    }
}

/// Atomically replace one stale durable outbound publication with a newly
/// protected publication prepared against a newer reconciled transport head.
///
/// This transition is required when an already-staged publication still targets
/// an older transport cursor after another local/remote publication advanced the
/// durable applied cursor. The old protected bytes and `PublicationId` are never
/// mutated or reused. The caller must provide a fresh publication identity and
/// protected objects produced from the already-reconciled trusted state.
///
/// Other pending outbox entries are preserved. If cursor encoding, recovery-state
/// validation or persistence fails, the caller's in-memory record remains
/// unchanged.
pub fn commit_rebased_outbox<S, C, R>(
    record: &mut DurableSyncRecord,
    store: &mut S,
    codec: &C,
    rebase: OutboxRebase,
    new_head: &R,
) -> Result<(), SessionCommitError<S::Error, C::Error>>
where
    S: SyncRecordStore,
    C: TransportCursorCodec<R>,
{
    if rebase.stale_publication_id == rebase.new_publication_id {
        return Err(SessionCommitError::Recovery(
            SyncRecoveryError::PublicationIdentityCollision,
        ));
    }

    let cursor = codec.encode(new_head).map_err(SessionCommitError::Cursor)?;
    let mut next = record.clone();
    next.retire_outbox(
        rebase.stale_publication_id,
        rebase.rebased_trusted_state.clone(),
        cursor.clone(),
    )
    .map_err(SessionCommitError::Recovery)?;
    next.prepare_outbox(
        rebase.rebased_trusted_state,
        rebase.new_publication_id,
        Some(cursor),
        rebase.new_protected_objects,
    )
    .map_err(SessionCommitError::Recovery)?;

    store.persist(&next).map_err(SessionCommitError::Store)?;
    *record = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{stage_outbound, TransportCursor};

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

    fn cursor(value: u64) -> TransportCursor {
        RevisionCodec.encode(&Revision(value)).unwrap()
    }

    fn pid(value: u64) -> PublicationId {
        let mut bytes = [0_u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        PublicationId::from_bytes(bytes)
    }

    #[test]
    fn stale_publication_is_replaced_with_fresh_identity_at_new_cursor() {
        let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(70)));
        let mut store = MemoryStore::default();
        stage_outbound(
            &mut record,
            &mut store,
            b"old-exposed".to_vec(),
            pid(70),
            vec![b"old-protected".to_vec()],
        )
        .unwrap();

        commit_rebased_outbox(
            &mut record,
            &mut store,
            &RevisionCodec,
            OutboxRebase::new(
                pid(70),
                b"rebased-exposed".to_vec(),
                pid(71),
                vec![b"rebased-protected".to_vec()],
            ),
            &Revision(71),
        )
        .unwrap();

        assert!(!record.outbox().contains_key(&pid(70)));
        let rebased = record.outbox().get(&pid(71)).unwrap();
        assert_eq!(rebased.objects(), &[b"rebased-protected".to_vec()]);
        assert_eq!(
            RevisionCodec
                .decode(rebased.expected_cursor().unwrap())
                .unwrap(),
            Revision(71)
        );
        assert_eq!(
            RevisionCodec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(71)
        );
        assert_eq!(record.trusted_state(), b"rebased-exposed");
        assert_eq!(store.committed.as_ref(), Some(&record));
    }

    #[test]
    fn rebase_persistence_failure_keeps_old_publication_and_cursor() {
        let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(80)));
        let mut setup_store = MemoryStore::default();
        stage_outbound(
            &mut record,
            &mut setup_store,
            b"old-exposed".to_vec(),
            pid(80),
            vec![b"old-protected".to_vec()],
        )
        .unwrap();
        let before = record.clone();

        let mut failing_store = MemoryStore {
            fail: true,
            ..MemoryStore::default()
        };
        assert!(commit_rebased_outbox(
            &mut record,
            &mut failing_store,
            &RevisionCodec,
            OutboxRebase::new(
                pid(80),
                b"rebased-exposed".to_vec(),
                pid(81),
                vec![b"rebased-protected".to_vec()],
            ),
            &Revision(81),
        )
        .is_err());

        assert_eq!(record, before);
        assert!(record.outbox().contains_key(&pid(80)));
        assert!(!record.outbox().contains_key(&pid(81)));
        assert_eq!(
            RevisionCodec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(80)
        );
    }

    #[test]
    fn rebase_cannot_reuse_exposed_publication_identity() {
        let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(90)));
        let mut store = MemoryStore::default();
        stage_outbound(
            &mut record,
            &mut store,
            b"old-exposed".to_vec(),
            pid(90),
            vec![b"old-protected".to_vec()],
        )
        .unwrap();
        let before = record.clone();
        let calls_before = store.persist_calls;

        let error = commit_rebased_outbox(
            &mut record,
            &mut store,
            &RevisionCodec,
            OutboxRebase::new(
                pid(90),
                b"rebased-exposed".to_vec(),
                pid(90),
                vec![b"different-protected".to_vec()],
            ),
            &Revision(91),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            SessionCommitError::Recovery(SyncRecoveryError::PublicationIdentityCollision)
        ));
        assert_eq!(record, before);
        assert_eq!(store.persist_calls, calls_before);
    }
}
