use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use apc_crypto::ContentKey;
use apc_storage_fs::UnixFsDurabilityBackend;
use apc_sync::{
    commit_reconciled_outbox, fetch_from_durable_cursor, publish_staged, stage_outbound,
    DurableSyncRecord, FetchOutcome, ProtectedSyncRecordStore, PublicationId, PublishOutcome,
    TransportCursor, TransportCursorCodec,
};
use apc_transport_github::{
    GitHubApi, GitHubCommitInfo, GitHubCommitOid, GitHubCreateCommitResult, GitHubFileAddition,
    GitHubFileChange, GitHubFileChangeKind, GitHubTransport,
};

const KEY_BYTES: [u8; 32] = [0x84; 32];
const STORE_CONTEXT: &[u8] = b"continuum-84/github-sync-session";
static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-durable-github-session-{}-{id}",
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FakeApiError(&'static str);

impl core::fmt::Display for FakeApiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug)]
struct FakeCommit {
    info: GitHubCommitInfo,
    files: BTreeMap<String, Vec<u8>>,
}

struct FakeGitHubApi {
    head: GitHubCommitOid,
    commits: BTreeMap<GitHubCommitOid, FakeCommit>,
    next_oid: u64,
}

impl FakeGitHubApi {
    fn seeded() -> Self {
        let root = oid(1);
        let info = GitHubCommitInfo {
            oid: root.clone(),
            parents: Vec::new(),
            changes: Vec::new(),
        };
        Self {
            head: root.clone(),
            commits: BTreeMap::from([(
                root,
                FakeCommit {
                    info,
                    files: BTreeMap::new(),
                },
            )]),
            next_oid: 1,
        }
    }
}

impl GitHubApi for FakeGitHubApi {
    type Error = FakeApiError;

    fn branch_head(&mut self) -> Result<Option<GitHubCommitOid>, Self::Error> {
        Ok(Some(self.head.clone()))
    }

    fn create_commit_on_branch(
        &mut self,
        expected_head: &GitHubCommitOid,
        additions: &[GitHubFileAddition],
        _message: &str,
    ) -> Result<GitHubCreateCommitResult, Self::Error> {
        if &self.head != expected_head {
            return Ok(GitHubCreateCommitResult::HeadChanged {
                current_head: Some(self.head.clone()),
            });
        }

        let parent = self.head.clone();
        let mut files = self
            .commits
            .get(&parent)
            .ok_or(FakeApiError("missing parent"))?
            .files
            .clone();
        let mut changes = Vec::new();
        for addition in additions {
            let kind = if files.contains_key(&addition.path) {
                GitHubFileChangeKind::Modified
            } else {
                GitHubFileChangeKind::Added
            };
            files.insert(addition.path.clone(), addition.bytes.clone());
            changes.push(GitHubFileChange {
                path: addition.path.clone(),
                kind,
            });
        }

        self.next_oid += 1;
        let head = oid(self.next_oid);
        let info = GitHubCommitInfo {
            oid: head.clone(),
            parents: vec![parent],
            changes,
        };
        self.commits
            .insert(head.clone(), FakeCommit { info, files });
        self.head = head.clone();
        Ok(GitHubCreateCommitResult::Created { head })
    }

    fn commit_info(&mut self, oid: &GitHubCommitOid) -> Result<GitHubCommitInfo, Self::Error> {
        self.commits
            .get(oid)
            .map(|commit| commit.info.clone())
            .ok_or(FakeApiError("unknown commit"))
    }

    fn read_file_at(&mut self, oid: &GitHubCommitOid, path: &str) -> Result<Vec<u8>, Self::Error> {
        self.commits
            .get(oid)
            .and_then(|commit| commit.files.get(path))
            .cloned()
            .ok_or(FakeApiError("unknown file"))
    }
}

struct GitHubCursorCodec;

impl TransportCursorCodec<GitHubCommitOid> for GitHubCursorCodec {
    type Error = &'static str;

