#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use apc_core::{commit_durable, DurabilityBackend};
use apc_storage_fs::UnixFsDurabilityBackend;

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-storage-process-kill-{}-{id}",
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

#[derive(Clone, Copy)]
enum Expected {
    Old,
    OldOrNew,
    New,
}

#[test]
fn process_kill_matrix_never_recovers_a_partial_state() {
    let cases = [
        ("after-write", Expected::Old),
        ("after-candidate-sync", Expected::Old),
        ("after-publish", Expected::OldOrNew),
        ("after-root-sync", Expected::New),
        ("after-ack", Expected::New),
    ];

    for (stage, expected) in cases {
        run_case(stage, expected);
    }
}

fn run_case(stage: &str, expected: Expected) {
    let directory = TestDir::new();
    let store = directory.path().join("store");
    let marker = directory.path().join("stage-ready");

    let mut backend = UnixFsDurabilityBackend::open(&store).unwrap();
    commit_durable(&mut backend, &b"old-state".to_vec()).unwrap();
    drop(backend);

    let worker = env!("CARGO_BIN_EXE_apc-storage-crash-worker");
    let mut child = Command::new(worker)
        .arg(&store)
        .arg(&marker)
        .arg(stage)
        .arg("new-state")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();

    wait_for_marker(&marker, &mut child, stage);
    child.kill().unwrap();
    let _status = child.wait().unwrap();

    let reopened = UnixFsDurabilityBackend::open(&store).unwrap();
    let recovered = reopened.load_committed().unwrap().unwrap();

    match expected {
        Expected::Old => assert_eq!(recovered, b"old-state", "stage {stage}"),
        Expected::OldOrNew => assert!(
            recovered == b"old-state" || recovered == b"new-state",
            "stage {stage} recovered an unexpected state: {recovered:?}"
        ),
        Expected::New => assert_eq!(recovered, b"new-state", "stage {stage}"),
    }
}

fn wait_for_marker(marker: &Path, child: &mut std::process::Child, stage: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if marker.exists() {
            return;
        }

        if let Some(status) = child.try_wait().unwrap() {
            panic!("crash worker exited before stage {stage}: {status}");
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for crash worker stage {stage}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
