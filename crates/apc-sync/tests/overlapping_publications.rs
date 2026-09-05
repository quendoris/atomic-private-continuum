use std::collections::BTreeMap;

use apc_core::{AtomId, RevisionId, ScalarRegister};
use apc_sync::{DomainKey, ScalarDirtyDomainState, ScalarSyncProjection, SyncProjection};

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

fn baseline() -> ScalarSyncProjection {
    let mut register = ScalarRegister::new();
    register.assign(rid(1), b"base".to_vec()).unwrap();
    SyncProjection::from_domains(BTreeMap::from([(body_key(), register)]))
}

fn state_with_baseline() -> ScalarDirtyDomainState {
    let mut state = ScalarDirtyDomainState::new();
    state.import_projection(&baseline()).unwrap();
    state
}

fn local_assign(state: &mut ScalarDirtyDomainState, revision: u64, value: &[u8]) {
    let key = body_key();
    let mut register = state.get(&key).unwrap().clone();
    register.assign(rid(revision), value.to_vec()).unwrap();
    state.set_local(key, register);
}

fn two_overlapping_exports() -> (
    ScalarDirtyDomainState,
    ScalarSyncProjection,
    ScalarSyncProjection,
) {
    let mut state = state_with_baseline();
    local_assign(&mut state, 10, b"A");
    let first = state.export_dirty().unwrap();

    local_assign(&mut state, 20, b"B");
    let second = state.export_dirty().unwrap();
    (state, first, second)
}

#[test]
fn older_ack_cannot_clear_newer_overlapping_publication() {
    let (mut state, first, second) = two_overlapping_exports();

    state.acknowledge(&first);
    assert!(state.dirty_keys().contains(&body_key()));
    assert_eq!(state.get(&body_key()), second.get(&body_key()));

    state.acknowledge(&second);
    assert!(state.dirty_keys().is_empty());
}

#[test]
fn newer_ack_then_late_older_ack_cannot_recreate_or_change_dirty_state() {
    let (mut state, first, second) = two_overlapping_exports();

    state.acknowledge(&second);
    assert!(state.dirty_keys().is_empty());

    state.acknowledge(&first);
    state.acknowledge(&first);
    assert!(state.dirty_keys().is_empty());
    assert_eq!(state.get(&body_key()), second.get(&body_key()));
}

#[test]
fn duplicate_ack_of_current_projection_is_idempotent() {
    let (mut state, _first, second) = two_overlapping_exports();

    state.acknowledge(&second);
    state.acknowledge(&second);
    assert!(state.dirty_keys().is_empty());
    assert_eq!(state.get(&body_key()), second.get(&body_key()));
}
