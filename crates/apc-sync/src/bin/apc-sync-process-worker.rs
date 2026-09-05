use std::env;
use std::fs;
use std::path::Path;

use apc_core::{AtomId, ContinuumId, RevisionId, ScalarRegister};
use apc_crypto::ContentKey;
use apc_sync::{
    encode_scalar_projection, protect_scalar_part, DomainKey, MultipartInbox, ProtectedSyncPart,
    PublicationId, ScalarDirtyDomainState, ScalarSyncProjection, SyncProjection,
};

const TEST_KEY: [u8; 32] = [0x91; 32];

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        panic!("usage: apc-sync-process-worker <mode> <output> [left-payload right-payload]");
    }

    match args[1].as_str() {
        "emit-left" => emit(Path::new(&args[2]), 10, b"left", pid(100)),
        "emit-right" => emit(Path::new(&args[2]), 20, b"right", pid(200)),
        "merge-left" => {
            require_merge_args(&args);
            merge(
                Path::new(&args[2]),
                Path::new(&args[3]),
                Path::new(&args[4]),
                true,
            );
        }
        "merge-right" => {
            require_merge_args(&args);
            merge(
                Path::new(&args[2]),
                Path::new(&args[3]),
                Path::new(&args[4]),
                false,
            );
        }
        mode => panic!("unknown worker mode {mode}"),
    }
}

fn require_merge_args(args: &[String]) {
    assert_eq!(
        args.len(),
        5,
        "merge mode requires output, left payload and right payload"
    );
}

fn emit(output: &Path, revision_id: u64, value: &[u8], publication_id: PublicationId) {
    let projection = edited_projection(revision_id, value);
    let key = ContentKey::from_bytes(TEST_KEY);
    let part = protect_scalar_part(&key, cid(7), publication_id, 0, 1, &projection).unwrap();
    fs::write(output, part.payload).unwrap();
}

fn merge(output: &Path, left_payload: &Path, right_payload: &Path, left_replica: bool) {
    let key = ContentKey::from_bytes(TEST_KEY);
    let continuum_id = cid(7);
    let baseline = baseline_projection();
    let mut state = ScalarDirtyDomainState::new();
    state.import_projection(&baseline).unwrap();

    if left_replica {
        locally_edit(&mut state, rid(10), b"left");
    } else {
        locally_edit(&mut state, rid(20), b"right");
    }

    let left = ProtectedSyncPart {
        publication_id: pid(100),
        part_index: 0,
        total_parts: 1,
        payload: fs::read(left_payload).unwrap(),
    };
    let right = ProtectedSyncPart {
        publication_id: pid(200),
        part_index: 0,
        total_parts: 1,
        payload: fs::read(right_payload).unwrap(),
    };

    let mut inbox = MultipartInbox::new();
    let parts = if left_replica {
        [right, left]
    } else {
        [left, right]
    };
    for part in parts {
        let projection = inbox
            .ingest(&key, continuum_id, part)
            .unwrap()
            .expect("single-part publication must complete immediately");
        state.import_projection(&projection).unwrap();
    }

    let full = SyncProjection::from_domains(state.domains().clone());
    let encoded = encode_scalar_projection(&full).unwrap();
    fs::write(output, encoded).unwrap();
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

fn edited_projection(revision_id: u64, value: &[u8]) -> ScalarSyncProjection {
    let baseline = baseline_projection();
    let mut register = baseline.get(&body_key()).unwrap().clone();
    register.assign(rid(revision_id), value.to_vec()).unwrap();
    SyncProjection::from_domains([(body_key(), register)].into_iter().collect())
}

fn locally_edit(state: &mut ScalarDirtyDomainState, revision_id: RevisionId, value: &[u8]) {
    let key = body_key();
    let mut register = state.get(&key).unwrap().clone();
    register.assign(revision_id, value.to_vec()).unwrap();
    state.set_local(key, register);
}
