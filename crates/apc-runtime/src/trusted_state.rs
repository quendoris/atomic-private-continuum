use std::collections::{BTreeMap, BTreeSet};

use apc_core::id::LOGICAL_ID_BYTES;
use apc_core::{
    CoreError, FinalizationSnapshot, FinalizedStatement, LocalScalarDomain, LocalScalarSnapshot,
    RevisionId, ScalarRegister, ScalarRevision, WorkingEpoch, WorkingEpochId, WorkingSnapshot,
};

use crate::TrustedStateCodec;

const MAGIC: &[u8; 8] = b"APCLREC1";
const VERSION: u16 = 1;

/// Deterministic development codec for one byte-valued local scalar recovery
/// snapshot.
///
/// This is deliberately a local/runtime recovery encoding. It is not the native
/// `.apc` format, not the sync capsule format and not a compatibility commitment.
/// Its purpose is to make the scalar semantic exposure/durable-outbox path
/// restartable without relying on an in-memory test vault.
#[derive(Clone, Copy, Debug, Default)]
pub struct DevelopmentScalarTrustedStateCodec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustedStateCodecError {
    InvalidMagic,
    UnsupportedVersion { version: u16 },
    UnexpectedEof,
    LengthOverflow,
    TrailingBytes,
    InvalidBoolean,
    DuplicateRevisionInSet,
    DuplicateFinalizedStatement,
    Core(CoreError),
}

impl core::fmt::Display for TrustedStateCodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid local scalar recovery magic"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported local scalar recovery version {version}")
            }
            Self::UnexpectedEof => write!(f, "truncated local scalar recovery state"),
            Self::LengthOverflow => write!(f, "local scalar recovery length overflows limits"),
            Self::TrailingBytes => write!(f, "local scalar recovery state contains trailing bytes"),
            Self::InvalidBoolean => {
                write!(f, "local scalar recovery state contains invalid boolean")
            }
            Self::DuplicateRevisionInSet => {
                write!(
                    f,
                    "local scalar recovery state repeats a revision ID in a set"
                )
            }
            Self::DuplicateFinalizedStatement => {
                write!(
                    f,
                    "local scalar recovery state repeats a finalized revision"
                )
            }
            Self::Core(error) => write!(f, "invalid local scalar semantic state: {error}"),
        }
    }
}

impl std::error::Error for TrustedStateCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CoreError> for TrustedStateCodecError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

impl TrustedStateCodec<LocalScalarSnapshot<Vec<u8>>> for DevelopmentScalarTrustedStateCodec {
    type Error = TrustedStateCodecError;

    fn encode(&self, state: &LocalScalarSnapshot<Vec<u8>>) -> Result<Vec<u8>, Self::Error> {
        encode_local_scalar_snapshot(state)
    }

    fn decode(&self, bytes: &[u8]) -> Result<LocalScalarSnapshot<Vec<u8>>, Self::Error> {
        decode_local_scalar_snapshot(bytes)
    }
}

/// Encode one byte-valued scalar recovery snapshot deterministically.
///
/// The snapshot is validated through `LocalScalarDomain::restore()` before any
/// bytes are emitted, so malformed finalization/exposure bookkeeping cannot be
/// made durable through this codec.
pub fn encode_local_scalar_snapshot(
    snapshot: &LocalScalarSnapshot<Vec<u8>>,
) -> Result<Vec<u8>, TrustedStateCodecError> {
    LocalScalarDomain::restore(snapshot.clone())?;

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());

    write_register(&mut out, &snapshot.working.causal)?;
    match &snapshot.working.pending {
        None => out.push(0),
        Some(epoch) => {
            out.push(1);
            out.extend_from_slice(epoch.id.as_bytes());
            write_bytes(&mut out, &epoch.value)?;
            write_id_set(&mut out, &epoch.observed_frontier)?;
        }
    }

    write_id_set(&mut out, &snapshot.finalization.local_revision_ids)?;
    write_len(&mut out, snapshot.finalization.finalized.len())?;
    for (revision_id, statement) in &snapshot.finalization.finalized {
        out.extend_from_slice(revision_id.as_bytes());
        write_bytes(&mut out, &statement.value)?;
        write_id_set(&mut out, &statement.parents)?;
    }
    write_id_set(&mut out, &snapshot.finalization.exposed_local_ids)?;
    write_id_set(&mut out, &snapshot.finalization.handed_off_local_ids)?;

    Ok(out)
}

