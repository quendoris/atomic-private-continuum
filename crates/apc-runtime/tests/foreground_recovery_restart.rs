use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use apc_core::id::LOGICAL_ID_BYTES;
use apc_core::{
    AtomId, ContinuumId, LocalScalarDomain, RevisionId, ScalarRegister, WorkingEpochId,
};
use apc_crypto::ContentKey;
use apc_runtime::{
    prepare_recovery_handoff, stage_prepared_recovery_handoff,
    DevelopmentMultiScalarTrustedStateCodec, ForegroundRecoveryCycleSpec,
    ForegroundRecoveryRuntime, LocalScalarRecoveryState, ScalarRecoveryCatchUpOutcome,
    ScalarRecoveryResumeAction, TrustedStateCodec,
};
use apc_storage_fs::UnixFsDurabilityBackend;
use apc_sync::{
    DomainKey, DurableSyncRecord, FetchOutcome, OpaqueTransport, ProtectedSyncRecordStore,
    PublicationId, PublishOutcome, TransportCursor, TransportCursorCodec,
};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

const STORE_KEY: [u8; 32] = [0xA8; 32];
const PUBLICATION_KEY: [u8; 32] = [0xA9; 32];
const STORE_CONTEXT: &[u8] = b"apc-runtime-test/foreground-recovery/continuum-a";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-runtime-foreground-recovery-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TransportRevision(u64);

struct RevisionCodec;

impl TransportCursorCodec<TransportRevision> for RevisionCodec {
    type Error = &'static str;

    fn encode(&self, revision: &TransportRevision) -> Result<TransportCursor, Self::Error> {
        TransportCursor::new(revision.0.to_be_bytes().to_vec()).map_err(|_| "encode")
    }

    fn decode(&self, cursor: &TransportCursor) -> Result<TransportRevision, Self::Error> {
        let bytes: [u8; 8] = cursor.as_bytes().try_into().map_err(|_| "decode")?;
        Ok(TransportRevision(u64::from_be_bytes(bytes)))
    }
}

struct AcceptedTransport {
    head: TransportRevision,
    objects: Vec<Vec<u8>>,
    fetch_calls: Arc<AtomicUsize>,
    publish_calls: Arc<AtomicUsize>,
}

impl OpaqueTransport for AcceptedTransport {
    type Revision = TransportRevision;
    type Error = &'static str;

    fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
        Ok(Some(self.head))
    }

    fn fetch_since(
        &mut self,
        known_head: Option<&Self::Revision>,
    ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
        self.fetch_calls.fetch_add(1, Ordering::Relaxed);
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
        self.publish_calls.fetch_add(1, Ordering::Relaxed);
        Err("lost-ACK recovery must not republish")
    }
}

fn logical_bytes(value: u64) -> [u8; LOGICAL_ID_BYTES] {
    let mut bytes = [0_u8; LOGICAL_ID_BYTES];
    bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
    bytes
}

fn atom(value: u64) -> AtomId {
    AtomId::from_bytes(logical_bytes(value))
}

fn rid(value: u64) -> RevisionId {
    RevisionId::from_bytes(logical_bytes(value))
}

fn wid(value: u64) -> WorkingEpochId {
    WorkingEpochId::from_bytes(logical_bytes(value))
}

fn pid(value: u64) -> PublicationId {
    PublicationId::from_bytes(logical_bytes(value))
}

fn cid(value: u64) -> ContinuumId {
    ContinuumId::from_bytes(logical_bytes(value))
}

fn domain(name: &str) -> DomainKey {
    DomainKey::new(atom(1), name.as_bytes()).unwrap()
}

fn finalized_domain(
    base_revision: u64,
    local_revision: u64,
    epoch: u64,
    base: &str,
    value: &str,
) -> LocalScalarDomain<Vec<u8>> {
    let mut causal = ScalarRegister::new();
    causal
        .assign(rid(base_revision), base.as_bytes().to_vec())
        .unwrap();
    let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
    domain
        .begin_epoch(wid(epoch), value.as_bytes().to_vec())
        .unwrap();
    domain.seal_local(rid(local_revision)).unwrap();
    domain.finalize(rid(local_revision)).unwrap();
    domain
}

