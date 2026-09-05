use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use apc_core::{
    CoreError, FinalizationSnapshot, FinalizedStatement, LocalScalarDomain, LocalScalarSnapshot,
    RevisionId, ScalarRegister, ScalarRevision, WorkingEpoch, WorkingEpochId, WorkingSnapshot,
};

const MAGIC: &[u8; 8] = b"APCLSNP1";
const VERSION: u16 = 1;
const ID_BYTES: usize = 32;

/// Errors for the development local-scalar recovery codec.
///
/// This codec is explicitly pre-format. It exists so real core recovery state can
/// cross the durability backend before the native `.apc` encoding is frozen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotCodecError {
    InvalidMagic,
    UnsupportedVersion { version: u16 },
    UnexpectedEof,
    LengthOverflow,
    InvalidOptionTag { tag: u8 },
    InvalidStructure(&'static str),
    TrailingBytes,
    Core(CoreError),
}

impl fmt::Display for SnapshotCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid local-scalar recovery magic"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported local-scalar recovery version {version}")
            }
            Self::UnexpectedEof => write!(f, "truncated local-scalar recovery snapshot"),
            Self::LengthOverflow => write!(f, "local-scalar recovery length overflows limits"),
            Self::InvalidOptionTag { tag } => {
                write!(f, "invalid local-scalar recovery option tag {tag}")
            }
            Self::InvalidStructure(message) => {
                write!(f, "invalid local-scalar recovery structure: {message}")
            }
            Self::TrailingBytes => write!(f, "local-scalar recovery snapshot has trailing bytes"),
            Self::Core(error) => write!(f, "invalid recovered core state: {error}"),
        }
    }
}

impl std::error::Error for SnapshotCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CoreError> for SnapshotCodecError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

pub fn encode_local_scalar_snapshot(
    snapshot: &LocalScalarSnapshot<Vec<u8>>,
) -> Result<Vec<u8>, SnapshotCodecError> {
    // Validate the complete recovery object before writing bytes. The returned
    // temporary domain is intentionally discarded; validation is the purpose.
    let _validated = LocalScalarDomain::restore(snapshot.clone())?;

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());

    write_register(&mut out, &snapshot.working.causal)?;
    write_pending(&mut out, snapshot.working.pending.as_ref())?;
    write_id_set(&mut out, &snapshot.finalization.local_revision_ids)?;
    write_finalized(&mut out, &snapshot.finalization.finalized)?;
    write_id_set(&mut out, &snapshot.finalization.exposed_local_ids)?;
    write_id_set(&mut out, &snapshot.finalization.handed_off_local_ids)?;
    Ok(out)
}

