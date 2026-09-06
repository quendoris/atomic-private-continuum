use std::collections::BTreeMap;

use apc_core::id::LOGICAL_ID_BYTES;
use apc_core::{AtomId, CoreError, LocalScalarDomain, LocalScalarSnapshot};
use apc_sync::{DomainKey, ProjectionError};

use crate::{
    decode_local_scalar_snapshot, encode_local_scalar_snapshot, TrustedStateCodec,
    TrustedStateCodecError,
};

const MAGIC: &[u8; 8] = b"APCLSET1";
const VERSION: u16 = 1;

/// Complete local recovery state for several independently mergeable scalar
/// domains.
///
/// This is a runtime recovery container, not a semantic transaction. Each entry
/// retains its own `LocalScalarSnapshot`, including its own working epoch,
/// observed frontier and finalization/exposure state. Mutating one entry does not
/// manufacture observation, causality or atomic semantics in another entry.
///
/// `DomainKey` is reused only as the current pre-format merge-domain key. Neither
/// its byte ordering nor this container encoding is a portable format promise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalScalarRecoveryState {
    domains: BTreeMap<DomainKey, LocalScalarSnapshot<Vec<u8>>>,
}

#[derive(Debug)]
pub enum LocalScalarRecoveryStateError {
    DuplicateDomain,
    Core(CoreError),
}

impl core::fmt::Display for LocalScalarRecoveryStateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicateDomain => write!(f, "local recovery state already contains this domain"),
            Self::Core(error) => write!(f, "invalid local scalar recovery domain: {error}"),
        }
    }
}

impl std::error::Error for LocalScalarRecoveryStateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::DuplicateDomain => None,
        }
    }
}

impl From<CoreError> for LocalScalarRecoveryStateError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

impl LocalScalarRecoveryState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_domains(
        domains: BTreeMap<DomainKey, LocalScalarSnapshot<Vec<u8>>>,
    ) -> Result<Self, LocalScalarRecoveryStateError> {
        for snapshot in domains.values() {
            LocalScalarDomain::restore(snapshot.clone())?;
        }
        Ok(Self { domains })
    }

    pub fn len(&self) -> usize {
        self.domains.len()
    }

    pub fn is_empty(&self) -> bool {
        self.domains.is_empty()
    }

    pub fn domains(&self) -> &BTreeMap<DomainKey, LocalScalarSnapshot<Vec<u8>>> {
        &self.domains
    }

    pub fn get(&self, key: &DomainKey) -> Option<&LocalScalarSnapshot<Vec<u8>>> {
        self.domains.get(key)
    }

    pub fn insert_new(
        &mut self,
        key: DomainKey,
        snapshot: LocalScalarSnapshot<Vec<u8>>,
    ) -> Result<(), LocalScalarRecoveryStateError> {
        if self.domains.contains_key(&key) {
            return Err(LocalScalarRecoveryStateError::DuplicateDomain);
        }
        LocalScalarDomain::restore(snapshot.clone())?;
        self.domains.insert(key, snapshot);
        Ok(())
    }

    /// Replace exactly one domain after validating its complete local snapshot.
    ///
    /// This operation deliberately has no multi-key variant. Crash-atomic storage
    /// may later persist the whole container as one physical blob, but that does
    /// not turn independent merge domains into one semantic transaction.
    pub fn replace_domain(
        &mut self,
        key: DomainKey,
        snapshot: LocalScalarSnapshot<Vec<u8>>,
    ) -> Result<Option<LocalScalarSnapshot<Vec<u8>>>, LocalScalarRecoveryStateError> {
        LocalScalarDomain::restore(snapshot.clone())?;
        Ok(self.domains.insert(key, snapshot))
    }

    pub fn restore_domain(
        &self,
        key: &DomainKey,
    ) -> Result<Option<LocalScalarDomain<Vec<u8>>>, CoreError> {
        self.domains
            .get(key)
            .cloned()
            .map(LocalScalarDomain::restore)
            .transpose()
    }
}

/// Deterministic development codec for `LocalScalarRecoveryState`.
///
/// `APCLSET1` is only a local recovery framing. It nests the existing `APCLREC1`
/// scalar snapshot bytes so scalar semantics remain owned by the already-tested
/// scalar codec instead of being reimplemented here.
#[derive(Clone, Copy, Debug, Default)]
pub struct DevelopmentMultiScalarTrustedStateCodec;