#[test]
fn foreground_runtime_reconciles_lost_ack_only_after_foreground_entry_and_survives_restart() {
    let directory = TestDir::new();
    let codec = DevelopmentMultiScalarTrustedStateCodec;
    let cursor_codec = RevisionCodec;
    let publication_key = ContentKey::from_bytes(PUBLICATION_KEY);
    let body_key = domain("body");
    let title_key = domain("title");

    let body = finalized_domain(100, 200, 1, "body-base-secret", "body-local-secret");
    let title = finalized_domain(300, 400, 2, "title-base-secret", "title-local-secret");
    let mut recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
        (body_key.clone(), body.snapshot()),
        (title_key.clone(), title.snapshot()),
    ]))
    .unwrap();
    let mut record = DurableSyncRecord::new(
        codec.encode(&recovery).unwrap(),
        Some(cursor_codec.encode(&TransportRevision(1)).unwrap()),
    );

    let backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let mut store = ProtectedSyncRecordStore::new(
        backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();

    let prepared = prepare_recovery_handoff(
        &recovery,
        cid(1),
        pid(7),
        &publication_key,
        BTreeMap::from([
            (body_key.clone(), BTreeSet::from([rid(200)])),
            (title_key.clone(), BTreeSet::from([rid(400)])),
        ]),
    )
    .unwrap();
    let accepted_objects = prepared.objects().to_vec();
    stage_prepared_recovery_handoff(&mut recovery, &mut record, &mut store, &codec, prepared)
        .unwrap();

    drop(store);
    drop(record);
    drop(recovery);

    let reopened_backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let mut reopened_store = ProtectedSyncRecordStore::new(
        reopened_backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();
    let mut recovered_record = reopened_store.load_committed().unwrap().unwrap();
    let mut recovered = codec.decode(recovered_record.trusted_state()).unwrap();
    let pre_observation = BTreeMap::new();

    let fetch_calls = Arc::new(AtomicUsize::new(0));
    let publish_calls = Arc::new(AtomicUsize::new(0));
    let mut runtime = ForegroundRecoveryRuntime::new(AcceptedTransport {
        head: TransportRevision(2),
        objects: accepted_objects,
        fetch_calls: Arc::clone(&fetch_calls),
        publish_calls: Arc::clone(&publish_calls),
    });

    assert!(!runtime.is_foreground());
    let pre_foreground_record = recovered_record.clone();
    assert!(runtime
        .resume_scalar_recovery(
            &mut recovered,
            &mut recovered_record,
            &mut reopened_store,
            &codec,
            &cursor_codec,
            ForegroundRecoveryCycleSpec::new(&publication_key, cid(1), &pre_observation),
        )
        .is_err());
    assert_eq!(fetch_calls.load(Ordering::Relaxed), 0);
    assert_eq!(publish_calls.load(Ordering::Relaxed), 0);
    assert_eq!(recovered_record, pre_foreground_record);
    assert!(recovered_record.outbox().contains_key(&pid(7)));

    runtime.enter_foreground();
    let report = runtime
        .resume_scalar_recovery(
            &mut recovered,
            &mut recovered_record,
            &mut reopened_store,
            &codec,
            &cursor_codec,
            ForegroundRecoveryCycleSpec::new(&publication_key, cid(1), &pre_observation),
        )
        .unwrap();

    assert_eq!(report.initial.observed_reconciled, BTreeSet::from([pid(7)]));
    assert_eq!(report.initial.action, ScalarRecoveryResumeAction::None);
    assert!(report.post_conflict.is_none());
    assert_eq!(fetch_calls.load(Ordering::Relaxed), 1);
    assert_eq!(publish_calls.load(Ordering::Relaxed), 0);
    assert!(recovered_record.outbox().is_empty());
    assert_eq!(
        cursor_codec
            .decode(recovered_record.applied_cursor().unwrap())
            .unwrap(),
        TransportRevision(2)
    );

    runtime.enter_background();
    let foreground_record = recovered_record.clone();
    assert!(runtime
        .resume_scalar_recovery(
            &mut recovered,
            &mut recovered_record,
            &mut reopened_store,
            &codec,
            &cursor_codec,
            ForegroundRecoveryCycleSpec::new(&publication_key, cid(1), &pre_observation),
        )
        .is_err());
    assert_eq!(fetch_calls.load(Ordering::Relaxed), 1);
    assert_eq!(publish_calls.load(Ordering::Relaxed), 0);
    assert_eq!(recovered_record, foreground_record);

    drop(runtime);
    drop(reopened_store);
    drop(recovered_record);
    drop(recovered);

    let final_backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let mut final_store = ProtectedSyncRecordStore::new(
        final_backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();
    let mut final_record = final_store.load_committed().unwrap().unwrap();
    let mut final_recovery = codec.decode(final_record.trusted_state()).unwrap();
    assert!(final_record.outbox().is_empty());
    assert_eq!(
        cursor_codec
            .decode(final_record.applied_cursor().unwrap())
            .unwrap(),
        TransportRevision(2)
    );

    let second_fetch_calls = Arc::new(AtomicUsize::new(0));
    let second_publish_calls = Arc::new(AtomicUsize::new(0));
    let mut second_runtime = ForegroundRecoveryRuntime::new(AcceptedTransport {
        head: TransportRevision(2),
        objects: Vec::new(),
        fetch_calls: Arc::clone(&second_fetch_calls),
        publish_calls: Arc::clone(&second_publish_calls),
    });

    assert!(second_runtime
        .resume_scalar_recovery(
            &mut final_recovery,
            &mut final_record,
            &mut final_store,
            &codec,
            &cursor_codec,
            ForegroundRecoveryCycleSpec::new(&publication_key, cid(1), &pre_observation),
        )
        .is_err());
    assert_eq!(second_fetch_calls.load(Ordering::Relaxed), 0);

    second_runtime.enter_foreground();
    let second = second_runtime
        .resume_scalar_recovery(
            &mut final_recovery,
            &mut final_record,
            &mut final_store,
            &codec,
            &cursor_codec,
            ForegroundRecoveryCycleSpec::new(&publication_key, cid(1), &pre_observation),
        )
        .unwrap();
    assert_eq!(
        second.initial.catch_up,
        ScalarRecoveryCatchUpOutcome::UpToDate {
            head: Some(TransportRevision(2))
        }
    );
    assert_eq!(second.initial.action, ScalarRecoveryResumeAction::None);
    assert!(second.post_conflict.is_none());
    assert_eq!(second_fetch_calls.load(Ordering::Relaxed), 1);
    assert_eq!(second_publish_calls.load(Ordering::Relaxed), 0);
}