pub fn decode_local_scalar_snapshot(
    encoded: &[u8],
) -> Result<LocalScalarSnapshot<Vec<u8>>, SnapshotCodecError> {
    let mut reader = Reader::new(encoded);
    if reader.read_exact(MAGIC.len())? != MAGIC {
        return Err(SnapshotCodecError::InvalidMagic);
    }

    let version = reader.read_u16()?;
    if version != VERSION {
        return Err(SnapshotCodecError::UnsupportedVersion { version });
    }

    let causal = read_register(&mut reader)?;
    let pending = read_pending(&mut reader)?;
    let local_revision_ids = read_id_set(&mut reader)?;
    let finalized = read_finalized(&mut reader)?;
    let exposed_local_ids = read_id_set(&mut reader)?;
    let handed_off_local_ids = read_id_set(&mut reader)?;

    if !reader.is_finished() {
        return Err(SnapshotCodecError::TrailingBytes);
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

    let _validated = LocalScalarDomain::restore(snapshot.clone())?;
    Ok(snapshot)
}

fn write_register(
    out: &mut Vec<u8>,
    register: &ScalarRegister<Vec<u8>>,
) -> Result<(), SnapshotCodecError> {
    write_len(out, register.len())?;
    for revision in register.revisions() {
        write_revision_id(out, revision.id);
        write_bytes(out, &revision.value)?;
        write_id_set(out, &revision.parents)?;
    }
    Ok(())
}

fn read_register(reader: &mut Reader<'_>) -> Result<ScalarRegister<Vec<u8>>, SnapshotCodecError> {
    let count = reader.read_len()?;
    let mut revisions = Vec::with_capacity(count);
    for _ in 0..count {
        let id = reader.read_revision_id()?;
        let value = reader.read_vec()?;
        let parents = read_id_set(reader)?;
        revisions.push(ScalarRevision::new(id, value, parents));
    }
    Ok(ScalarRegister::from_revisions(revisions)?)
}

fn write_pending(
    out: &mut Vec<u8>,
    pending: Option<&WorkingEpoch<Vec<u8>>>,
) -> Result<(), SnapshotCodecError> {
    match pending {
        None => out.push(0),
        Some(epoch) => {
            out.push(1);
            out.extend_from_slice(epoch.id.as_bytes());
            write_bytes(out, &epoch.value)?;
            write_id_set(out, &epoch.observed_frontier)?;
        }
    }
    Ok(())
}

fn read_pending(
    reader: &mut Reader<'_>,
) -> Result<Option<WorkingEpoch<Vec<u8>>>, SnapshotCodecError> {
    match reader.read_u8()? {
        0 => Ok(None),
        1 => Ok(Some(WorkingEpoch {
            id: reader.read_working_epoch_id()?,
            value: reader.read_vec()?,
            observed_frontier: read_id_set(reader)?,
        })),
        tag => Err(SnapshotCodecError::InvalidOptionTag { tag }),
    }
}

fn write_finalized(
    out: &mut Vec<u8>,
    finalized: &BTreeMap<RevisionId, FinalizedStatement<Vec<u8>>>,
) -> Result<(), SnapshotCodecError> {
    write_len(out, finalized.len())?;
    for (revision_id, statement) in finalized {
        if revision_id != &statement.revision_id {
            return Err(SnapshotCodecError::InvalidStructure(
                "finalized map key differs from statement revision ID",
            ));
        }
        write_revision_id(out, statement.revision_id);
        write_bytes(out, &statement.value)?;
        write_id_set(out, &statement.parents)?;
    }
    Ok(())
}

fn read_finalized(
    reader: &mut Reader<'_>,
) -> Result<BTreeMap<RevisionId, FinalizedStatement<Vec<u8>>>, SnapshotCodecError> {
    let count = reader.read_len()?;
    let mut finalized = BTreeMap::new();
    for _ in 0..count {
        let revision_id = reader.read_revision_id()?;
        let statement = FinalizedStatement {
            revision_id,
            value: reader.read_vec()?,
            parents: read_id_set(reader)?,
        };
        if finalized.insert(revision_id, statement).is_some() {
            return Err(SnapshotCodecError::InvalidStructure(
                "duplicate finalized revision ID",
            ));
        }
    }
    Ok(finalized)
}

fn write_id_set(
    out: &mut Vec<u8>,
    ids: &BTreeSet<RevisionId>,
) -> Result<(), SnapshotCodecError> {
    write_len(out, ids.len())?;
    for id in ids {
        write_revision_id(out, *id);
    }
    Ok(())
}

fn read_id_set(reader: &mut Reader<'_>) -> Result<BTreeSet<RevisionId>, SnapshotCodecError> {
    let count = reader.read_len()?;
    let mut ids = BTreeSet::new();
    for _ in 0..count {
        if !ids.insert(reader.read_revision_id()?) {
            return Err(SnapshotCodecError::InvalidStructure(
                "duplicate revision ID in set",
            ));
        }
    }
    Ok(ids)
}

fn write_revision_id(out: &mut Vec<u8>, id: RevisionId) {
    out.extend_from_slice(id.as_bytes());
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), SnapshotCodecError> {
    write_len(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_len(out: &mut Vec<u8>, len: usize) -> Result<(), SnapshotCodecError> {
    let len = u64::try_from(len).map_err(|_| SnapshotCodecError::LengthOverflow)?;
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

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], SnapshotCodecError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(SnapshotCodecError::LengthOverflow)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(SnapshotCodecError::UnexpectedEof)?;
        self.position = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, SnapshotCodecError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, SnapshotCodecError> {
        Ok(u16::from_be_bytes(
            self.read_exact(2)?.try_into().expect("fixed length slice"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, SnapshotCodecError> {
        Ok(u64::from_be_bytes(
            self.read_exact(8)?.try_into().expect("fixed length slice"),
        ))
    }

    fn read_len(&mut self) -> Result<usize, SnapshotCodecError> {
        usize::try_from(self.read_u64()?).map_err(|_| SnapshotCodecError::LengthOverflow)
    }

    fn read_vec(&mut self) -> Result<Vec<u8>, SnapshotCodecError> {
        let len = self.read_len()?;
        Ok(self.read_exact(len)?.to_vec())
    }

    fn read_revision_id(&mut self) -> Result<RevisionId, SnapshotCodecError> {
        let bytes: [u8; ID_BYTES] = self
            .read_exact(ID_BYTES)?
            .try_into()
            .expect("fixed length revision ID");
        Ok(RevisionId::from_bytes(bytes))
    }

    fn read_working_epoch_id(&mut self) -> Result<WorkingEpochId, SnapshotCodecError> {
        let bytes: [u8; ID_BYTES] = self
            .read_exact(ID_BYTES)?
            .try_into()
            .expect("fixed length working epoch ID");
        Ok(WorkingEpochId::from_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_bytes(value: u64) -> [u8; ID_BYTES] {
        let mut bytes = [0_u8; ID_BYTES];
        bytes[ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(id_bytes(value))
    }

    fn wid(value: u64) -> WorkingEpochId {
        WorkingEpochId::from_bytes(id_bytes(value))
    }

    fn rich_snapshot() -> LocalScalarSnapshot<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), b"base".to_vec()).unwrap();
        let mut domain = LocalScalarDomain::from_causal(causal).unwrap();

        domain.begin_epoch(wid(1), b"local".to_vec()).unwrap();
        domain.seal_local(rid(200)).unwrap();
        domain.finalize(rid(200)).unwrap();
        domain.handoff([rid(200)]).unwrap();
        domain.begin_epoch(wid(2), b"pending".to_vec()).unwrap();
        domain.update_pending(b"pending-latest".to_vec()).unwrap();
        domain.snapshot()
    }

    #[test]
    fn deterministic_round_trip_preserves_complete_recovery_state() {
        let snapshot = rich_snapshot();
        let first = encode_local_scalar_snapshot(&snapshot).unwrap();
        let second = encode_local_scalar_snapshot(&snapshot).unwrap();

        assert_eq!(first, second);
        assert_eq!(decode_local_scalar_snapshot(&first).unwrap(), snapshot);
    }

    #[test]
    fn decoded_snapshot_restores_domain_exactly() {
        let snapshot = rich_snapshot();
        let encoded = encode_local_scalar_snapshot(&snapshot).unwrap();
        let decoded = decode_local_scalar_snapshot(&encoded).unwrap();

        let original = LocalScalarDomain::restore(snapshot).unwrap();
        let restored = LocalScalarDomain::restore(decoded).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn trailing_and_truncated_bytes_fail_closed() {
        let encoded = encode_local_scalar_snapshot(&rich_snapshot()).unwrap();

        let mut trailing = encoded.clone();
        trailing.push(0);
        assert_eq!(
            decode_local_scalar_snapshot(&trailing).unwrap_err(),
            SnapshotCodecError::TrailingBytes
        );

        assert!(matches!(
            decode_local_scalar_snapshot(&encoded[..encoded.len() - 1]),
            Err(SnapshotCodecError::UnexpectedEof)
        ));
    }

    #[test]
    fn invalid_magic_and_version_fail_closed() {
        let encoded = encode_local_scalar_snapshot(&rich_snapshot()).unwrap();

        let mut bad_magic = encoded.clone();
        bad_magic[0] ^= 1;
        assert_eq!(
            decode_local_scalar_snapshot(&bad_magic).unwrap_err(),
            SnapshotCodecError::InvalidMagic
        );

        let mut bad_version = encoded;
        bad_version[MAGIC.len()..MAGIC.len() + 2].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            decode_local_scalar_snapshot(&bad_version).unwrap_err(),
            SnapshotCodecError::UnsupportedVersion { version: 2 }
        );
    }
}