    fn encode(&self, revision: &GitHubCommitOid) -> Result<TransportCursor, Self::Error> {
        TransportCursor::new(revision.as_str().as_bytes().to_vec()).map_err(|_| "encode")
    }

    fn decode(&self, cursor: &TransportCursor) -> Result<GitHubCommitOid, Self::Error> {
        let text = std::str::from_utf8(cursor.as_bytes()).map_err(|_| "non-UTF8 GitHub cursor")?;
        GitHubCommitOid::new(text.to_owned()).map_err(|_| "invalid GitHub cursor")
    }
}

fn oid(value: u64) -> GitHubCommitOid {
    GitHubCommitOid::new(format!("{value:040x}")).unwrap()
}

fn pid(value: u64) -> PublicationId {
    let mut bytes = [0_u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    PublicationId::from_bytes(bytes)
}

fn open_store(path: &Path) -> ProtectedSyncRecordStore<UnixFsDurabilityBackend> {
    let backend = UnixFsDurabilityBackend::open(path).unwrap();
    ProtectedSyncRecordStore::new(
        backend,
        ContentKey::from_bytes(KEY_BYTES),
        STORE_CONTEXT.to_vec(),
    )
    .unwrap()
}

#[test]
fn github_accept_then_local_restart_recovers_via_conflict_and_refetch() {
    let directory = TestDir::new();
    let codec = GitHubCursorCodec;
    let mut transport = GitHubTransport::new(FakeGitHubApi::seeded());
    let baseline_head = apc_sync::OpaqueTransport::head(&mut transport)
        .unwrap()
        .unwrap();
    let baseline_cursor = codec.encode(&baseline_head).unwrap();

    let mut record = DurableSyncRecord::new(b"local-baseline".to_vec(), Some(baseline_cursor));
    let mut store = open_store(directory.path());
    let wire = b"opaque-authenticated-wire-object".to_vec();
    stage_outbound(
        &mut record,
        &mut store,
        b"local-exposed".to_vec(),
        pid(84),
        vec![wire.clone()],
    )
    .unwrap();

    let accepted_head = match publish_staged(&record, pid(84), &mut transport, &codec).unwrap() {
        PublishOutcome::Published { head } => head,
        other => panic!("unexpected GitHub publication result: {other:?}"),
    };

    // Remote GitHub state survives; local process loses everything after its
    // last durable record, including the successful network return value.
    drop(store);
    drop(record);
    let remote_api = transport.into_api();
    let mut transport = GitHubTransport::new(remote_api);

    let mut store = open_store(directory.path());
    let mut restarted = store.load_committed().unwrap().unwrap();
    assert_eq!(restarted.trusted_state(), b"local-exposed");
    assert_eq!(
        restarted.outbox().get(&pid(84)).unwrap().objects(),
        std::slice::from_ref(&wire)
    );
    assert_eq!(
        codec.decode(restarted.applied_cursor().unwrap()).unwrap(),
        baseline_head
    );

    assert_eq!(
        publish_staged(&restarted, pid(84), &mut transport, &codec).unwrap(),
        PublishOutcome::Conflict {
            current_head: Some(accepted_head.clone())
        }
    );

    let FetchOutcome::Changed { head, objects } =
        fetch_from_durable_cursor(&restarted, &mut transport, &codec).unwrap()
    else {
        panic!("GitHub lost-ACK recovery must refetch from durable cursor")
    };
    assert_eq!(head, accepted_head);
    assert_eq!(objects, vec![wire]);

    commit_reconciled_outbox(
        &mut restarted,
        &mut store,
        &codec,
        pid(84),
        b"reconciled-github-state".to_vec(),
        &head,
    )
    .unwrap();
    drop(store);

    let store = open_store(directory.path());
    let recovered = store.load_committed().unwrap().unwrap();
    assert_eq!(recovered.trusted_state(), b"reconciled-github-state");
    assert!(recovered.outbox().is_empty());
    assert_eq!(
        codec.decode(recovered.applied_cursor().unwrap()).unwrap(),
        accepted_head
    );
}
