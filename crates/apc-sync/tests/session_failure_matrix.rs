use apc_sync::{
    commit_received, commit_reconciled_outbox, fetch_from_durable_cursor, publish_staged,
    stage_outbound, DurableSyncRecord, FetchOutcome, OpaqueTransport, PublicationId,
    PublishOutcome, SyncRecordStore, TransportCursor, TransportCursorCodec,
};

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

fn cursor(value: u64) -> TransportCursor {
    RevisionCodec.encode(&Revision(value)).unwrap()
}

fn pid(value: u64) -> PublicationId {
    let mut bytes = [0_u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    PublicationId::from_bytes(bytes)
}

#[derive(Default)]
struct RecordingStore {
    committed: Option<DurableSyncRecord>,
}

impl SyncRecordStore for RecordingStore {
    type Error = &'static str;

    fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
        self.committed = Some(record.clone());
        Ok(())
    }
}

struct FailingStore;

impl SyncRecordStore for FailingStore {
    type Error = &'static str;

    fn persist(&mut self, _record: &DurableSyncRecord) -> Result<(), Self::Error> {
        Err("durability failure")
    }
}

#[derive(Default)]
struct FailingTransport {
    publish_calls: usize,
}

impl OpaqueTransport for FailingTransport {
    type Revision = Revision;
    type Error = &'static str;

    fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
        Err("network failure")
    }

    fn fetch_since(
        &mut self,
        _known_head: Option<&Self::Revision>,
    ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
        Err("network failure")
    }

    fn publish(
        &mut self,
        _expected_head: Option<&Self::Revision>,
        _objects: &[Vec<u8>],
    ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
        self.publish_calls += 1;
        Err("network failure")
    }
}

struct AcceptThenLoseAckTransport {
    head: Revision,
    accepted_objects: Vec<Vec<u8>>,
    lose_next_ack: bool,
}

impl AcceptThenLoseAckTransport {
    fn new(head: u64) -> Self {
        Self {
            head: Revision(head),
            accepted_objects: Vec::new(),
            lose_next_ack: true,
        }
    }
}

impl OpaqueTransport for AcceptThenLoseAckTransport {
    type Revision = Revision;
    type Error = &'static str;

    fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
        Ok(Some(self.head))
    }

    fn fetch_since(
        &mut self,
        known_head: Option<&Self::Revision>,
    ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
        if known_head == Some(&self.head) {
            return Ok(FetchOutcome::UpToDate {
                head: Some(self.head),
            });
        }

        let predecessor = Revision(self.head.0.saturating_sub(1));
        if known_head == Some(&predecessor) {
            return Ok(FetchOutcome::Changed {
                head: self.head,
                objects: self.accepted_objects.clone(),
            });
        }

        Ok(FetchOutcome::BaselineUnavailable {
            head: Some(self.head),
        })
    }

    fn publish(
        &mut self,
        expected_head: Option<&Self::Revision>,
        objects: &[Vec<u8>],
    ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
        if expected_head != Some(&self.head) {
            return Ok(PublishOutcome::Conflict {
                current_head: Some(self.head),
            });
        }

        self.head = Revision(self.head.0 + 1);
        self.accepted_objects = objects.to_vec();
        if self.lose_next_ack {
            self.lose_next_ack = false;
            Err("response lost after remote acceptance")
        } else {
            Ok(PublishOutcome::Published { head: self.head })
        }
    }
}

#[test]
fn outbox_persistence_failure_does_not_expose_new_in_memory_state() {
    let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(10)));
    let before = record.clone();
    let mut store = FailingStore;

    assert!(stage_outbound(
        &mut record,
        &mut store,
        b"exposed".to_vec(),
        pid(1),
        vec![b"protected".to_vec()],
    )
    .is_err());

    assert_eq!(record, before);
    assert!(record.outbox().is_empty());
}

#[test]
fn network_failure_after_durable_outbox_keeps_exact_retry_material() {
    let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(10)));
    let mut store = RecordingStore::default();
    let wire = b"exact-protected-object".to_vec();

    stage_outbound(
        &mut record,
        &mut store,
        b"exposed".to_vec(),
        pid(2),
        vec![wire.clone()],
    )
    .unwrap();
    let durable_before_network = store.committed.clone().unwrap();

    let mut transport = FailingTransport::default();
    assert!(publish_staged(&record, pid(2), &mut transport, &RevisionCodec).is_err());

    assert_eq!(transport.publish_calls, 1);
    assert_eq!(record, durable_before_network);
    assert_eq!(
        record.outbox().get(&pid(2)).unwrap().objects(),
        std::slice::from_ref(&wire)
    );
}

