use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use apc_core::id::LOGICAL_ID_BYTES;
use apc_core::{DurabilityBackend, LocalScalarDomain, RevisionId, ScalarRegister, WorkingEpochId};
use apc_crypto::{protect, unprotect, ContentKey};
use apc_runtime::{
    recover_scalar_domain, stage_scalar_handoff, DevelopmentScalarTrustedStateCodec,
    GitHubCursorCodec, ProtectedPublication, TrustedStateCodec,
};
use apc_storage_fs::UnixFsDurabilityBackend;
use apc_sync::{DurableSyncRecord, ProtectedSyncRecordStore, PublicationId, TransportCursorCodec};
use apc_transport_github::GitHubCommitOid;

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

const STORE_KEY: [u8; 32] = [0x71; 32];
const PUBLICATION_KEY: [u8; 32] = [0x72; 32];
const STORE_CONTEXT: &[u8] = b"apc-runtime-test/vertical-recovery/continuum-a";
const PUBLICATION_CONTEXT: &[u8] = b"apc-runtime-test/protected-publication/continuum-a";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-runtime-vertical-recovery-{}-{id}",
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

fn rid(value: u64) -> RevisionId {
    RevisionId::from_bytes(logical_bytes(value))
}

fn wid(value: u64) -> WorkingEpochId {
    WorkingEpochId::from_bytes(logical_bytes(value))
}

fn pid(value: u64) -> PublicationId {
    let mut bytes = [0_u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    PublicationId::from_bytes(bytes)
}

fn prepared_domain() -> LocalScalarDomain<Vec<u8>> {
    let mut causal = ScalarRegister::new();
    causal.assign(rid(100), b"remote-base".to_vec()).unwrap();

    let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
    domain
        .begin_epoch(wid(1), b"local-finalized".to_vec())
        .unwrap();
    domain.seal_local(rid(200)).unwrap();
    domain.finalize(rid(200)).unwrap();

    domain
        .begin_epoch(wid(2), b"pending-before-crash".to_vec())
        .unwrap();
    domain
        .update_pending(b"pending-latest-secret".to_vec())
        .unwrap();
    domain
}

#[test]
fn semantic_exposure_survives_real_crypto_filesystem_restart_and_exact_outbox_retry() {
    let directory = TestDir::new();
    let codec = DevelopmentScalarTrustedStateCodec;
    let cursor_codec = GitHubCursorCodec;
    let transport_head = GitHubCommitOid::new("0123456789abcdef0123456789abcdef01234567").unwrap();
    let cursor = cursor_codec.encode(&transport_head).unwrap();

    let mut domain = prepared_domain();
    let initial_trusted_state = codec.encode(&domain.snapshot()).unwrap();
    let mut record = DurableSyncRecord::new(initial_trusted_state, Some(cursor.clone()));

    let clear_publication = b"portable-semantic-publication-secret";
    let protected_object = protect(
        &ContentKey::from_bytes(PUBLICATION_KEY),
        PUBLICATION_CONTEXT,
        clear_publication,
    )
    .unwrap();

    let backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let mut store = ProtectedSyncRecordStore::new(
        backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();

    stage_scalar_handoff(
        &mut domain,
        &mut record,
        &mut store,
        &codec,
        [rid(200)],
        ProtectedPublication::new(pid(1), vec![protected_object.clone()]),
    )
    .unwrap();

    assert!(domain
        .finalization()
        .exposed_local_ids()
        .contains(&rid(200)));
    assert_eq!(
        domain.pending().unwrap().value,
        b"pending-latest-secret".to_vec()
    );

    let raw_committed = store.backend().load_committed().unwrap().unwrap();
    assert!(!raw_committed
        .windows(b"pending-latest-secret".len())
        .any(|window| window == b"pending-latest-secret"));
    assert!(!raw_committed
        .windows(b"APCLREC1".len())
        .any(|window| window == b"APCLREC1"));

    drop(store);
    drop(record);
    drop(domain);

    let reopened_backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let reopened_store = ProtectedSyncRecordStore::new(
        reopened_backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();
    let recovered_record = reopened_store.load_committed().unwrap().unwrap();
    let recovered_domain = recover_scalar_domain(&recovered_record, &codec).unwrap();

    assert!(recovered_domain
        .finalization()
        .exposed_local_ids()
        .contains(&rid(200)));
    assert!(recovered_domain
        .finalization()
        .handed_off_local_ids()
        .contains(&rid(200)));
    assert!(recovered_domain
        .finalization()
        .finalized()
        .contains_key(&rid(200)));
    assert_eq!(
        recovered_domain.pending().unwrap().value,
        b"pending-latest-secret".to_vec()
    );

    let recovered_head = cursor_codec
        .decode(recovered_record.applied_cursor().unwrap())
        .unwrap();
    assert_eq!(recovered_head, transport_head);

    let recovered_outbox = recovered_record.outbox().get(&pid(1)).unwrap();
    assert_eq!(recovered_outbox.expected_cursor(), Some(&cursor));
    assert_eq!(
        recovered_outbox.objects(),
        std::slice::from_ref(&protected_object)
    );

    let opened_publication = unprotect(
        &ContentKey::from_bytes(PUBLICATION_KEY),
        PUBLICATION_CONTEXT,
        &recovered_outbox.objects()[0],
    )
    .unwrap();
    assert_eq!(opened_publication, clear_publication);
}
