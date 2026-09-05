use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use apc_crypto::ContentKey;
use apc_storage_fs::UnixFsDurabilityBackend;
use apc_sync::{
    commit_reconciled_outbox, fetch_from_durable_cursor, publish_staged, stage_outbound,
    DurableSyncRecord, FetchOutcome, OpaqueTransport, ProtectedSyncRecordStore, PublicationId,
    PublishOutcome, TransportCursor, TransportCursorCodec,
};

const KEY_BYTES: [u8; 32] = [0x7b; 32];
const STORE_CONTEXT: &[u8] = b"continuum-7/local-sync-state";
static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apc-durable-sync-session-{}-{id}",
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
struct Revision(u64);

struct RevisionCodec;

impl TransportCursorCodec<Revision> for RevisionCodec {
    type Error = &'static str;

    fn encode(&self, revision: &Revision) -> Result<TransportCursor, Self::Error> {
        TransportCursor::new(revision.0.to_be_bytes().to_vec()).map_err(|_| "encode")
    }

    fn decode(&self, cursor: &TransportCursor) -> Result<Revision, Self::Error> {
        let bytes: [u8; 8] = cursor.as_bytes().try_into().map_err(|_| "decode")?;
        Ok(Revision(u64::from_be_bytes(bytes)))
    }
}

#[derive(Clone, Debug)]
struct TransportCommit {
    revision: Revision,
    objects: Vec<Vec<u8>>,
}

#[derive(Default)]
struct MemoryTransport {
    commits: Vec<TransportCommit>,
    next_revision: u64,
}

impl OpaqueTransport for MemoryTransport {
    type Revision = Revision;
    type Error = &'static str;

    fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
        Ok(self.commits.last().map(|commit| commit.revision))
    }

    fn fetch_since(
        &mut self,
        known_head: Option<&Self::Revision>,
    ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
        let current = self.commits.last().map(|commit| commit.revision);
        if current.as_ref() == known_head {
            return Ok(FetchOutcome::UpToDate { head: current });
        }

        let start = match known_head {
            None => 0,
            Some(known) => match self
                .commits
                .iter()
                .position(|commit| &commit.revision == known)
            {
                Some(index) => index + 1,
                None => return Ok(FetchOutcome::BaselineUnavailable { head: current }),
            },
        };

        let Some(head) = current else {
            return Ok(FetchOutcome::UpToDate { head: None });
        };
        let objects = self.commits[start..]
            .iter()
            .flat_map(|commit| commit.objects.iter().cloned())
            .collect();
        Ok(FetchOutcome::Changed { head, objects })
    }

    fn publish(
        &mut self,
        expected_head: Option<&Self::Revision>,
        objects: &[Vec<u8>],
    ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
        let current = self.commits.last().map(|commit| commit.revision);
        if current.as_ref() != expected_head {
            return Ok(PublishOutcome::Conflict {
                current_head: current,
            });
        }

        self.next_revision += 1;
        let revision = Revision(self.next_revision);
        self.commits.push(TransportCommit {
            revision,
            objects: objects.to_vec(),
        });
        Ok(PublishOutcome::Published { head: revision })
    }
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
fn remote_acceptance_before_local_ack_survives_restart_without_guessing() {
    let directory = TestDir::new();
    let codec = RevisionCodec;
    let mut transport = MemoryTransport::default();

    let baseline_head = match transport.publish(None, &[b"baseline".to_vec()]).unwrap() {
        PublishOutcome::Published { head } => head,
        other => panic!("unexpected baseline publication: {other:?}"),
    };
    let baseline_cursor = codec.encode(&baseline_head).unwrap();

    let mut record = DurableSyncRecord::new(b"local-baseline".to_vec(), Some(baseline_cursor));
    let mut store = open_store(directory.path());
    stage_outbound(
        &mut record,
        &mut store,
        b"local-exposed".to_vec(),
        pid(9),
        vec![b"exact-protected-publication".to_vec()],
    )
    .unwrap();

    let accepted_head = match publish_staged(&record, pid(9), &mut transport, &codec).unwrap() {
        PublishOutcome::Published { head } => head,
        other => panic!("unexpected first publication: {other:?}"),
    };

    // Simulate application/process loss after the remote side accepted the
    // publication but before the local success path could persist an ACK.
    drop(store);
    drop(record);

    let mut store = open_store(directory.path());
    let mut restarted = store.load_committed().unwrap().unwrap();
    assert_eq!(restarted.trusted_state(), b"local-exposed");
    assert!(restarted.outbox().contains_key(&pid(9)));
    assert_eq!(
        restarted.outbox().get(&pid(9)).unwrap().objects(),
        &[b"exact-protected-publication".to_vec()]
    );
    assert_eq!(
        codec.decode(restarted.applied_cursor().unwrap()).unwrap(),
        baseline_head
    );

    // Retrying the exact durable bytes against the old expected head does not
    // duplicate or overwrite the accepted publication. It becomes a conflict,
    // which is evidence that reconciliation is required.
    assert_eq!(
        publish_staged(&restarted, pid(9), &mut transport, &codec).unwrap(),
        PublishOutcome::Conflict {
            current_head: Some(accepted_head)
        }
    );

    let fetched = fetch_from_durable_cursor(&restarted, &mut transport, &codec).unwrap();
    let FetchOutcome::Changed { head, objects } = fetched else {
        panic!("lost-ACK recovery must refetch from the durable baseline cursor")
    };
    assert_eq!(head, accepted_head);
    assert_eq!(objects, vec![b"exact-protected-publication".to_vec()]);

    commit_reconciled_outbox(
        &mut restarted,
        &mut store,
        &codec,
        pid(9),
        b"reconciled-state".to_vec(),
        &head,
    )
    .unwrap();
    drop(store);

    let store = open_store(directory.path());
    let recovered = store.load_committed().unwrap().unwrap();
    assert_eq!(recovered.trusted_state(), b"reconciled-state");
    assert!(recovered.outbox().is_empty());
    assert_eq!(
        codec.decode(recovered.applied_cursor().unwrap()).unwrap(),
        accepted_head
    );
}
