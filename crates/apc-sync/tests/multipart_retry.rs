use std::collections::BTreeMap;

use apc_core::{AtomId, ContinuumId, RevisionId, ScalarRegister};
use apc_crypto::ContentKey;
use apc_sync::{
    protect_scalar_part, DomainKey, MultipartInbox, PublicationId, ScalarSyncProjection,
    SyncProjection,
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

fn projection(atom_value: u64, revision: u64, value: &[u8]) -> ScalarSyncProjection {
    let domain = DomainKey::new(atom(atom_value), b"body".to_vec()).unwrap();
    let mut register = ScalarRegister::new();
    register.assign(rid(revision), value.to_vec()).unwrap();
    SyncProjection::from_domains(BTreeMap::from([(domain, register)]))
}

#[test]
fn interrupted_incomplete_publication_can_restart_by_replaying_immutable_parts() {
    let key = ContentKey::from_bytes([0xb1; 32]);
    let continuum_id = cid(4);
    let publication_id = pid(90);

    let parts = [
        protect_scalar_part(
            &key,
            continuum_id,
            publication_id,
            0,
            3,
            &projection(1, 10, b"one"),
        )
        .unwrap(),
        protect_scalar_part(
            &key,
            continuum_id,
            publication_id,
            1,
            3,
            &projection(2, 20, b"two"),
        )
        .unwrap(),
        protect_scalar_part(
            &key,
            continuum_id,
            publication_id,
            2,
            3,
            &projection(3, 30, b"three"),
        )
        .unwrap(),
    ];

    let mut first_session = MultipartInbox::new();
    assert!(first_session
        .ingest(&key, continuum_id, parts[2].clone())
        .unwrap()
        .is_none());
    assert!(first_session
        .ingest(&key, continuum_id, parts[0].clone())
        .unwrap()
        .is_none());
    assert_eq!(first_session.pending_publications(), 1);

    // No semantic projection escaped. Losing this in-memory assembly state is
    // therefore safe: a resumed session can replay immutable protected parts
    // from transport rather than persisting partial clear merge state.
    drop(first_session);

    let mut resumed = MultipartInbox::new();
    assert!(resumed
        .ingest(&key, continuum_id, parts[0].clone())
        .unwrap()
        .is_none());
    assert!(resumed
        .ingest(&key, continuum_id, parts[0].clone())
        .unwrap()
        .is_none());
    assert!(resumed
        .ingest(&key, continuum_id, parts[2].clone())
        .unwrap()
        .is_none());

    let complete = resumed
        .ingest(&key, continuum_id, parts[1].clone())
        .unwrap()
        .expect("replayed publication must complete when missing part arrives");

    assert_eq!(complete.len(), 3);
    assert_eq!(resumed.pending_publications(), 0);
}

#[test]
fn incomplete_publication_does_not_block_an_independent_complete_publication() {
    let key = ContentKey::from_bytes([0xb2; 32]);
    let continuum_id = cid(5);
    let slow_id = pid(100);
    let fast_id = pid(200);

    let slow_first = protect_scalar_part(
        &key,
        continuum_id,
        slow_id,
        0,
        2,
        &projection(1, 10, b"slow-one"),
    )
    .unwrap();
    let slow_second = protect_scalar_part(
        &key,
        continuum_id,
        slow_id,
        1,
        2,
        &projection(2, 20, b"slow-two"),
    )
    .unwrap();
    let fast = protect_scalar_part(
        &key,
        continuum_id,
        fast_id,
        0,
        1,
        &projection(3, 30, b"fast"),
    )
    .unwrap();

    let mut inbox = MultipartInbox::new();
    assert!(inbox
        .ingest(&key, continuum_id, slow_first)
        .unwrap()
        .is_none());

    let fast_complete = inbox
        .ingest(&key, continuum_id, fast)
        .unwrap()
        .expect("independent single-part publication should complete");
    assert_eq!(fast_complete.len(), 1);
    assert_eq!(inbox.pending_publications(), 1);

    let slow_complete = inbox
        .ingest(&key, continuum_id, slow_second)
        .unwrap()
        .expect("slow publication should complete independently");
    assert_eq!(slow_complete.len(), 2);
    assert_eq!(inbox.pending_publications(), 0);
}
