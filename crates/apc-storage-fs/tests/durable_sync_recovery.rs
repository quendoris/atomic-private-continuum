use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use apc_core::{commit_durable, DurabilityBackend};
use apc_crypto::{protect, unprotect, ContentKey};
use apc_storage_fs::UnixFsDurabilityBackend;
use apc_sync::{
    decode_durable_sync_record, encode_durable_sync_record, DurableSyncRecord, PublicationId,
    TransportCursor,
};

const KEY_BYTES: [u8; 32] = [0x6a; 32];
const CONTEXT: &[u8] = b"apc-test/durable-sync-recovery/v1";
static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-durable-sync-recovery-{}-{id}",
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

fn cursor(value: &str) -> TransportCursor {
    TransportCursor::new(value.as_bytes().to_vec()).unwrap()
}

fn pid(value: u64) -> PublicationId {
    let mut bytes = [0_u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    PublicationId::from_bytes(bytes)
}

fn protect_record(record: &DurableSyncRecord) -> Vec<u8> {
    let encoded = encode_durable_sync_record(record).unwrap();
    protect(&ContentKey::from_bytes(KEY_BYTES), CONTEXT, &encoded).unwrap()
}

fn unprotect_record(bytes: &[u8]) -> DurableSyncRecord {
    let encoded = unprotect(&ContentKey::from_bytes(KEY_BYTES), CONTEXT, bytes).unwrap();
    decode_durable_sync_record(&encoded).unwrap()
}

fn reopen_record(path: &Path) -> DurableSyncRecord {
    let backend = UnixFsDurabilityBackend::open(path).unwrap();
    let protected = backend.load_committed().unwrap().unwrap();
    unprotect_record(&protected)
}

#[test]
fn durable_candidate_without_root_publication_cannot_advance_transport_cursor() {
    let directory = TestDir::new();
    let original = DurableSyncRecord::new(b"old-trusted-state".to_vec(), Some(cursor("R0")));
    let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    commit_durable(&mut backend, &protect_record(&original)).unwrap();

    let mut received = original.clone();
    received.apply_received(b"merged-trusted-state".to_vec(), cursor("R1"));
    let protected_received = protect_record(&received);

    let candidate = backend.write_candidate(&protected_received).unwrap();
    backend.sync_candidate(&candidate).unwrap();
    drop(backend);

    let recovered = reopen_record(directory.path());
    assert_eq!(recovered, original);
    assert_eq!(recovered.trusted_state(), b"old-trusted-state");
    assert_eq!(recovered.applied_cursor(), Some(&cursor("R0")));

    let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    commit_durable(&mut backend, &protected_received).unwrap();
    drop(backend);

    let recovered = reopen_record(directory.path());
    assert_eq!(recovered, received);
    assert_eq!(recovered.trusted_state(), b"merged-trusted-state");
    assert_eq!(recovered.applied_cursor(), Some(&cursor("R1")));
}

#[test]
fn durable_outbox_survives_restart_and_reuses_exact_protected_wire_bytes() {
    let directory = TestDir::new();
    let wire_objects = vec![
        b"already-protected-wire-object-A".to_vec(),
        b"already-protected-wire-object-B".to_vec(),
    ];

    let mut record = DurableSyncRecord::new(b"local-before-handoff".to_vec(), Some(cursor("R0")));
    record
        .prepare_outbox(
            b"local-with-durable-exposure".to_vec(),
            pid(7),
            Some(cursor("R0")),
            wire_objects.clone(),
        )
        .unwrap();

    let protected = protect_record(&record);
    assert!(!protected
        .windows(b"local-with-durable-exposure".len())
        .any(|window| window == b"local-with-durable-exposure"));

    let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    commit_durable(&mut backend, &protected).unwrap();
    drop(backend);

    let mut recovered = reopen_record(directory.path());
    let outbox = recovered.outbox().get(&pid(7)).unwrap();
    assert_eq!(recovered.trusted_state(), b"local-with-durable-exposure");
    assert_eq!(recovered.applied_cursor(), Some(&cursor("R0")));
    assert_eq!(outbox.objects(), wire_objects.as_slice());

    recovered.apply_received(b"merged-after-fetch".to_vec(), cursor("R1"));
    let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    commit_durable(&mut backend, &protect_record(&recovered)).unwrap();
    drop(backend);

    let mut recovered = reopen_record(directory.path());
    assert_eq!(recovered.trusted_state(), b"merged-after-fetch");
    assert_eq!(recovered.applied_cursor(), Some(&cursor("R1")));
    assert_eq!(
        recovered.outbox().get(&pid(7)).unwrap().objects(),
        wire_objects.as_slice()
    );

    recovered
        .retire_outbox(pid(7), b"reconciled-after-publish".to_vec(), cursor("R2"))
        .unwrap();
    let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    commit_durable(&mut backend, &protect_record(&recovered)).unwrap();
    drop(backend);

    let recovered = reopen_record(directory.path());
    assert_eq!(recovered.trusted_state(), b"reconciled-after-publish");
    assert_eq!(recovered.applied_cursor(), Some(&cursor("R2")));
    assert!(recovered.outbox().is_empty());
}
