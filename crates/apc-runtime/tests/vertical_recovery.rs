use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use apc_core::id::LOGICAL_ID_BYTES;
use apc_core::{
    AtomId, ContinuumId, DurabilityBackend, LocalScalarDomain, RevisionId, ScalarRegister,
    WorkingEpochId,
};
use apc_crypto::ContentKey;
use apc_runtime::{
    commit_received_scalar_domain, decode_single_scalar_domain_object, prepare_scalar_handoff,
    recover_scalar_domain, stage_prepared_scalar_handoff, DevelopmentScalarTrustedStateCodec,
    GitHubCursorCodec, ReceivedScalarState, TrustedStateCodec,
};
use apc_storage_fs::UnixFsDurabilityBackend;
use apc_sync::{
    decode_protected_sync_part, unprotect_scalar_part, DomainKey, DurableSyncRecord,
    ProtectedSyncRecordStore, PublicationId, SyncRecordStore, TransportCursorCodec,
};
use apc_transport_github::GitHubCommitOid;

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

const STORE_KEY: [u8; 32] = [0x71; 32];
const PUBLICATION_KEY: [u8; 32] = [0x72; 32];
const STORE_CONTEXT: &[u8] = b"apc-runtime-test/vertical-recovery/continuum-a";

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
    PublicationId::from_bytes(logical_bytes(value))
}

fn cid(value: u64) -> ContinuumId {
    ContinuumId::from_bytes(logical_bytes(value))
}

