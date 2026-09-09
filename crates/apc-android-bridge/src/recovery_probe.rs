use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use apc_core::{
    commit_durable, AtomId, ContinuumId, DurabilityBackend, LocalScalarDomain, RevisionId,
    ScalarRegister, WorkingEpochId,
};
use apc_crypto::ContentKey;
use apc_runtime::{
    prepare_recovery_handoff, stage_prepared_recovery_handoff,
    DevelopmentMultiScalarTrustedStateCodec, ForegroundRecoveryCycleSpec, ForegroundRecoveryRuntime,
    LocalScalarRecoveryState, TrustedStateCodec,
};
use apc_storage_fs::{FsStorageError, UnixFsDurabilityBackend};
use apc_sync::{
    DomainKey, DurableSyncRecord, FetchOutcome, OpaqueTransport, ProtectedSyncRecordStore,
    PublicationId, PublishOutcome, TransportCursor, TransportCursorCodec,
};

const PROBE_DIR: &str = "apc-recovery-probe-v1";
const PROBE_REMOTE_DIR: &str = "apc-recovery-probe-remote-v1";
const PROBE_CONTEXT: &[u8] = b"A.P.C. Android recovery probe\0v1";
const PROBE_SECRET: &[u8] = b"android-probe-local-secret";
const PROBE_KEY_BYTES: [u8; 32] = [0xA7; 32];
const PROBE_INITIAL_HEAD: u64 = 1;
const PROBE_ACCEPTED_HEAD: u64 = 2;
const REMOTE_MAGIC: &[u8; 8] = b"APCARMT1";

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

fn remote_root(files_dir: &Path) -> PathBuf {
    files_dir.join(PROBE_REMOTE_DIR)
}

fn content_key() -> ContentKey {
    // Harness-only fixed key. Production Android key ownership remains deliberately open.
    ContentKey::from_bytes(PROBE_KEY_BYTES)
}

#[derive(Clone, Copy, Debug, Default)]
struct ProbeCursorCodec;

impl TransportCursorCodec<u64> for ProbeCursorCodec {
    type Error = &'static str;

    fn encode(&self, revision: &u64) -> Result<TransportCursor, Self::Error> {
        TransportCursor::new(revision.to_be_bytes().to_vec()).map_err(|_| "encode probe cursor")
    }

    fn decode(&self, cursor: &TransportCursor) -> Result<u64, Self::Error> {
        let bytes: [u8; 8] = cursor
            .as_bytes()
            .try_into()
            .map_err(|_| "decode probe cursor")?;
        Ok(u64::from_be_bytes(bytes))
    }
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

fn reset_path(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("reset probe path {}: {error}", path.display())),
    }
}

fn stage_local_record(
    files_dir: &Path,
) -> Result<
    (
        LocalScalarRecoveryState,
        DurableSyncRecord,
        ProtectedSyncRecordStore<UnixFsDurabilityBackend>,
    ),
    String,
> {
    reset_path(&probe_root(files_dir))?;

    let codec = DevelopmentMultiScalarTrustedStateCodec;
    let cursor_codec = ProbeCursorCodec;
    let mut recovery = initial_recovery()?;
    let trusted_state = codec
        .encode(&recovery)
        .map_err(|error| format!("encode initial trusted state: {error}"))?;
    let cursor = cursor_codec
        .encode(&PROBE_INITIAL_HEAD)
        .map_err(str::to_owned)?;
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
    validate_pending_committed(&committed)?;

    Ok((recovery, record, store))
}

pub fn stage(files_dir: &Path) -> Result<String, String> {
    let _ = stage_local_record(files_dir)?;
    Ok("PASS staged encrypted exposed state + exact outbox".to_owned())
}

pub fn verify(files_dir: &Path) -> Result<String, String> {
    let store = open_store(files_dir)?;
    let committed = store
        .load_committed()
        .map_err(|error| format!("load committed record after restart: {error}"))?
        .ok_or_else(|| "no committed recovery probe exists".to_owned())?;
    validate_pending_committed(&committed)?;

    Ok("PASS reopened encrypted state; cursor/exposure/outbox intact".to_owned())
}