#[test]
fn remote_accept_then_response_loss_is_recovered_as_unknown_outcome() {
    let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(40)));
    let mut store = RecordingStore::default();
    let wire = b"protected-accepted-object".to_vec();
    stage_outbound(
        &mut record,
        &mut store,
        b"exposed".to_vec(),
        pid(4),
        vec![wire.clone()],
    )
    .unwrap();

    let mut transport = AcceptThenLoseAckTransport::new(40);
    assert!(publish_staged(&record, pid(4), &mut transport, &RevisionCodec).is_err());
    assert_eq!(transport.head, Revision(41));
    assert!(record.outbox().contains_key(&pid(4)));

    assert_eq!(
        publish_staged(&record, pid(4), &mut transport, &RevisionCodec).unwrap(),
        PublishOutcome::Conflict {
            current_head: Some(Revision(41))
        }
    );

    assert_eq!(
        fetch_from_durable_cursor(&record, &mut transport, &RevisionCodec).unwrap(),
        FetchOutcome::Changed {
            head: Revision(41),
            objects: vec![wire]
        }
    );
    assert!(record.outbox().contains_key(&pid(4)));
}

#[test]
fn inbound_commit_failure_cannot_advance_only_the_in_memory_cursor() {
    let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(20)));
    let before = record.clone();
    let mut store = FailingStore;

    assert!(commit_received(
        &mut record,
        &mut store,
        &RevisionCodec,
        b"merged".to_vec(),
        &Revision(21),
    )
    .is_err());

    assert_eq!(record, before);
    assert_eq!(
        RevisionCodec
            .decode(record.applied_cursor().unwrap())
            .unwrap(),
        Revision(20)
    );
}

#[test]
fn reconcile_commit_failure_keeps_pending_outbox_and_old_cursor() {
    let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(30)));
    let mut good_store = RecordingStore::default();
    stage_outbound(
        &mut record,
        &mut good_store,
        b"exposed".to_vec(),
        pid(3),
        vec![b"protected".to_vec()],
    )
    .unwrap();
    let before = record.clone();

    let mut failing_store = FailingStore;
    assert!(commit_reconciled_outbox(
        &mut record,
        &mut failing_store,
        &RevisionCodec,
        pid(3),
        b"reconciled".to_vec(),
        &Revision(31),
    )
    .is_err());

    assert_eq!(record, before);
    assert!(record.outbox().contains_key(&pid(3)));
    assert_eq!(
        RevisionCodec
            .decode(record.applied_cursor().unwrap())
            .unwrap(),
        Revision(30)
    );
}

#[test]
fn reconciling_one_publication_preserves_other_pending_publications_verbatim() {
    let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(50)));
    let mut store = RecordingStore::default();
    let first_wire = b"first-protected-object".to_vec();
    let second_wire = b"second-protected-object".to_vec();

    stage_outbound(
        &mut record,
        &mut store,
        b"exposed-first".to_vec(),
        pid(5),
        vec![first_wire],
    )
    .unwrap();
    stage_outbound(
        &mut record,
        &mut store,
        b"exposed-second".to_vec(),
        pid(6),
        vec![second_wire.clone()],
    )
    .unwrap();

    commit_reconciled_outbox(
        &mut record,
        &mut store,
        &RevisionCodec,
        pid(5),
        b"reconciled-first".to_vec(),
        &Revision(51),
    )
    .unwrap();

    assert!(!record.outbox().contains_key(&pid(5)));
    let second = record.outbox().get(&pid(6)).unwrap();
    assert_eq!(second.objects(), std::slice::from_ref(&second_wire));
    assert_eq!(
        RevisionCodec
            .decode(second.expected_cursor().unwrap())
            .unwrap(),
        Revision(50)
    );
    assert_eq!(
        RevisionCodec
            .decode(record.applied_cursor().unwrap())
            .unwrap(),
        Revision(51)
    );
}
