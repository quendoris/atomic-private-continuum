use apc_core::id::LOGICAL_ID_BYTES;
use apc_core::{
    AtomId, ContinuumId, LocalScalarDomain, RevisionId, ScalarRegister, WorkingEpochId,
};
use apc_crypto::ContentKey;
use apc_runtime::{
    prepare_scalar_handoff, resume_single_scalar_domain, stage_prepared_scalar_handoff,
    DevelopmentScalarTrustedStateCodec, ScalarCatchUpSpec, ScalarResumeError, TrustedStateCodec,
};
use apc_sync::{
    DomainKey, DurableSyncRecord, FetchOutcome, OpaqueTransport, PublicationId, PublishOutcome,
    SessionCommitError, SyncRecordStore, TransportCursor, TransportCursorCodec,
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

struct FailSecondPersistStore {
    calls: usize,
    committed: Option<DurableSyncRecord>,
}

impl SyncRecordStore for FailSecondPersistStore {
    type Error = &'static str;

    fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
        self.calls += 1;
        if self.calls == 2 {
            return Err("injected second persistence failure");
        }
        self.committed = Some(record.clone());
        Ok(())
    }
}

struct AcceptedTransport {
    head: Revision,
    objects: Vec<Vec<u8>>,
    publish_calls: usize,
}

impl OpaqueTransport for AcceptedTransport {
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
            Ok(FetchOutcome::UpToDate {
                head: Some(self.head),
            })
        } else {
            Ok(FetchOutcome::Changed {
                head: self.head,
                objects: self.objects.clone(),
            })
        }
    }

    fn publish(
        &mut self,
        _expected_head: Option<&Self::Revision>,
        _objects: &[Vec<u8>],
    ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
        self.publish_calls += 1;
        Err("unexpected republish during lost-ack reconciliation")
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

#[test]
fn failed_observed_outbox_retirement_keeps_caught_up_cursor_and_pending_retry_durable() {
    let trusted_codec = DevelopmentScalarTrustedStateCodec;
    let cursor_codec = RevisionCodec;
    let publication_key = ContentKey::from_bytes([0xB1; 32]);
    let semantic_key = DomainKey::new(atom(1), b"body".to_vec()).unwrap();

    let mut causal = ScalarRegister::new();
    causal.assign(rid(100), b"base".to_vec()).unwrap();
    let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
    domain
        .begin_epoch(wid(1), b"locally-exposed".to_vec())
        .unwrap();
    domain.seal_local(rid(200)).unwrap();
    domain.finalize(rid(200)).unwrap();

    let mut record = DurableSyncRecord::new(
        trusted_codec.encode(&domain.snapshot()).unwrap(),
        Some(cursor_codec.encode(&Revision(1)).unwrap()),
    );
    let prepared = prepare_scalar_handoff(
        &domain,
        semantic_key.clone(),
        cid(1),
        pid(7),
        &publication_key,
        [rid(200)],
    )
    .unwrap();
    let remotely_accepted = prepared.publication().objects().to_vec();
    let mut staging_store = MemoryStore::default();
    stage_prepared_scalar_handoff(
        &mut domain,
        &mut record,
        &mut staging_store,
        &trusted_codec,
        prepared,
    )
    .unwrap();

    let mut transport = AcceptedTransport {
        head: Revision(2),
        objects: remotely_accepted,
        publish_calls: 0,
    };
    let mut failing_store = FailSecondPersistStore {
        calls: 0,
        committed: Some(record.clone()),
    };

    let error = resume_single_scalar_domain(
        &mut domain,
        &mut record,
        &mut failing_store,
        &trusted_codec,
        &cursor_codec,
        &mut transport,
        ScalarCatchUpSpec::new(&publication_key, cid(1), &semantic_key, None),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ScalarResumeError::Reconcile(SessionCommitError::Store(
            "injected second persistence failure"
        ))
    ));
    assert_eq!(failing_store.calls, 2);
    assert_eq!(transport.publish_calls, 0);

    // The first durability barrier (authenticated catch-up to R2) succeeded and
    // remains the complete recoverable state. The failed second barrier cannot
    // retire only the process-local outbox.
    assert_eq!(
        cursor_codec
            .decode(record.applied_cursor().unwrap())
            .unwrap(),
        Revision(2)
    );
    assert!(record.outbox().contains_key(&pid(7)));
    assert_eq!(failing_store.committed.as_ref(), Some(&record));

    let recovered_snapshot = trusted_codec.decode(record.trusted_state()).unwrap();
    let recovered_domain = LocalScalarDomain::restore(recovered_snapshot).unwrap();
    assert_eq!(recovered_domain, domain);
    assert!(recovered_domain.causal().revision(rid(200)).is_some());
}
