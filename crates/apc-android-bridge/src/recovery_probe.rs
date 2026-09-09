use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use apc_core::{AtomId, ContinuumId, LocalScalarDomain, RevisionId, ScalarRegister, WorkingEpochId};
use apc_crypto::ContentKey;
use apc_runtime::{
    prepare_recovery_handoff, stage_prepared_recovery_handoff,
    DevelopmentMultiScalarTrustedStateCodec, LocalScalarRecoveryState, TrustedStateCodec,
};
use apc_storage_fs::UnixFsDurabilityBackend;
use apc_sync::{
    DomainKey, DurableSyncRecord, ProtectedSyncRecordStore, PublicationId, TransportCursor,
};

const PROBE_DIR: &str = "apc-recovery-probe-v1";
const PROBE_CONTEXT: &[u8] = b"A.P.C. Android recovery probe\0v1";
const PROBE_CURSOR: &[u8] = b"android-probe-R1";
const PROBE_SECRET: &[u8] = b"android-probe-local-secret";
const PROBE_KEY_BYTES: [u8; 32] = [0xA7; 32];

fn logical_bytes(value: u64) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    bytes
}

fn atom(value: u64) -> AtomId {
    AtomId::from_bytes(logical_bytes(value))
}

fn cid(value: u64) -> ContinuumId {
    ContinuumId::from_bytes(logical_bytes(value))
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

fn domain_key() -> DomainKey {
    DomainKey::new(atom(1), b"body".to_vec()).expect("static probe domain is valid")
}

fn probe_root(files_dir: &Path) -> PathBuf {
    files_dir.join(PROBE_DIR)
}

fn content_key() -> ContentKey {
    // Harness-only fixed key. Production Android key ownership remains deliberately open.
    ContentKey::from_bytes(PROBE_KEY_BYTES)
}

fn open_store(
    files_dir: &Path,
) -> Result<ProtectedSyncRecordStore<UnixFsDurabilityBackend>, String> {
    let backend = UnixFsDurabilityBackend::open(probe_root(files_dir))
        .map_err(|error| format!("open durability backend: {error}"))?;
    ProtectedSyncRecordStore::new(backend, content_key(), PROBE_CONTEXT.to_vec())
        .map_err(|error| format!("open protected sync store: {error}"))
}

fn initial_recovery() -> Result<LocalScalarRecoveryState, String> {
    let mut causal = ScalarRegister::new();
    causal
        .assign(rid(100), b"android-probe-base".to_vec())
        .map_err(|error| format!("create base causal state: {error}"))?;

    let mut domain = LocalScalarDomain::from_causal(causal)
        .map_err(|error| format!("create local scalar domain: {error}"))?;
    domain
        .begin_epoch(wid(1), PROBE_SECRET.to_vec())
        .map_err(|error| format!("begin working epoch: {error}"))?;
    domain
        .seal_local(rid(200))
        .map_err(|error| format!("seal local revision: {error}"))?;
    domain
        .finalize(rid(200))
        .map_err(|error| format!("finalize local revision: {error}"))?;

    LocalScalarRecoveryState::from_domains(BTreeMap::from([(domain_key(), domain.snapshot())]))
        .map_err(|error| format!("create recovery container: {error}"))
}

pub fn stage(files_dir: &Path) -> Result<String, String> {
    let root = probe_root(files_dir);
    match fs::remove_dir_all(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("reset prior probe state: {error}")),
    }

    let codec = DevelopmentMultiScalarTrustedStateCodec;
    let mut recovery = initial_recovery()?;
    let trusted_state = codec
        .encode(&recovery)
        .map_err(|error| format!("encode initial trusted state: {error}"))?;
    let cursor = TransportCursor::new(PROBE_CURSOR.to_vec())
        .map_err(|error| format!("create probe cursor: {error}"))?;
    let mut record = DurableSyncRecord::new(trusted_state, Some(cursor));
    let mut store = open_store(files_dir)?;

    let publication_key = content_key();
    let prepared = prepare_recovery_handoff(
        &recovery,
        cid(1),
        pid(7),
        &publication_key,
        BTreeMap::from([(domain_key(), BTreeSet::from([rid(200)]))]),
    )
    .map_err(|error| format!("prepare protected handoff: {error}"))?;

    stage_prepared_recovery_handoff(&mut recovery, &mut record, &mut store, &codec, prepared)
        .map_err(|error| format!("durably stage protected handoff: {error:?}"))?;

    let committed = store
        .load_committed()
        .map_err(|error| format!("reload committed record: {error}"))?
        .ok_or_else(|| "durability backend returned no committed record".to_owned())?;
    validate_committed(&committed)?;

    Ok("PASS staged encrypted exposed state + exact outbox".to_owned())
}

pub fn verify(files_dir: &Path) -> Result<String, String> {
    let store = open_store(files_dir)?;
    let committed = store
        .load_committed()
        .map_err(|error| format!("load committed record after restart: {error}"))?
        .ok_or_else(|| "no committed recovery probe exists".to_owned())?;
    validate_committed(&committed)?;

    Ok("PASS reopened encrypted state; cursor/exposure/outbox intact".to_owned())
}

fn validate_committed(record: &DurableSyncRecord) -> Result<(), String> {
    let cursor = record
        .applied_cursor()
        .ok_or_else(|| "committed record lost applied cursor".to_owned())?;
    if cursor.as_bytes() != PROBE_CURSOR {
        return Err("committed record restored a different cursor".to_owned());
    }

    if record.outbox().len() != 1 {
        return Err(format!(
            "committed record expected one pending publication, found {}",
            record.outbox().len()
        ));
    }
    let entry = record
        .outbox()
        .get(&pid(7))
        .ok_or_else(|| "committed record lost probe publication".to_owned())?;
    let expected = entry
        .expected_cursor()
        .ok_or_else(|| "probe outbox lost expected cursor".to_owned())?;
    if expected.as_bytes() != PROBE_CURSOR {
        return Err("probe outbox expected cursor changed".to_owned());
    }
    if entry.objects().len() != 1 || entry.objects()[0].is_empty() {
        return Err("probe outbox did not retain one exact protected wire object".to_owned());
    }

    let codec = DevelopmentMultiScalarTrustedStateCodec;
    let recovery = codec
        .decode(record.trusted_state())
        .map_err(|error| format!("decode committed trusted state: {error}"))?;
    let domain = recovery
        .restore_domain(&domain_key())
        .map_err(|error| format!("restore committed scalar domain: {error}"))?
        .ok_or_else(|| "committed trusted state lost probe domain".to_owned())?;

    if !domain
        .finalization()
        .exposed_local_ids()
        .contains(&rid(200))
    {
        return Err("committed trusted state lost exposure marker".to_owned());
    }
    if !domain
        .finalization()
        .handed_off_local_ids()
        .contains(&rid(200))
    {
        return Err("committed trusted state lost handoff marker".to_owned());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "apc-android-recovery-probe-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn staged_probe_survives_real_filesystem_reopen() {
        let dir = TestDir::new();

        assert!(stage(&dir.0).unwrap().starts_with("PASS"));
        assert!(verify(&dir.0).unwrap().starts_with("PASS"));
    }
}