#[derive(Debug)]
pub enum MultiScalarTrustedStateCodecError {
    InvalidMagic,
    UnsupportedVersion { version: u16 },
    UnexpectedEof,
    LengthOverflow,
    TrailingBytes,
    DuplicateDomain,
    Domain(ProjectionError),
    Scalar(TrustedStateCodecError),
}

impl core::fmt::Display for MultiScalarTrustedStateCodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid multi-scalar local recovery magic"),
            Self::UnsupportedVersion { version } => {
                write!(
                    f,
                    "unsupported multi-scalar local recovery version {version}"
                )
            }
            Self::UnexpectedEof => write!(f, "truncated multi-scalar local recovery state"),
            Self::LengthOverflow => {
                write!(f, "multi-scalar local recovery length overflows limits")
            }
            Self::TrailingBytes => {
                write!(
                    f,
                    "multi-scalar local recovery state contains trailing bytes"
                )
            }
            Self::DuplicateDomain => write!(f, "multi-scalar local recovery repeats a domain"),
            Self::Domain(error) => write!(f, "invalid local recovery domain key: {error}"),
            Self::Scalar(error) => write!(f, "invalid nested scalar recovery state: {error}"),
        }
    }
}

impl std::error::Error for MultiScalarTrustedStateCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Domain(error) => Some(error),
            Self::Scalar(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ProjectionError> for MultiScalarTrustedStateCodecError {
    fn from(value: ProjectionError) -> Self {
        Self::Domain(value)
    }
}

impl From<TrustedStateCodecError> for MultiScalarTrustedStateCodecError {
    fn from(value: TrustedStateCodecError) -> Self {
        Self::Scalar(value)
    }
}

impl TrustedStateCodec<LocalScalarRecoveryState> for DevelopmentMultiScalarTrustedStateCodec {
    type Error = MultiScalarTrustedStateCodecError;

    fn encode(&self, state: &LocalScalarRecoveryState) -> Result<Vec<u8>, Self::Error> {
        encode_multi_scalar_recovery_state(state)
    }

    fn decode(&self, bytes: &[u8]) -> Result<LocalScalarRecoveryState, Self::Error> {
        decode_multi_scalar_recovery_state(bytes)
    }
}

pub fn encode_multi_scalar_recovery_state(
    state: &LocalScalarRecoveryState,
) -> Result<Vec<u8>, MultiScalarTrustedStateCodecError> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    write_len(&mut out, state.domains.len())?;

    for (key, snapshot) in &state.domains {
        out.extend_from_slice(key.atom_id.as_bytes());
        write_bytes(&mut out, &key.domain)?;
        let scalar = encode_local_scalar_snapshot(snapshot)?;
        write_bytes(&mut out, &scalar)?;
    }

    Ok(out)
}

