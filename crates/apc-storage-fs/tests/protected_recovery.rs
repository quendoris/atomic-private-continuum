#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use apc_core::{
    commit_durable, DurabilityBackend, LocalScalarDomain, RevisionId, ScalarRegister,
    WorkingEpochId,
};
use apc_crypto::{protect, unprotect, ContentKey, ProtectionError};
use apc_storage_fs::{
    decode_local_scalar_snapshot, encode_local_scalar_snapshot, UnixFsDurabilityBackend,
};

const RECOVERY_CONTEXT: &[u8] = b"apc-test/local-recovery/continuum-0001";
const TEST_KEY: [u8; 32] = [0x5a; 32];

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-protected-recovery-{}-{id}",
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

fn id_bytes(value: u64) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    bytes
}

fn rid(value: u64) -> RevisionId {
    RevisionId::from_bytes(id_bytes(value))
}

fn wid(value: u64) -> WorkingEpochId {
    WorkingEpochId::from_bytes(id_bytes(value))
}

fn recovery_domain() -> LocalScalarDomain<Vec<u8>> {
    let mut causal = ScalarRegister::new();
    causal.assign(rid(100), b"base-value".to_vec()).unwrap();
    let mut domain = LocalScalarDomain::from_causal(causal).unwrap();

    domain
        .begin_epoch(wid(1), b"committed-local".to_vec())
        .unwrap();
    domain.seal_local(rid(200)).unwrap();
    domain.finalize(rid(200)).unwrap();
    domain.handoff([rid(200)]).unwrap();

    domain
        .begin_epoch(wid(2), b"draft-in-memory".to_vec())
        .unwrap();
    domain
        .update_pending(b"draft-latest-sensitive".to_vec())
        .unwrap();
    domain
}

#[test]
fn real_scalar_recovery_state_is_aead_protected_before_filesystem_storage() {
    let directory = TestDir::new();
    let domain = recovery_domain();
    let snapshot_bytes = encode_local_scalar_snapshot(&domain.snapshot()).unwrap();
    let key = ContentKey::from_bytes(TEST_KEY);
    let protected = protect(&key, RECOVERY_CONTEXT, &snapshot_bytes).unwrap();

    assert!(!protected
        .windows(b"draft-latest-sensitive".len())
        .any(|window| window == b"draft-latest-sensitive"));

    let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    commit_durable(&mut backend, &protected).unwrap();
    drop(backend);

    let reopened = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let recovered_protected = reopened.load_committed().unwrap().unwrap();
    let recovered_plain = unprotect(&key, RECOVERY_CONTEXT, &recovered_protected).unwrap();
    let recovered_snapshot = decode_local_scalar_snapshot(&recovered_plain).unwrap();
    let restored = LocalScalarDomain::restore(recovered_snapshot).unwrap();

    assert_eq!(restored, domain);
}

#[test]
fn storage_bytes_cannot_be_reused_under_a_different_recovery_context() {
    let directory = TestDir::new();
    let domain = recovery_domain();
    let encoded = encode_local_scalar_snapshot(&domain.snapshot()).unwrap();
    let key = ContentKey::from_bytes(TEST_KEY);
    let protected = protect(&key, RECOVERY_CONTEXT, &encoded).unwrap();

    let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    commit_durable(&mut backend, &protected).unwrap();
    drop(backend);

    let reopened = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let recovered = reopened.load_committed().unwrap().unwrap();
    assert_eq!(
        unprotect(&key, b"apc-test/local-recovery/other-continuum", &recovered).unwrap_err(),
        ProtectionError::AuthenticationFailed
    );
}
