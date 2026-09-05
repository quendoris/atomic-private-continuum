use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use apc_core::{AtomId, RevisionId};
use apc_sync::{decode_scalar_projection, DomainKey};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-sync-process-exchange-{}-{id}",
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

fn bytes(value: u64) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    bytes
}

fn atom(value: u64) -> AtomId {
    AtomId::from_bytes(bytes(value))
}

fn rid(value: u64) -> RevisionId {
    RevisionId::from_bytes(bytes(value))
}

fn body_key() -> DomainKey {
    DomainKey::new(atom(1), b"body".to_vec()).unwrap()
}

fn run_worker(args: &[&Path], mode: &str) {
    let worker = env!("CARGO_BIN_EXE_apc-sync-process-worker");
    let mut command = Command::new(worker);
    command.arg(mode);
    for arg in args {
        command.arg(arg);
    }
    let status = command.status().unwrap();
    assert!(status.success(), "sync process worker {mode} failed: {status}");
}

#[test]
fn independent_processes_exchange_authenticated_state_and_converge() {
    let directory = TestDir::new();
    let left_payload = directory.path().join("left.payload");
    let right_payload = directory.path().join("right.payload");
    let left_result = directory.path().join("left.result");
    let right_result = directory.path().join("right.result");

    run_worker(&[&left_payload], "emit-left");
    run_worker(&[&right_payload], "emit-right");

    let left_protected = fs::read(&left_payload).unwrap();
    let right_protected = fs::read(&right_payload).unwrap();
    assert!(!left_protected
        .windows(b"left".len())
        .any(|window| window == b"left"));
    assert!(!right_protected
        .windows(b"right".len())
        .any(|window| window == b"right"));

    run_worker(
        &[&left_result, &left_payload, &right_payload],
        "merge-left",
    );
    run_worker(
        &[&right_result, &left_payload, &right_payload],
        "merge-right",
    );

    let left_bytes = fs::read(&left_result).unwrap();
    let right_bytes = fs::read(&right_result).unwrap();
    assert_eq!(left_bytes, right_bytes);

    let projection = decode_scalar_projection(&left_bytes).unwrap();
    let register = projection.get(&body_key()).unwrap();
    assert_eq!(
        register.frontier_ids(),
        [rid(10), rid(20)].into_iter().collect()
    );
    assert_eq!(register.materialized(), Some(&b"right".to_vec()));
}