fn validate_pending_committed(record: &DurableSyncRecord) -> Result<(), String> {
    validate_cursor(record, PROBE_INITIAL_HEAD)?;

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
    if ProbeCursorCodec.decode(expected).map_err(str::to_owned)? != PROBE_INITIAL_HEAD {
        return Err("probe outbox expected cursor changed".to_owned());
    }
    if entry.objects().len() != 1 || entry.objects()[0].is_empty() {
        return Err("probe outbox did not retain one exact protected wire object".to_owned());
    }

    validate_exposure(record)
}

fn validate_reconciled_committed(record: &DurableSyncRecord) -> Result<(), String> {
    validate_cursor(record, PROBE_ACCEPTED_HEAD)?;
    if !record.outbox().is_empty() {
        return Err("reconciled record still contains the lost-ACK outbox entry".to_owned());
    }
    validate_exposure(record)
}

fn validate_cursor(record: &DurableSyncRecord, expected: u64) -> Result<(), String> {
    let cursor = record
        .applied_cursor()
        .ok_or_else(|| "committed record lost applied cursor".to_owned())?;
    let actual = ProbeCursorCodec.decode(cursor).map_err(str::to_owned)?;
    if actual != expected {
        return Err(format!(
            "committed record restored cursor {actual}, expected {expected}"
        ));
    }
    Ok(())
}

fn validate_exposure(record: &DurableSyncRecord) -> Result<(), String> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProbeRemoteState {
    head: u64,
    publish_count: u64,
    objects: Vec<Vec<u8>>,
}

impl Default for ProbeRemoteState {
    fn default() -> Self {
        Self {
            head: PROBE_INITIAL_HEAD,
            publish_count: 0,
            objects: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbePublishMode {
    Normal,
    LoseAcceptedResponse,
}

#[derive(Debug)]
enum ProbeTransportError {
    Storage(FsStorageError),
    InvalidState(String),
    LostAcceptedResponse,
}

impl core::fmt::Display for ProbeTransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Storage(error) => write!(f, "probe transport storage error: {error}"),
            Self::InvalidState(error) => write!(f, "invalid probe transport state: {error}"),
            Self::LostAcceptedResponse => write!(f, "simulated response loss after remote acceptance"),
        }
    }
}

impl std::error::Error for ProbeTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::InvalidState(_) | Self::LostAcceptedResponse => None,
        }
    }
}

impl From<FsStorageError> for ProbeTransportError {
    fn from(value: FsStorageError) -> Self {
        Self::Storage(value)
    }
}

struct ProbeTransport {
    backend: UnixFsDurabilityBackend,
    mode: ProbePublishMode,
}

impl ProbeTransport {
    fn open(files_dir: &Path, mode: ProbePublishMode) -> Result<Self, String> {
        let backend = UnixFsDurabilityBackend::open(remote_root(files_dir))
            .map_err(|error| format!("open simulated remote transport: {error}"))?;
        Ok(Self { backend, mode })
    }

    fn load_state(&self) -> Result<ProbeRemoteState, ProbeTransportError> {
        match self.backend.load_committed()? {
            Some(bytes) => decode_remote_state(&bytes),
            None => Ok(ProbeRemoteState::default()),
        }
    }

    fn persist_state(&mut self, state: &ProbeRemoteState) -> Result<(), ProbeTransportError> {
        let encoded = encode_remote_state(state)?;
        commit_durable(&mut self.backend, &encoded)?;
        Ok(())
    }
}

impl OpaqueTransport for ProbeTransport {
    type Revision = u64;
    type Error = ProbeTransportError;

    fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
        Ok(Some(self.load_state()?.head))
    }

    fn fetch_since(
        &mut self,
        known_head: Option<&Self::Revision>,
    ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
        let state = self.load_state()?;
        if known_head == Some(&state.head) {
            return Ok(FetchOutcome::UpToDate {
                head: Some(state.head),
            });
        }
        if known_head == Some(&PROBE_INITIAL_HEAD) && state.head == PROBE_ACCEPTED_HEAD {
            return Ok(FetchOutcome::Changed {
                head: state.head,
                objects: state.objects,
            });
        }
        Ok(FetchOutcome::BaselineUnavailable {
            head: Some(state.head),
        })
    }

    fn publish(
        &mut self,
        expected_head: Option<&Self::Revision>,
        objects: &[Vec<u8>],
    ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
        if objects.is_empty() || objects.iter().any(Vec::is_empty) {
            return Err(ProbeTransportError::InvalidState(
                "publish must contain non-empty opaque objects".to_owned(),
            ));
        }

        let current = self.load_state()?;
        if expected_head != Some(&current.head) {
            return Ok(PublishOutcome::Conflict {
                current_head: Some(current.head),
            });
        }

        let head = current
            .head
            .checked_add(1)
            .ok_or_else(|| ProbeTransportError::InvalidState("head overflow".to_owned()))?;
        let accepted = ProbeRemoteState {
            head,
            publish_count: current.publish_count + 1,
            objects: objects.to_vec(),
        };
        self.persist_state(&accepted)?;

        if self.mode == ProbePublishMode::LoseAcceptedResponse {
            self.mode = ProbePublishMode::Normal;
            return Err(ProbeTransportError::LostAcceptedResponse);
        }

        Ok(PublishOutcome::Published { head })
    }
}

fn encode_remote_state(state: &ProbeRemoteState) -> Result<Vec<u8>, ProbeTransportError> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(REMOTE_MAGIC);
    encoded.extend_from_slice(&state.head.to_be_bytes());
    encoded.extend_from_slice(&state.publish_count.to_be_bytes());
    let count = u64::try_from(state.objects.len()).map_err(|_| {
        ProbeTransportError::InvalidState("object count does not fit u64".to_owned())
    })?;
    encoded.extend_from_slice(&count.to_be_bytes());
    for object in &state.objects {
        let len = u64::try_from(object.len()).map_err(|_| {
            ProbeTransportError::InvalidState("object length does not fit u64".to_owned())
        })?;
        encoded.extend_from_slice(&len.to_be_bytes());
        encoded.extend_from_slice(object);
    }
    Ok(encoded)
}

fn decode_remote_state(bytes: &[u8]) -> Result<ProbeRemoteState, ProbeTransportError> {
    if bytes.len() < REMOTE_MAGIC.len() || &bytes[..REMOTE_MAGIC.len()] != REMOTE_MAGIC {
        return Err(ProbeTransportError::InvalidState(
            "wrong remote-state magic".to_owned(),
        ));
    }
    let mut offset = REMOTE_MAGIC.len();
    let head = read_u64(bytes, &mut offset)?;
    let publish_count = read_u64(bytes, &mut offset)?;
    let count = usize::try_from(read_u64(bytes, &mut offset)?).map_err(|_| {
        ProbeTransportError::InvalidState("object count overflows usize".to_owned())
    })?;
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        let len = usize::try_from(read_u64(bytes, &mut offset)?).map_err(|_| {
            ProbeTransportError::InvalidState("object length overflows usize".to_owned())
        })?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| ProbeTransportError::InvalidState("object offset overflow".to_owned()))?;
        let object = bytes
            .get(offset..end)
            .ok_or_else(|| ProbeTransportError::InvalidState("truncated object".to_owned()))?;
        if object.is_empty() {
            return Err(ProbeTransportError::InvalidState(
                "empty remote object".to_owned(),
            ));
        }
        objects.push(object.to_vec());
        offset = end;
    }
    if offset != bytes.len() {
        return Err(ProbeTransportError::InvalidState(
            "trailing remote-state bytes".to_owned(),
        ));
    }
    Ok(ProbeRemoteState {
        head,
        publish_count,
        objects,
    })
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Result<u64, ProbeTransportError> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| ProbeTransportError::InvalidState("u64 offset overflow".to_owned()))?;
    let raw: [u8; 8] = bytes
        .get(*offset..end)
        .ok_or_else(|| ProbeTransportError::InvalidState("truncated u64".to_owned()))?
        .try_into()
        .expect("fixed-size u64 slice");
    *offset = end;
    Ok(u64::from_be_bytes(raw))
}

fn load_remote(files_dir: &Path) -> Result<ProbeRemoteState, String> {
    ProbeTransport::open(files_dir, ProbePublishMode::Normal)?
        .load_state()
        .map_err(|error| error.to_string())
}

