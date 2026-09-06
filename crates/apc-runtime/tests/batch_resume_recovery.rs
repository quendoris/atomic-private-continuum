use std::collections::BTreeSet;

use apc_core::id::LOGICAL_ID_BYTES;
use apc_core::{
    AtomId, ContinuumId, LocalScalarDomain, RevisionId, ScalarRegister, WorkingEpochId,
};
use apc_crypto::ContentKey;
use apc_runtime::{
    prepare_scalar_handoff, resume_scalar_outbox_set, stage_prepared_scalar_handoff,
    DevelopmentScalarTrustedStateCodec, ScalarBatchResumeAction, ScalarBatchResumeError,
    ScalarCatchUpSpec, TrustedStateCodec,
};
use apc_sync::{
    DomainKey, DurableSyncRecord, FetchOutcome, OpaqueTransport, PublicationId, PublishOutcome,
    SyncRecordStore, TransportCursor, TransportCursorCodec,
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
struct FailableStore {
    committed: Option<DurableSyncRecord>,
    fail_next: bool,
    persist_calls: usize,
}

impl SyncRecordStore for FailableStore {
    type Error = &'static str;

    fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
        self.persist_calls += 1;
        if self.fail_next {
            self.fail_next = false;
            return Err("injected durability failure");
        }
        self.committed = Some(record.clone());
        Ok(())
    }
}

struct AcceptAndReplayTransport {
    head: Revision,
    accepted_objects: Vec<Vec<u8>>,
    fetch_calls: usize,
    publish_calls: usize,
}

impl OpaqueTransport for AcceptAndReplayTransport {
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
            return Ok(FetchOutcome::UpToDate {
                head: Some(self.head),
            });
        }

        if known_head == Some(&Revision(1)) && self.head == Revision(2) {
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

        self.accepted_objects = objects.to_vec();
        self.head = Revision(2);
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

fn base_domain() -> LocalScalarDomain<Vec<u8>> {
    let mut causal = ScalarRegister::new();
    causal.assign(rid(100), b"base".to_vec()).unwrap();
    LocalScalarDomain::from_causal(causal).unwrap()
}

#[test]
fn accepted_batch_with_failed_local_reconcile_is_rediscovered_and_retired_as_a_set() {
    let trusted_codec = DevelopmentScalarTrustedStateCodec;
    let cursor_codec = RevisionCodec;
    let key = ContentKey::from_bytes([0xF1; 32]);
    let semantic_key = domain_key();

    let mut domain = base_domain();
    let mut record = DurableSyncRecord::new(
        trusted_codec.encode(&domain.snapshot()).unwrap(),
        Some(cursor_codec.encode(&Revision(1)).unwrap()),
    );
    let mut store = FailableStore::default();

    domain.begin_epoch(wid(1), b"first".to_vec()).unwrap();
    domain.seal_local(rid(200)).unwrap();
    domain.finalize(rid(200)).unwrap();
    let first = prepare_scalar_handoff(
        &domain,
        semantic_key.clone(),
        cid(1),
        pid(1),
        &key,
        [rid(200)],
    )
    .unwrap();
    stage_prepared_scalar_handoff(
        &mut domain,
        &mut record,
        &mut store,
        &trusted_codec,
        first,
    )
    .unwrap();

    domain.begin_epoch(wid(2), b"second".to_vec()).unwrap();
    domain.seal_local(rid(300)).unwrap();
    domain.finalize(rid(300)).unwrap();
    let second = prepare_scalar_handoff(
        &domain,
        semantic_key.clone(),
        cid(1),
        pid(2),
        &key,
        [rid(300)],
    )
    .unwrap();
    stage_prepared_scalar_handoff(
        &mut domain,
        &mut record,
        &mut store,
        &trusted_codec,
        second,
    )
    .unwrap();

    assert_eq!(record.outbox().len(), 2);
    assert_eq!(
        cursor_codec
            .decode(record.applied_cursor().unwrap())
            .unwrap(),
        Revision(1)
    );

    let before_failed_reconcile = record.clone();
    store.fail_next = true;
    let mut transport = AcceptAndReplayTransport {
        head: Revision(1),
        accepted_objects: Vec::new(),
        fetch_calls: 0,
        publish_calls: 0,
    };

    let error = resume_scalar_outbox_set(
        &mut domain,
        &mut record,
        &mut store,
        &trusted_codec,
        &cursor_codec,
        &mut transport,
        ScalarCatchUpSpec::new(&key, cid(1), &semantic_key, None),
    )
    .unwrap_err();

    assert!(matches!(error, ScalarBatchResumeError::Reconcile(_)));
    assert_eq!(transport.head, Revision(2));
    assert_eq!(transport.publish_calls, 1);
    assert_eq!(record, before_failed_reconcile);
    assert_eq!(record.outbox().len(), 2);
    assert_eq!(
        cursor_codec
            .decode(record.applied_cursor().unwrap())
            .unwrap(),
        Revision(1)
    );

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
    assert_eq!(transport.publish_calls, 1);
    assert_eq!(transport.fetch_calls, 2);
    assert!(record.outbox().is_empty());
    assert_eq!(
        cursor_codec
            .decode(record.applied_cursor().unwrap())
            .unwrap(),
        Revision(2)
    );
    assert!(domain.causal().revision(rid(200)).is_some());
    assert!(domain.causal().revision(rid(300)).is_some());
    assert_eq!(store.committed.as_ref(), Some(&record));
}
