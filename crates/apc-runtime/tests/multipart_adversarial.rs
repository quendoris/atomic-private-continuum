use std::collections::{BTreeMap, BTreeSet};

use apc_core::id::LOGICAL_ID_BYTES;
use apc_core::{AtomId, ContinuumId, RevisionId, ScalarRegister};
use apc_crypto::ContentKey;
use apc_runtime::{decode_complete_scalar_domain_objects, ScalarObjectDecodeError};
use apc_sync::{
    encode_protected_sync_part, protect_scalar_part, DomainKey, PublicationId, SyncPartError,
    SyncProjection,
};

fn bytes(value: u64) -> [u8; LOGICAL_ID_BYTES] {
    let mut bytes = [0_u8; LOGICAL_ID_BYTES];
    bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
    bytes
}

fn rid(value: u64) -> RevisionId {
    RevisionId::from_bytes(bytes(value))
}

fn pid(value: u64) -> PublicationId {
    PublicationId::from_bytes(bytes(value))
}

fn cid(value: u64) -> ContinuumId {
    ContinuumId::from_bytes(bytes(value))
}

fn atom(value: u64) -> AtomId {
    AtomId::from_bytes(bytes(value))
}

fn domain_key() -> DomainKey {
    DomainKey::new(atom(1), b"body".to_vec()).unwrap()
}

fn projection(revision_id: RevisionId, value: &[u8]) -> apc_sync::ScalarSyncProjection {
    let mut register = ScalarRegister::new();
    register.assign(revision_id, value.to_vec()).unwrap();
    SyncProjection::from_domains(BTreeMap::from([(domain_key(), register)]))
}

fn encoded_part(
    key: &ContentKey,
    publication_id: PublicationId,
    part_index: u32,
    total_parts: u32,
    revision_id: RevisionId,
    value: &[u8],
) -> Vec<u8> {
    let part = protect_scalar_part(
        key,
        cid(1),
        publication_id,
        part_index,
        total_parts,
        &projection(revision_id, value),
    )
    .unwrap();
    encode_protected_sync_part(&part).unwrap()
}

#[test]
fn interleaved_reversed_complete_publications_assemble_without_id_ordering() {
    let key = ContentKey::from_bytes([0xA1; 32]);
    let expected_domain = domain_key();

    let p1_0 = encoded_part(&key, pid(100), 0, 2, rid(10), b"p1-a");
    let p1_1 = encoded_part(&key, pid(100), 1, 2, rid(11), b"p1-b");
    let p2_0 = encoded_part(&key, pid(1), 0, 2, rid(20), b"p2-a");
    let p2_1 = encoded_part(&key, pid(1), 1, 2, rid(21), b"p2-b");

    // Publication IDs and part indexes are deliberately presented in an order
    // that disagrees with their canonical byte ordering. Completion order is a
    // transport fact only and must not become semantic ordering.
    let decoded = decode_complete_scalar_domain_objects(
        &key,
        cid(1),
        &expected_domain,
        &[p2_1, p1_0, p2_0, p1_1],
    )
    .unwrap();

    assert_eq!(decoded.len(), 2);

    let merged = decoded
        .into_iter()
        .reduce(|left, right| left.merge(&right).unwrap())
        .unwrap();
    let revision_ids: BTreeSet<_> = merged.revisions().map(|revision| revision.id).collect();

    assert_eq!(revision_ids, BTreeSet::from([rid(10), rid(11), rid(20), rid(21)]));
    assert_eq!(
        merged.frontier_ids(),
        BTreeSet::from([rid(10), rid(11), rid(20), rid(21)])
    );
}

#[test]
fn identical_duplicate_part_while_pending_is_harmless() {
    let key = ContentKey::from_bytes([0xA2; 32]);
    let expected_domain = domain_key();

    let first = encoded_part(&key, pid(7), 0, 2, rid(30), b"first");
    let second = encoded_part(&key, pid(7), 1, 2, rid(31), b"second");

    let decoded = decode_complete_scalar_domain_objects(
        &key,
        cid(1),
        &expected_domain,
        &[first.clone(), first, second],
    )
    .unwrap();

    assert_eq!(decoded.len(), 1);
    let revision_ids: BTreeSet<_> = decoded[0]
        .revisions()
        .map(|revision| revision.id)
        .collect();
    assert_eq!(revision_ids, BTreeSet::from([rid(30), rid(31)]));
}

#[test]
fn authenticated_conflicting_duplicate_part_fails_closed() {
    let key = ContentKey::from_bytes([0xA3; 32]);
    let expected_domain = domain_key();

    let first_version = encoded_part(&key, pid(9), 0, 2, rid(40), b"first-version");
    let conflicting_version =
        encoded_part(&key, pid(9), 0, 2, rid(41), b"conflicting-version");
    let final_part = encoded_part(&key, pid(9), 1, 2, rid(42), b"final");

    let error = decode_complete_scalar_domain_objects(
        &key,
        cid(1),
        &expected_domain,
        &[first_version, conflicting_version, final_part],
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ScalarObjectDecodeError::Protection(SyncPartError::MultipartPartCollision)
    ));
}

#[test]
fn complete_publication_is_not_returned_when_another_publication_is_incomplete() {
    let key = ContentKey::from_bytes([0xA4; 32]);
    let expected_domain = domain_key();

    let complete = encoded_part(&key, pid(3), 0, 1, rid(50), b"complete");
    let incomplete = encoded_part(&key, pid(4), 0, 2, rid(60), b"incomplete");

    let error = decode_complete_scalar_domain_objects(
        &key,
        cid(1),
        &expected_domain,
        &[complete, incomplete],
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ScalarObjectDecodeError::IncompleteMultipartPublications { count: 1 }
    ));
}