/// Simulate the hard case: the transport durably accepts the exact protected
/// publication, then its response disappears before local reconciliation.
pub fn stage_lost_ack(files_dir: &Path) -> Result<String, String> {
    reset_path(&remote_root(files_dir))?;
    let (mut recovery, mut record, mut store) = stage_local_record(files_dir)?;
    let transport = ProbeTransport::open(files_dir, ProbePublishMode::LoseAcceptedResponse)?;
    let mut runtime = ForegroundRecoveryRuntime::new(transport);
    runtime.enter_foreground();

    let codec = DevelopmentMultiScalarTrustedStateCodec;
    let cursor_codec = ProbeCursorCodec;
    let key = content_key();
    let pre_observation = BTreeMap::new();
    let result = runtime.resume_scalar_recovery(
        &mut recovery,
        &mut record,
        &mut store,
        &codec,
        &cursor_codec,
        ForegroundRecoveryCycleSpec::new(&key, cid(1), &pre_observation),
    );
    if result.is_ok() {
        return Err("lost-ACK stage unexpectedly received a successful response".to_owned());
    }

    let durable = store
        .load_committed()
        .map_err(|error| format!("reload local state after lost response: {error}"))?
        .ok_or_else(|| "local state disappeared after lost response".to_owned())?;
    validate_pending_committed(&durable)?;

    let remote = load_remote(files_dir)?;
    if remote.head != PROBE_ACCEPTED_HEAD
        || remote.publish_count != 1
        || remote.objects.is_empty()
    {
        return Err("simulated remote did not durably accept exactly one publication".to_owned());
    }

    Ok("PASS remote accepted once; response lost; durable local outbox still pending".to_owned())
}

/// Reopen after process death and prove the previous accepted publication by
/// authenticated refetch. Successful reconciliation must not publish it again.
pub fn resume_lost_ack(files_dir: &Path) -> Result<String, String> {
    let mut store = open_store(files_dir)?;
    let mut record = store
        .load_committed()
        .map_err(|error| format!("load local state for lost-ACK resume: {error}"))?
        .ok_or_else(|| "no staged lost-ACK local state exists".to_owned())?;
    validate_pending_committed(&record)?;

    let codec = DevelopmentMultiScalarTrustedStateCodec;
    let mut recovery = codec
        .decode(record.trusted_state())
        .map_err(|error| format!("decode local recovery for resume: {error}"))?;
    let transport = ProbeTransport::open(files_dir, ProbePublishMode::Normal)?;
    let mut runtime = ForegroundRecoveryRuntime::new(transport);
    runtime.enter_foreground();

    let cursor_codec = ProbeCursorCodec;
    let key = content_key();
    let pre_observation = BTreeMap::new();
    runtime
        .resume_scalar_recovery(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            ForegroundRecoveryCycleSpec::new(&key, cid(1), &pre_observation),
        )
        .map_err(|error| format!("foreground lost-ACK recovery failed: {error:?}"))?;

    validate_reconciled_committed(&record)?;
    let durable = store
        .load_committed()
        .map_err(|error| format!("reload reconciled local state: {error}"))?
        .ok_or_else(|| "reconciled local state disappeared".to_owned())?;
    validate_reconciled_committed(&durable)?;

    let remote = load_remote(files_dir)?;
    if remote.head != PROBE_ACCEPTED_HEAD || remote.publish_count != 1 {
        return Err("lost-ACK recovery retransmitted an already accepted publication".to_owned());
    }

    Ok("PASS authenticated refetch reconciled lost ACK; no second publish".to_owned())
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

    #[test]
    fn lost_ack_probe_reconciles_after_restart_without_second_publish() {
        let dir = TestDir::new();

        assert!(stage_lost_ack(&dir.0).unwrap().starts_with("PASS"));
        assert_eq!(load_remote(&dir.0).unwrap().publish_count, 1);
        assert!(resume_lost_ack(&dir.0).unwrap().starts_with("PASS"));
        assert_eq!(load_remote(&dir.0).unwrap().publish_count, 1);
    }
}
