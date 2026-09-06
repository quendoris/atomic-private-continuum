use apc_sync::{
    fetch_from_durable_cursor, publish_staged, stage_outbound, DurableSyncRecord, FetchOutcome,
    ForegroundSyncLifecycle, ForegroundTransportError, OpaqueTransport, PublicationId,
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
struct MemoryStore {
    committed: Option<DurableSyncRecord>,
}

impl SyncRecordStore for MemoryStore {
    type Error = core::convert::Infallible;

    fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
        self.committed = Some(record.clone());
        Ok(())
    }
}

struct BackgroundingAcceptTransport {
    lifecycle: ForegroundSyncLifecycle,
    head: Revision,
    accepted_objects: Vec<Vec<u8>>,
    publish_calls: usize,
    background_on_next_publish: bool,
}

impl BackgroundingAcceptTransport {
    fn new(lifecycle: ForegroundSyncLifecycle, head: u64) -> Self {
        Self {
            lifecycle,
            head: Revision(head),
            accepted_objects: Vec::new(),
            publish_calls: 0,
            background_on_next_publish: true,
        }
    }
}

impl OpaqueTransport for BackgroundingAcceptTransport {
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
        self.publish_calls += 1;
        if expected_head != Some(&self.head) {
            return Ok(PublishOutcome::Conflict {
                current_head: Some(self.head),
            });
        }

        self.head = Revision(self.head.0 + 1);
        self.accepted_objects = objects.to_vec();

        if self.background_on_next_publish {
            self.background_on_next_publish = false;
            self.lifecycle.enter_background();
            return Err("request cancelled after remote acceptance");
        }

        Ok(PublishOutcome::Published { head: self.head })
    }
}

#[test]
fn background_during_in_flight_acceptance_blocks_more_io_until_resume_then_reconciles() {
    let mut record = DurableSyncRecord::new(b"baseline".to_vec(), Some(cursor(60)));
    let mut store = MemoryStore::default();
    let wire = b"exact-protected-publication".to_vec();
    stage_outbound(
        &mut record,
        &mut store,
        b"exposed".to_vec(),
        pid(60),
        vec![wire.clone()],
    )
    .unwrap();

    let lifecycle = ForegroundSyncLifecycle::new();
    lifecycle.enter_foreground();
    let inner = BackgroundingAcceptTransport::new(lifecycle.clone(), 60);
    let mut transport = lifecycle.guard(inner);

    // The request crosses the foreground gate, is accepted remotely, then the
    // simulated platform backgrounds/cancels before a success response survives.
    assert!(publish_staged(&record, pid(60), &mut transport, &RevisionCodec).is_err());
    assert!(!lifecycle.is_foreground());
    assert_eq!(transport.inner().head, Revision(61));
    assert_eq!(transport.inner().publish_calls, 1);
    assert!(record.outbox().contains_key(&pid(60)));

    // While backgrounded, a retry is rejected locally and never reaches transport.
    let blocked = publish_staged(&record, pid(60), &mut transport, &RevisionCodec).unwrap_err();
    assert!(matches!(
        blocked,
        apc_sync::SessionIoError::Transport(ForegroundTransportError::Backgrounded)
    ));
    assert_eq!(transport.inner().publish_calls, 1);

    // Resume uses the same durable outbox/cursor path. The retry discovers that
    // the old expected head is stale, then fetch rediscovers the accepted bytes.
    lifecycle.enter_foreground();
    assert_eq!(
        publish_staged(&record, pid(60), &mut transport, &RevisionCodec).unwrap(),
        PublishOutcome::Conflict {
            current_head: Some(Revision(61))
        }
    );
    assert_eq!(transport.inner().publish_calls, 2);

    assert_eq!(
        fetch_from_durable_cursor(&record, &mut transport, &RevisionCodec).unwrap(),
        FetchOutcome::Changed {
            head: Revision(61),
            objects: vec![wire]
        }
    );
    assert!(record.outbox().contains_key(&pid(60)));
}
