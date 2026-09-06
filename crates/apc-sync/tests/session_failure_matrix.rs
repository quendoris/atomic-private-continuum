use apc_sync::{
    commit_received, commit_reconciled_outbox, publish_staged, stage_outbound, DurableSyncRecord,
    OpaqueTransport, PublicationId, PublishOutcome, SyncRecordStore, TransportCursor,
    TransportCursorCodec,
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
    ) -> Result<apc_sync::FetchOutcome<Self::Revision>, Self::Error> {
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
