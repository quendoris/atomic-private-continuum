use std::collections::BTreeMap;
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
    catch_up_scalar_recovery_state, DevelopmentMultiScalarTrustedStateCodec,
    LocalScalarRecoveryState, ScalarRecoveryCatchUpOutcome, ScalarRecoveryCatchUpSpec,
    TrustedStateCodec,
};
use apc_storage_fs::UnixFsDurabilityBackend;
use apc_sync::{
    encode_protected_sync_part, protect_scalar_part, DomainKey, DurableSyncRecord, FetchOutcome,
    OpaqueTransport, ProtectedSyncRecordStore, PublicationId, PublishOutcome, SyncProjection,
    SyncRecordStore, TransportCursor, TransportCursorCodec,
};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

const STORE_KEY: [u8; 32] = [0xD1; 32];
const PUBLICATION_KEY: [u8; 32] = [0xD2; 32];
const STORE_CONTEXT: &[u8] = b"apc-runtime-test/multi-domain-catchup/continuum-a";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-runtime-multi-domain-catchup-{}-{id}",
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

struct RangeTransport {
    head: TransportRevision,
    objects: Vec<Vec<u8>>,
}

impl OpaqueTransport for RangeTransport {
    type Revision = TransportRevision;
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
        Err("publish unused")
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

fn dirty_domain(
    base_revision: u64,
    epoch: u64,
    base: &str,
    draft: &str,
) -> LocalScalarDomain<Vec<u8>> {
    let mut causal = ScalarRegister::new();
    causal
        .assign(rid(base_revision), base.as_bytes().to_vec())
        .unwrap();
    let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
    domain
        .begin_epoch(wid(epoch), draft.as_bytes().to_vec())
        .unwrap();
    domain
}

fn remote_from_base(
    base_revision: u64,
    remote_revision: u64,
    base: &str,
    value: &str,
) -> ScalarRegister<Vec<u8>> {
    let mut remote = ScalarRegister::new();
    remote
        .assign(rid(base_revision), base.as_bytes().to_vec())
        .unwrap();
    remote
        .assign(rid(remote_revision), value.as_bytes().to_vec())
        .unwrap();
    remote
}

#[test]
fn multi_domain_authenticated_catchup_survives_encrypted_filesystem_restart() {
    let directory = TestDir::new();
    let codec = DevelopmentMultiScalarTrustedStateCodec;
    let cursor_codec = RevisionCodec;
    let body_key = domain("body");
    let title_key = domain("title");
    let notes_key = domain("notes");

    let body = dirty_domain(100, 1, "body-base-secret", "body-draft-secret");
    let title = dirty_domain(300, 2, "title-base-secret", "title-draft-secret");
    let notes = dirty_domain(500, 3, "notes-base-secret", "notes-draft-secret");
    let original_notes = notes.snapshot();

    let mut recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
        (body_key.clone(), body.snapshot()),
        (title_key.clone(), title.snapshot()),
        (notes_key.clone(), original_notes.clone()),
    ]))
    .unwrap();

    let remote_projection = SyncProjection::from_domains(BTreeMap::from([
        (
            body_key.clone(),
            remote_from_base(100, 900, "body-base-secret", "remote-body-secret"),
        ),
        (
            title_key.clone(),
            remote_from_base(300, 901, "title-base-secret", "remote-title-secret"),
        ),
    ]));
    let publication_key = ContentKey::from_bytes(PUBLICATION_KEY);
    let protected =
        protect_scalar_part(&publication_key, cid(1), pid(1), 0, 1, &remote_projection).unwrap();
    let mut transport = RangeTransport {
        head: TransportRevision(2),
        objects: vec![encode_protected_sync_part(&protected).unwrap()],
    };

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
    let pre_observation =
        BTreeMap::from([(body_key.clone(), rid(200)), (title_key.clone(), rid(400))]);

    let outcome = catch_up_scalar_recovery_state(
        &mut recovery,
        &mut record,
        &mut store,
        &codec,
        &cursor_codec,
        &mut transport,
        ScalarRecoveryCatchUpSpec::new(&publication_key, cid(1), &pre_observation),
    )
    .unwrap();

    assert!(matches!(
        outcome,
        ScalarRecoveryCatchUpOutcome::Applied {
            head: TransportRevision(2),
            object_count: 1,
            ..
        }
    ));

    let raw_committed = store.backend().load_committed().unwrap().unwrap();
    for secret in [
        b"body-draft-secret".as_slice(),
        b"title-draft-secret".as_slice(),
        b"notes-draft-secret".as_slice(),
        b"remote-body-secret".as_slice(),
        b"remote-title-secret".as_slice(),
        b"APCLSET1".as_slice(),
    ] {
        assert!(!raw_committed
            .windows(secret.len())
            .any(|window| window == secret));
    }

    drop(store);
    drop(record);
    drop(recovery);

    let reopened_backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
    let reopened_store = ProtectedSyncRecordStore::new(
        reopened_backend,
        ContentKey::from_bytes(STORE_KEY),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap();
    let recovered_record = reopened_store.load_committed().unwrap().unwrap();
    let recovered = codec.decode(recovered_record.trusted_state()).unwrap();

    assert_eq!(
        cursor_codec
            .decode(recovered_record.applied_cursor().unwrap())
            .unwrap(),
        TransportRevision(2)
    );

    let recovered_body = recovered.restore_domain(&body_key).unwrap().unwrap();
    assert!(recovered_body.pending().is_none());
    assert_eq!(
        recovered_body.causal().frontier_ids(),
        [rid(200), rid(900)].into_iter().collect()
    );

    let recovered_title = recovered.restore_domain(&title_key).unwrap().unwrap();
    assert!(recovered_title.pending().is_none());
    assert_eq!(
        recovered_title.causal().frontier_ids(),
        [rid(400), rid(901)].into_iter().collect()
    );

    let recovered_notes = recovered.restore_domain(&notes_key).unwrap().unwrap();
    assert_eq!(recovered.get(&notes_key), Some(&original_notes));
    assert_eq!(recovered_notes.pending().unwrap().id, wid(3));

    assert!(recovered_body.causal().revision(rid(400)).is_none());
    assert!(recovered_body.causal().revision(rid(901)).is_none());
    assert!(recovered_title.causal().revision(rid(200)).is_none());
    assert!(recovered_title.causal().revision(rid(900)).is_none());
}