pub fn decode_multi_scalar_recovery_state(
    encoded: &[u8],
) -> Result<LocalScalarRecoveryState, MultiScalarTrustedStateCodecError> {
    let mut reader = Reader::new(encoded);
    if reader.read_exact(MAGIC.len())? != MAGIC {
        return Err(MultiScalarTrustedStateCodecError::InvalidMagic);
    }

    let version = reader.read_u16()?;
    if version != VERSION {
        return Err(MultiScalarTrustedStateCodecError::UnsupportedVersion { version });
    }

    let count = reader.read_len()?;
    let mut domains = BTreeMap::new();
    for _ in 0..count {
        let atom_id = reader.read_atom_id()?;
        let domain = reader.read_vec()?;
        let key = DomainKey::new(atom_id, domain)?;
        let scalar_bytes = reader.read_vec()?;
        let snapshot = decode_local_scalar_snapshot(&scalar_bytes)?;
        if domains.insert(key, snapshot).is_some() {
            return Err(MultiScalarTrustedStateCodecError::DuplicateDomain);
        }
    }

    if !reader.is_finished() {
        return Err(MultiScalarTrustedStateCodecError::TrailingBytes);
    }

    Ok(LocalScalarRecoveryState { domains })
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), MultiScalarTrustedStateCodecError> {
    write_len(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_len(out: &mut Vec<u8>, len: usize) -> Result<(), MultiScalarTrustedStateCodecError> {
    let len = u64::try_from(len).map_err(|_| MultiScalarTrustedStateCodecError::LengthOverflow)?;
    out.extend_from_slice(&len.to_be_bytes());
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn is_finished(&self) -> bool {
        self.position == self.bytes.len()
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], MultiScalarTrustedStateCodecError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(MultiScalarTrustedStateCodecError::LengthOverflow)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(MultiScalarTrustedStateCodecError::UnexpectedEof)?;
        self.position = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, MultiScalarTrustedStateCodecError> {
        Ok(u16::from_be_bytes(
            self.read_exact(2)?.try_into().expect("fixed-length u16"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, MultiScalarTrustedStateCodecError> {
        Ok(u64::from_be_bytes(
            self.read_exact(8)?.try_into().expect("fixed-length u64"),
        ))
    }

    fn read_len(&mut self) -> Result<usize, MultiScalarTrustedStateCodecError> {
        usize::try_from(self.read_u64()?)
            .map_err(|_| MultiScalarTrustedStateCodecError::LengthOverflow)
    }

    fn read_vec(&mut self) -> Result<Vec<u8>, MultiScalarTrustedStateCodecError> {
        let len = self.read_len()?;
        Ok(self.read_exact(len)?.to_vec())
    }

    fn read_atom_id(&mut self) -> Result<AtomId, MultiScalarTrustedStateCodecError> {
        let bytes: [u8; LOGICAL_ID_BYTES] = self
            .read_exact(LOGICAL_ID_BYTES)?
            .try_into()
            .expect("fixed-length atom ID");
        Ok(AtomId::from_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use apc_core::{RevisionId, ScalarRegister, WorkingEpochId};

    use super::*;

    fn logical_bytes(value: u64) -> [u8; LOGICAL_ID_BYTES] {
        let mut bytes = [0_u8; LOGICAL_ID_BYTES];
        bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn atom(value: u64) -> AtomId {
        AtomId::from_bytes(logical_bytes(value))
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(logical_bytes(value))
    }

    fn wid(value: u64) -> WorkingEpochId {
        WorkingEpochId::from_bytes(logical_bytes(value))
    }

    fn key(atom_value: u64, domain: &str) -> DomainKey {
        DomainKey::new(atom(atom_value), domain.as_bytes()).unwrap()
    }

    fn base_snapshot(base_revision: u64, value: &str) -> LocalScalarSnapshot<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal
            .assign(rid(base_revision), value.as_bytes().to_vec())
            .unwrap();
        LocalScalarDomain::from_causal(causal).unwrap().snapshot()
    }

    fn pending_snapshot(
        base_revision: u64,
        epoch: u64,
        base: &str,
        draft: &str,
    ) -> LocalScalarSnapshot<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal
            .assign(rid(base_revision), base.as_bytes().to_vec())
            .unwrap();
        let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
        domain
            .begin_epoch(wid(epoch), draft.as_bytes().to_vec())
            .unwrap();
        domain.snapshot()
    }

    fn exposed_snapshot(
        base_revision: u64,
        local_revision: u64,
        epoch: u64,
        base: &str,
        value: &str,
    ) -> LocalScalarSnapshot<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal
            .assign(rid(base_revision), base.as_bytes().to_vec())
            .unwrap();
        let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
        domain
            .begin_epoch(wid(epoch), value.as_bytes().to_vec())
            .unwrap();
        domain.seal_local(rid(local_revision)).unwrap();
        domain.finalize(rid(local_revision)).unwrap();
        domain.handoff([rid(local_revision)]).unwrap();
        domain.snapshot()
    }

    #[test]
    fn deterministic_round_trip_preserves_independent_domain_recovery_state() {
        let body = key(1, "body");
        let title = key(1, "title");
        let state = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (
                body.clone(),
                exposed_snapshot(100, 200, 1, "body-base", "body-local"),
            ),
            (
                title.clone(),
                pending_snapshot(300, 2, "title-base", "title-draft"),
            ),
        ]))
        .unwrap();

        let first = encode_multi_scalar_recovery_state(&state).unwrap();
        let second = encode_multi_scalar_recovery_state(&state).unwrap();
        assert_eq!(first, second);

        let decoded = decode_multi_scalar_recovery_state(&first).unwrap();
        assert_eq!(decoded, state);
        assert!(decoded
            .restore_domain(&body)
            .unwrap()
            .unwrap()
            .finalization()
            .exposed_local_ids()
            .contains(&rid(200)));
        assert_eq!(
            decoded
                .restore_domain(&title)
                .unwrap()
                .unwrap()
                .pending()
                .unwrap()
                .id,
            wid(2)
        );
    }

    #[test]
    fn replacing_one_domain_does_not_mutate_another_domain() {
        let body = key(1, "body");
        let title = key(1, "title");
        let title_snapshot = pending_snapshot(300, 2, "title-base", "title-draft");
        let mut state = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (body.clone(), base_snapshot(100, "body-base")),
            (title.clone(), title_snapshot.clone()),
        ]))
        .unwrap();

        state
            .replace_domain(body, pending_snapshot(100, 9, "body-base", "body-draft"))
            .unwrap();

        assert_eq!(state.get(&title), Some(&title_snapshot));
    }

    #[test]
    fn canonical_encoding_does_not_depend_on_insertion_order() {
        let body = key(1, "body");
        let title = key(1, "title");
        let body_snapshot = base_snapshot(100, "body");
        let title_snapshot = base_snapshot(200, "title");

        let mut left = LocalScalarRecoveryState::new();
        left.insert_new(body.clone(), body_snapshot.clone())
            .unwrap();
        left.insert_new(title.clone(), title_snapshot.clone())
            .unwrap();

        let mut right = LocalScalarRecoveryState::new();
        right.insert_new(title, title_snapshot).unwrap();
        right.insert_new(body, body_snapshot).unwrap();

        assert_eq!(
            encode_multi_scalar_recovery_state(&left).unwrap(),
            encode_multi_scalar_recovery_state(&right).unwrap()
        );
    }

    #[test]
    fn duplicate_domain_is_rejected_by_container_and_decoder() {
        let domain = key(1, "body");
        let snapshot = base_snapshot(100, "base");
        let mut state = LocalScalarRecoveryState::new();
        state.insert_new(domain.clone(), snapshot.clone()).unwrap();
        assert!(matches!(
            state.insert_new(domain.clone(), snapshot.clone()),
            Err(LocalScalarRecoveryStateError::DuplicateDomain)
        ));

        let scalar = encode_local_scalar_snapshot(&snapshot).unwrap();
        let mut encoded = Vec::new();
        encoded.extend_from_slice(MAGIC);
        encoded.extend_from_slice(&VERSION.to_be_bytes());
        write_len(&mut encoded, 2).unwrap();
        for _ in 0..2 {
            encoded.extend_from_slice(domain.atom_id.as_bytes());
            write_bytes(&mut encoded, &domain.domain).unwrap();
            write_bytes(&mut encoded, &scalar).unwrap();
        }

        assert!(matches!(
            decode_multi_scalar_recovery_state(&encoded),
            Err(MultiScalarTrustedStateCodecError::DuplicateDomain)
        ));
    }

    #[test]
    fn invalid_domain_and_nested_scalar_fail_closed() {
        let snapshot = base_snapshot(100, "base");
        let scalar = encode_local_scalar_snapshot(&snapshot).unwrap();

        let mut empty_domain = Vec::new();
        empty_domain.extend_from_slice(MAGIC);
        empty_domain.extend_from_slice(&VERSION.to_be_bytes());
        write_len(&mut empty_domain, 1).unwrap();
        empty_domain.extend_from_slice(atom(1).as_bytes());
        write_bytes(&mut empty_domain, &[]).unwrap();
        write_bytes(&mut empty_domain, &scalar).unwrap();
        assert!(matches!(
            decode_multi_scalar_recovery_state(&empty_domain),
            Err(MultiScalarTrustedStateCodecError::Domain(
                ProjectionError::EmptyDomainIdentifier
            ))
        ));

        let mut bad_scalar = Vec::new();
        bad_scalar.extend_from_slice(MAGIC);
        bad_scalar.extend_from_slice(&VERSION.to_be_bytes());
        write_len(&mut bad_scalar, 1).unwrap();
        bad_scalar.extend_from_slice(atom(1).as_bytes());
        write_bytes(&mut bad_scalar, b"body").unwrap();
        write_bytes(&mut bad_scalar, &[0]).unwrap();
        assert!(matches!(
            decode_multi_scalar_recovery_state(&bad_scalar),
            Err(MultiScalarTrustedStateCodecError::Scalar(
                TrustedStateCodecError::UnexpectedEof
            ))
        ));
    }
}
