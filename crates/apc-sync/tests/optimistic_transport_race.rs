use std::collections::BTreeSet;

use apc_core::{AtomId, ContinuumId, RevisionId, ScalarRegister};
use apc_crypto::ContentKey;
use apc_sync::{
    protect_scalar_part, DomainKey, MultipartInbox, ProtectedSyncPart, PublicationId,
    ScalarDirtyDomainState, ScalarSyncProjection, SyncProjection,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TransportRevision([u8; 32]);

#[derive(Clone, Debug)]
struct TransportCommit {
    revision: TransportRevision,
    parent: Option<TransportRevision>,
    parts: Vec<ProtectedSyncPart>,
}

#[derive(Clone, Debug, Default)]
struct MemoryCasTransport {
    commits: Vec<TransportCommit>,
    next_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PublishConflict {
    current_head: Option<TransportRevision>,
}

impl MemoryCasTransport {
    fn head(&self) -> Option<TransportRevision> {
        self.commits.last().map(|commit| commit.revision)
    }

    fn publish(
        &mut self,
        expected_head: Option<TransportRevision>,
        parts: Vec<ProtectedSyncPart>,
    ) -> Result<TransportRevision, PublishConflict> {
        if self.head() != expected_head {
            return Err(PublishConflict {
                current_head: self.head(),
            });
        }

        self.next_revision += 1;
        let revision = TransportRevision(bytes(10_000 + self.next_revision));
        self.commits.push(TransportCommit {
            revision,
            parent: expected_head,
            parts,
        });
        Ok(revision)
    }

    fn parts_after(&self, known_head: Option<TransportRevision>) -> Vec<ProtectedSyncPart> {
        let start = match known_head {
            None => 0,
            Some(known) => self
                .commits
                .iter()
                .position(|commit| commit.revision == known)
                .map(|index| index + 1)
                .expect("known transport revision must exist"),
        };

        self.commits[start..]
            .iter()
            .flat_map(|commit| commit.parts.iter().cloned())
            .collect()
    }

    fn validate_linear_parents(&self) {
        let mut previous = None;
        for commit in &self.commits {
            assert_eq!(commit.parent, previous);
            previous = Some(commit.revision);
        }
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

fn cid(value: u64) -> ContinuumId {
    ContinuumId::from_bytes(bytes(value))
}

fn rid(value: u64) -> RevisionId {
    RevisionId::from_bytes(bytes(value))
}

fn pid(value: u64) -> PublicationId {
    PublicationId::from_bytes(bytes(value))
}

fn body_key() -> DomainKey {
    DomainKey::new(atom(1), b"body".to_vec()).unwrap()
}

fn baseline_projection() -> ScalarSyncProjection {
    let mut register = ScalarRegister::new();
    register.assign(rid(1), b"base".to_vec()).unwrap();
    SyncProjection::from_domains([(body_key(), register)].into_iter().collect())
}

fn locally_edit(state: &mut ScalarDirtyDomainState, revision_id: RevisionId, value: &[u8]) {
    let key = body_key();
    let mut register = state.get(&key).unwrap().clone();
    register.assign(revision_id, value.to_vec()).unwrap();
    state.set_local(key, register);
}

fn protect_single(
    key: &ContentKey,
    continuum_id: ContinuumId,
    publication_id: PublicationId,
    projection: &ScalarSyncProjection,
) -> ProtectedSyncPart {
    protect_scalar_part(key, continuum_id, publication_id, 0, 1, projection).unwrap()
}

fn import_parts(
    state: &mut ScalarDirtyDomainState,
    inbox: &mut MultipartInbox,
    key: &ContentKey,
    continuum_id: ContinuumId,
    parts: Vec<ProtectedSyncPart>,
) {
    for part in parts {
        let projection = inbox
            .ingest(key, continuum_id, part)
            .unwrap()
            .expect("all test publications contain one part");
        state.import_projection(&projection).unwrap();
    }
}

#[test]
fn stale_publisher_fetches_remote_state_retains_local_dirty_work_and_retries() {
    let content_key = ContentKey::from_bytes([0x81; 32]);
    let continuum_id = cid(9);
    let baseline = baseline_projection();

    let mut left = ScalarDirtyDomainState::new();
    let mut right = ScalarDirtyDomainState::new();
    left.import_projection(&baseline).unwrap();
    right.import_projection(&baseline).unwrap();
    locally_edit(&mut left, rid(10), b"left");
    locally_edit(&mut right, rid(20), b"right");

    let left_projection = left.export_dirty().unwrap();
    let right_projection = right.export_dirty().unwrap();
    let left_part = protect_single(&content_key, continuum_id, pid(100), &left_projection);
    let right_part = protect_single(&content_key, continuum_id, pid(200), &right_projection);

    let mut transport = MemoryCasTransport::default();
    let left_expected = transport.head();
    let right_expected = transport.head();
    assert_eq!(left_expected, right_expected);

    let left_head = transport
        .publish(left_expected, vec![left_part])
        .expect("first publisher must win empty head");
    left.acknowledge(&left_projection);
    assert!(left.dirty_keys().is_empty());

    let conflict = transport
        .publish(right_expected, vec![right_part])
        .unwrap_err();
    assert_eq!(conflict.current_head, Some(left_head));
    assert!(right.dirty_keys().contains(&body_key()));

    // The losing publisher incorporates protected state introduced since the
    // head it originally observed. Its own unpublished contribution stays dirty.
    let mut right_inbox = MultipartInbox::new();
    import_parts(
        &mut right,
        &mut right_inbox,
        &content_key,
        continuum_id,
        transport.parts_after(right_expected),
    );
    assert!(right.dirty_keys().contains(&body_key()));
    assert_eq!(
        right.get(&body_key()).unwrap().frontier_ids(),
        BTreeSet::from([rid(10), rid(20)])
    );

    let retry_projection = right.export_dirty().unwrap();
    let retry_part = protect_single(&content_key, continuum_id, pid(300), &retry_projection);
    let right_head = transport
        .publish(Some(left_head), vec![retry_part])
        .expect("retry against current head must succeed");
    right.acknowledge(&retry_projection);
    assert!(right.dirty_keys().is_empty());

    // The first publisher catches up only from the transport revision it already
    // incorporated. Transport revision identity is used for fetch bookkeeping,
    // never as logical edit ordering.
    let mut left_inbox = MultipartInbox::new();
    import_parts(
        &mut left,
        &mut left_inbox,
        &content_key,
        continuum_id,
        transport.parts_after(Some(left_head)),
    );

    assert_eq!(transport.head(), Some(right_head));
    transport.validate_linear_parents();
    assert_eq!(left.domains(), right.domains());
    assert!(left.dirty_keys().is_empty());
    assert!(right.dirty_keys().is_empty());

    let register = left.get(&body_key()).unwrap();
    assert_eq!(register.frontier_ids(), BTreeSet::from([rid(10), rid(20)]));
    assert_eq!(register.materialized(), Some(&b"right".to_vec()));
}
