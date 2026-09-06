use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use apc_core::id::LOGICAL_ID_BYTES;
use apc_core::{
    AtomId, DurabilityBackend, LocalScalarDomain, RevisionId, ScalarRegister, WorkingEpochId,
};
use apc_crypto::ContentKey;
use apc_runtime::{
    DevelopmentMultiScalarTrustedStateCodec, LocalScalarRecoveryState, TrustedStateCodec,
};
use apc_storage_fs::UnixFsDurabilityBackend;
use apc_sync::{DomainKey, DurableSyncRecord, ProtectedSyncRecordStore, SyncRecordStore};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

const STORE_KEY: [u8; 32] = [0xB1; 32];
const STORE_CONTEXT: &[u8] = b"apc-runtime-test/multi-domain-recovery/continuum-a";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-runtime-multi-domain-recovery-{}-{id}",
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

fn key(atom_value: u64, domain: &str) -> DomainKey {
    DomainKey::new(atom(atom_value), domain.as_bytes()).unwrap()
}

fn pending_domain() -> LocalScalarDomain<Vec<u8>> {
    let mut causal = ScalarRegister::new();
    causal
        .assign(rid(100), b"body-base-secret".to_vec())
        .unwrap();
    let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
    domain
        .begin_epoch(wid(10), b"body-draft-secret".to_vec())
        .unwrap();
    domain
}

fn exposed_domain() -> LocalScalarDomain<Vec<u8>> {
    let mut causal = ScalarRegister::new();
    causal
        .assign(rid(300), b"title-base-secret".to_vec())
        .unwrap();
    let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
    domain
        .begin_epoch(wid(20), b"title-final-secret".to_vec())
        .unwrap();
    domain.seal_local(rid(400)).unwrap();
    domain.finalize(rid(400)).unwrap();
    domain.handoff([rid(400)]).unwrap();
    domain
}

#[test]
fn independent_domains_survive_protected_filesystem_restart_without_cross_domain_semantics() {
    let directory = TestDir::new();
    let codec = DevelopmentMultiScalarTrustedStateCodec;
    let body_key = key(1, "body");
    let title_key = key(1, "title");

    let body = pending_domain();
    let title = exposed_domain();
    let state = LocalScalarRecoveryState::from_domains(BTreeMap::from([
        (body_key.clone(), body.snapshot()),
        (title_key.clone(), title.snapshot()),
    ]))
    .unwrap();

    let trusted_state = codec.encode(&state).unwrap();
    let record = DurableSyncRecord::new(trusted_state, None);

    let backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let mut store = ProtectedSyncRecordStore::new(
        backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();
    store.persist(&record).unwrap();

    let raw_committed = store.backend().load_committed().unwrap().unwrap();
    for secret in [
        b"body-base-secret".as_slice(),
        b"body-draft-secret".as_slice(),
        b"title-base-secret".as_slice(),
        b"title-final-secret".as_slice(),
        b"APCLSET1".as_slice(),
        b"APCLREC1".as_slice(),
    ] {
        assert!(!raw_committed
            .windows(secret.len())
            .any(|window| window == secret));
    }

    drop(store);

    let reopened_backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let reopened_store = ProtectedSyncRecordStore::new(
        reopened_backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();
    let recovered_record = reopened_store.load_committed().unwrap().unwrap();
    let recovered = codec.decode(recovered_record.trusted_state()).unwrap();

    assert_eq!(recovered, state);
    assert_eq!(recovered.len(), 2);

    let recovered_body = recovered.restore_domain(&body_key).unwrap().unwrap();
    assert_eq!(
        recovered_body.pending().unwrap().value,
        b"body-draft-secret".to_vec()
    );
    assert!(recovered_body.finalization().local_revision_ids().is_empty());
    assert_eq!(recovered_body.causal().frontier_ids(), body.causal().frontier_ids());

    let recovered_title = recovered.restore_domain(&title_key).unwrap().unwrap();
    assert!(recovered_title.pending().is_none());
    assert!(recovered_title
        .finalization()
        .exposed_local_ids()
        .contains(&rid(400)));
    assert!(recovered_title
        .finalization()
        .handed_off_local_ids()
        .contains(&rid(400)));
    assert_eq!(
        recovered_title.causal().frontier_ids(),
        title.causal().frontier_ids()
    );

    // Physical co-persistence of both snapshots is only crash atomicity. The
    // domains retain disjoint causal graphs and independent working/finalization
    // bookkeeping; no cross-domain parent relation is introduced by the wrapper.
    assert!(recovered_body.causal().revision(rid(300)).is_none());
    assert!(recovered_body.causal().revision(rid(400)).is_none());
    assert!(recovered_title.causal().revision(rid(100)).is_none());
}