fn atom(value: u64) -> AtomId {
    AtomId::from_bytes(logical_bytes(value))
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

fn shared_base() -> ScalarRegister<Vec<u8>> {
    let mut causal = ScalarRegister::new();
    causal.assign(rid(100), b"shared-base".to_vec()).unwrap();
    causal
}

fn inbound_sender() -> LocalScalarDomain<Vec<u8>> {
    let mut domain = LocalScalarDomain::from_causal(shared_base()).unwrap();
    domain
        .begin_epoch(wid(90), b"remote-observed-value".to_vec())
        .unwrap();
    domain.seal_local(rid(900)).unwrap();
    domain.finalize(rid(900)).unwrap();
    domain
}

fn dirty_inbound_receiver() -> LocalScalarDomain<Vec<u8>> {
    let mut domain = LocalScalarDomain::from_causal(shared_base()).unwrap();
    domain
        .begin_epoch(wid(20), b"local-draft-before-remote".to_vec())
        .unwrap();
    domain
        .update_pending(b"local-draft-latest-sensitive".to_vec())
        .unwrap();
    domain
}

#[test]
fn semantic_exposure_survives_real_sync_crypto_filesystem_restart_and_exact_outbox_retry() {
    let directory = TestDir::new();
    let codec = DevelopmentScalarTrustedStateCodec;
    let cursor_codec = GitHubCursorCodec;
    let transport_head = GitHubCommitOid::new("0123456789abcdef0123456789abcdef01234567").unwrap();
    let cursor = cursor_codec.encode(&transport_head).unwrap();

    let mut domain = prepared_domain();
    let initial_trusted_state = codec.encode(&domain.snapshot()).unwrap();
    let mut record = DurableSyncRecord::new(initial_trusted_state, Some(cursor.clone()));

    let semantic_key = DomainKey::new(atom(1), b"body".to_vec()).unwrap();
    let publication_key = ContentKey::from_bytes(PUBLICATION_KEY);
    let prepared = prepare_scalar_handoff(
        &domain,
        semantic_key.clone(),
        cid(1),
        pid(1),
        &publication_key,
        [rid(200)],
    )
    .unwrap();
    let protected_object = prepared.publication().objects()[0].clone();

    let backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let mut store = ProtectedSyncRecordStore::new(
        backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();

    stage_prepared_scalar_handoff(&mut domain, &mut record, &mut store, &codec, prepared).unwrap();

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
    assert!(!raw_committed
        .windows(b"local-finalized".len())
        .any(|window| window == b"local-finalized"));

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

    let part = decode_protected_sync_part(&recovered_outbox.objects()[0]).unwrap();
    let projection =
        unprotect_scalar_part(&ContentKey::from_bytes(PUBLICATION_KEY), cid(1), &part).unwrap();
    let published = projection.get(&semantic_key).unwrap();

    assert!(published.revision(rid(100)).is_some());
    assert!(published.revision(rid(200)).is_some());
    assert_eq!(published.len(), 2);
    assert_eq!(
        published.materialized().map(Vec::as_slice),
        Some(b"local-finalized".as_slice())
    );
}

#[test]
fn dirty_remote_observation_survives_real_crypto_filesystem_restart_with_true_concurrency() {
    let directory = TestDir::new();
    let codec = DevelopmentScalarTrustedStateCodec;
    let cursor_codec = GitHubCursorCodec;
    let old_head = GitHubCommitOid::new("1111111111111111111111111111111111111111").unwrap();
    let new_head = GitHubCommitOid::new("2222222222222222222222222222222222222222").unwrap();
    let old_cursor = cursor_codec.encode(&old_head).unwrap();

    let semantic_key = DomainKey::new(atom(2), b"title".to_vec()).unwrap();
    let publication_key = ContentKey::from_bytes(PUBLICATION_KEY);
    let sender = inbound_sender();
    let prepared_remote = prepare_scalar_handoff(
        &sender,
        semantic_key.clone(),
        cid(2),
        pid(2),
        &publication_key,
        [rid(900)],
    )
    .unwrap();
    let remote = decode_single_scalar_domain_object(
        &publication_key,
        cid(2),
        &semantic_key,
        &prepared_remote.publication().objects()[0],
    )
    .unwrap();

    let mut receiver = dirty_inbound_receiver();
    let initial_trusted_state = codec.encode(&receiver.snapshot()).unwrap();
    let mut record = DurableSyncRecord::new(initial_trusted_state, Some(old_cursor));

    let backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let mut store = ProtectedSyncRecordStore::new(
        backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();
    store.persist(&record).unwrap();

    let sealed = commit_received_scalar_domain(
        &mut receiver,
        &mut record,
        &mut store,
        &codec,
        &cursor_codec,
        ReceivedScalarState::new(&remote, Some(rid(200)), &new_head),
    )
    .unwrap()
    .unwrap();

    assert_eq!(sealed.parents, BTreeSet::from([rid(100)]));
    assert_eq!(
        receiver.causal().frontier_ids(),
        BTreeSet::from([rid(200), rid(900)])
    );
    assert!(!receiver.causal().is_ancestor(rid(200), rid(900)));
    assert!(!receiver.causal().is_ancestor(rid(900), rid(200)));
    assert!(receiver
        .finalization()
        .local_revision_ids()
        .contains(&rid(200)));
    assert!(receiver.pending().is_none());
    assert!(record.outbox().is_empty());
    assert_eq!(
        cursor_codec.decode(record.applied_cursor().unwrap()).unwrap(),
        new_head
    );

    let raw_committed = store.backend().load_committed().unwrap().unwrap();
    assert!(!raw_committed
        .windows(b"local-draft-latest-sensitive".len())
        .any(|window| window == b"local-draft-latest-sensitive"));
    assert!(!raw_committed
        .windows(b"remote-observed-value".len())
        .any(|window| window == b"remote-observed-value"));
    assert!(!raw_committed
        .windows(b"APCLREC1".len())
        .any(|window| window == b"APCLREC1"));

    let expected = receiver.clone();
    drop(store);
    drop(record);
    drop(receiver);

    let reopened_backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let reopened_store = ProtectedSyncRecordStore::new(
        reopened_backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();
    let recovered_record = reopened_store.load_committed().unwrap().unwrap();
    let recovered = recover_scalar_domain(&recovered_record, &codec).unwrap();

    assert_eq!(recovered, expected);
    assert_eq!(
        recovered.causal().frontier_ids(),
        BTreeSet::from([rid(200), rid(900)])
    );
    assert_eq!(
        recovered.causal().revision(rid(200)).unwrap().parents,
        BTreeSet::from([rid(100)])
    );
    assert!(!recovered.causal().is_ancestor(rid(200), rid(900)));
    assert!(!recovered.causal().is_ancestor(rid(900), rid(200)));
    assert_eq!(
        cursor_codec
            .decode(recovered_record.applied_cursor().unwrap())
            .unwrap(),
        new_head
    );
}