/// Decode and semantically validate one byte-valued scalar recovery snapshot.
pub fn decode_local_scalar_snapshot(
    encoded: &[u8],
) -> Result<LocalScalarSnapshot<Vec<u8>>, TrustedStateCodecError> {
    let mut reader = Reader::new(encoded);
    if reader.read_exact(MAGIC.len())? != MAGIC {
        return Err(TrustedStateCodecError::InvalidMagic);
    }

    let version = reader.read_u16()?;
    if version != VERSION {
        return Err(TrustedStateCodecError::UnsupportedVersion { version });
    }

    let causal = read_register(&mut reader)?;
    let pending = match reader.read_u8()? {
        0 => None,
        1 => Some(WorkingEpoch {
            id: reader.read_working_epoch_id()?,
            value: reader.read_vec()?,
            observed_frontier: read_id_set(&mut reader)?,
        }),
        _ => return Err(TrustedStateCodecError::InvalidBoolean),
    };

    let local_revision_ids = read_id_set(&mut reader)?;
    let finalized_count = reader.read_len()?;
    let mut finalized = BTreeMap::new();
    for _ in 0..finalized_count {
        let revision_id = reader.read_revision_id()?;
        let statement = FinalizedStatement {
            revision_id,
            value: reader.read_vec()?,
            parents: read_id_set(&mut reader)?,
        };
        if finalized.insert(revision_id, statement).is_some() {
            return Err(TrustedStateCodecError::DuplicateFinalizedStatement);
        }
    }
    let exposed_local_ids = read_id_set(&mut reader)?;
    let handed_off_local_ids = read_id_set(&mut reader)?;

    if !reader.is_finished() {
        return Err(TrustedStateCodecError::TrailingBytes);
    }

    let snapshot = LocalScalarSnapshot {
        working: WorkingSnapshot { causal, pending },
        finalization: FinalizationSnapshot {
            local_revision_ids,
            finalized,
            exposed_local_ids,
            handed_off_local_ids,
        },
    };
    LocalScalarDomain::restore(snapshot.clone())?;
    Ok(snapshot)
}

fn write_register(
    out: &mut Vec<u8>,
    register: &ScalarRegister<Vec<u8>>,
) -> Result<(), TrustedStateCodecError> {
    register.validate()?;
    write_len(out, register.len())?;
    for revision in register.revisions() {
        out.extend_from_slice(revision.id.as_bytes());
        write_bytes(out, &revision.value)?;
        write_id_set(out, &revision.parents)?;
    }
    Ok(())
}

fn read_register(
    reader: &mut Reader<'_>,
) -> Result<ScalarRegister<Vec<u8>>, TrustedStateCodecError> {
    let revision_count = reader.read_len()?;
    let mut revisions = Vec::new();
    for _ in 0..revision_count {
        revisions.push(ScalarRevision::new(
            reader.read_revision_id()?,
            reader.read_vec()?,
            read_id_set(reader)?,
        ));
    }
    Ok(ScalarRegister::from_revisions(revisions)?)
}

fn write_id_set(
    out: &mut Vec<u8>,
    ids: &BTreeSet<RevisionId>,
) -> Result<(), TrustedStateCodecError> {
    write_len(out, ids.len())?;
    for id in ids {
        out.extend_from_slice(id.as_bytes());
    }
    Ok(())
}

