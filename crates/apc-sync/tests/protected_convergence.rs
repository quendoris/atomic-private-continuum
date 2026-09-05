use std::collections::BTreeSet;

use apc_core::{AtomId, ContinuumId, RevisionId, ScalarRegister};
use apc_crypto::ContentKey;
use apc_sync::{
    protect_scalar_part, DomainKey, MultipartInbox, PublicationId, ScalarDirtyDomainState,
    ScalarSyncProjection, SyncProjection,
};

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

fn locally_edit(
    state: &mut ScalarDirtyDomainState,
    revision_id: RevisionId,
    value: &[u8],
) {
    let key = body_key();
    let mut register = state.get(&key).unwrap().clone();
    register.assign(revision_id, value.to_vec()).unwrap();
    state.set_local(key, register);
}

fn ingest_and_import(
    inbox: &mut MultipartInbox,
    state: &mut ScalarDirtyDomainState,
    key: &ContentKey,
    continuum_id: ContinuumId,
    part: apc_sync::ProtectedSyncPart,
) {
    let projection = inbox
        .ingest(key, continuum_id, part)
        .unwrap()
        .expect("single-part publication must complete immediately");
    state.import_projection(&projection).unwrap();
}

#[test]
fn two_replicas_exchange_protected_state_in_opposite_orders_and_converge() {
    let content_key = ContentKey::from_bytes([0x71; 32]);
    let continuum_id = cid(7);
    let baseline = baseline_projection();

    let mut left = ScalarDirtyDomainState::new();
    let mut right = ScalarDirtyDomainState::new();
    left.import_projection(&baseline).unwrap();
    right.import_projection(&baseline).unwrap();

    locally_edit(&mut left, rid(10), b"left");
    locally_edit(&mut right, rid(20), b"right");

    let left_projection = left.export_dirty().unwrap();
    let right_projection = right.export_dirty().unwrap();

    let left_part = protect_scalar_part(
        &content_key,
        continuum_id,
        pid(100),
        0,
        1,
        &left_projection,
    )
    .unwrap();
    let right_part = protect_scalar_part(
        &content_key,
        continuum_id,
        pid(200),
        0,
        1,
        &right_projection,
    )
    .unwrap();

    // Successful publication acknowledgement clears only the state actually
    // exported. Later remote import must not manufacture new local dirtiness.
    left.acknowledge(&left_projection);
    right.acknowledge(&right_projection);
    assert!(left.dirty_keys().is_empty());
    assert!(right.dirty_keys().is_empty());

    let mut left_inbox = MultipartInbox::new();
    ingest_and_import(
        &mut left_inbox,
        &mut left,
        &content_key,
        continuum_id,
        right_part.clone(),
    );
    ingest_and_import(
        &mut left_inbox,
        &mut left,
        &content_key,
        continuum_id,
        left_part.clone(),
    );

    let mut right_inbox = MultipartInbox::new();
    ingest_and_import(
        &mut right_inbox,
        &mut right,
        &content_key,
        continuum_id,
        left_part,
    );
    ingest_and_import(
        &mut right_inbox,
        &mut right,
        &content_key,
        continuum_id,
        right_part,
    );

    assert_eq!(left.domains(), right.domains());
    assert!(left.dirty_keys().is_empty());
    assert!(right.dirty_keys().is_empty());

    let register = left.get(&body_key()).unwrap();
    assert_eq!(
        register.frontier_ids(),
        BTreeSet::from([rid(10), rid(20)])
    );
    assert_eq!(register.materialized(), Some(&b"right".to_vec()));
}