fn read_id_set(reader: &mut Reader<'_>) -> Result<BTreeSet<RevisionId>, TrustedStateCodecError> {
    let count = reader.read_len()?;
    let mut ids = BTreeSet::new();
    for _ in 0..count {
        if !ids.insert(reader.read_revision_id()?) {
            return Err(TrustedStateCodecError::DuplicateRevisionInSet);
        }
    }
    Ok(ids)
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), TrustedStateCodecError> {
    write_len(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_len(out: &mut Vec<u8>, len: usize) -> Result<(), TrustedStateCodecError> {
    let len = u64::try_from(len).map_err(|_| TrustedStateCodecError::LengthOverflow)?;
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

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], TrustedStateCodecError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(TrustedStateCodecError::LengthOverflow)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(TrustedStateCodecError::UnexpectedEof)?;
        self.position = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, TrustedStateCodecError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, TrustedStateCodecError> {
        Ok(u16::from_be_bytes(
            self.read_exact(2)?.try_into().expect("fixed-length u16"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, TrustedStateCodecError> {
        Ok(u64::from_be_bytes(
            self.read_exact(8)?.try_into().expect("fixed-length u64"),
        ))
    }

    fn read_len(&mut self) -> Result<usize, TrustedStateCodecError> {
        usize::try_from(self.read_u64()?).map_err(|_| TrustedStateCodecError::LengthOverflow)
    }

    fn read_vec(&mut self) -> Result<Vec<u8>, TrustedStateCodecError> {
        let len = self.read_len()?;
        Ok(self.read_exact(len)?.to_vec())
    }

    fn read_revision_id(&mut self) -> Result<RevisionId, TrustedStateCodecError> {
        let bytes: [u8; LOGICAL_ID_BYTES] = self
            .read_exact(LOGICAL_ID_BYTES)?
            .try_into()
            .expect("fixed-length revision ID");
        Ok(RevisionId::from_bytes(bytes))
    }

    fn read_working_epoch_id(&mut self) -> Result<WorkingEpochId, TrustedStateCodecError> {
        let bytes: [u8; LOGICAL_ID_BYTES] = self
            .read_exact(LOGICAL_ID_BYTES)?
            .try_into()
            .expect("fixed-length working epoch ID");
        Ok(WorkingEpochId::from_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logical_bytes(value: u64) -> [u8; LOGICAL_ID_BYTES] {
        let mut bytes = [0_u8; LOGICAL_ID_BYTES];
        bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(logical_bytes(value))
    }

    fn wid(value: u64) -> WorkingEpochId {
        WorkingEpochId::from_bytes(logical_bytes(value))
    }

    fn exposed_snapshot() -> LocalScalarSnapshot<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), b"base".to_vec()).unwrap();
        let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
        domain.begin_epoch(wid(1), b"local".to_vec()).unwrap();
        domain.seal_local(rid(200)).unwrap();
        domain.finalize(rid(200)).unwrap();
        domain.handoff([rid(200)]).unwrap();
        domain.snapshot()
    }

    #[test]
    fn deterministic_encoding_round_trips_exposure_and_finalization() {
        let snapshot = exposed_snapshot();
        let first = encode_local_scalar_snapshot(&snapshot).unwrap();
        let second = encode_local_scalar_snapshot(&snapshot).unwrap();

        assert_eq!(first, second);
        assert_eq!(decode_local_scalar_snapshot(&first).unwrap(), snapshot);
    }

    #[test]
    fn pending_epoch_round_trips_with_original_observed_frontier() {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), b"base".to_vec()).unwrap();
        let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
        domain.begin_epoch(wid(9), b"draft".to_vec()).unwrap();
        domain.update_pending(b"latest-draft".to_vec()).unwrap();
        let snapshot = domain.snapshot();

        let decoded =
            decode_local_scalar_snapshot(&encode_local_scalar_snapshot(&snapshot).unwrap())
                .unwrap();
        assert_eq!(decoded, snapshot);
        assert_eq!(decoded.working.pending.unwrap().id, wid(9));
    }

    #[test]
    fn malformed_magic_version_truncation_and_trailing_bytes_fail_closed() {
        let encoded = encode_local_scalar_snapshot(&exposed_snapshot()).unwrap();

        let mut bad_magic = encoded.clone();
        bad_magic[0] ^= 1;
        assert_eq!(
            decode_local_scalar_snapshot(&bad_magic).unwrap_err(),
            TrustedStateCodecError::InvalidMagic
        );

        let mut bad_version = encoded.clone();
        bad_version[MAGIC.len()..MAGIC.len() + 2].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            decode_local_scalar_snapshot(&bad_version).unwrap_err(),
            TrustedStateCodecError::UnsupportedVersion { version: 2 }
        );

        assert!(matches!(
            decode_local_scalar_snapshot(&encoded[..encoded.len() - 1]),
            Err(TrustedStateCodecError::UnexpectedEof)
        ));

        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            decode_local_scalar_snapshot(&trailing).unwrap_err(),
            TrustedStateCodecError::TrailingBytes
        );
    }

    #[test]
    fn semantically_invalid_pending_frontier_is_rejected_before_encoding() {
        let mut snapshot = exposed_snapshot();
        snapshot.working.pending = Some(WorkingEpoch {
            id: wid(10),
            value: b"forged".to_vec(),
            observed_frontier: BTreeSet::from([rid(999)]),
        });

        assert!(matches!(
            encode_local_scalar_snapshot(&snapshot),
            Err(TrustedStateCodecError::Core(
                CoreError::InvalidWorkingSnapshot
            ))
        ));
    }
}
