use std::collections::{BTreeMap, BTreeSet};

use apc_core::{AtomId, CoreError, RevisionId, ScalarRegister, ScalarRevision};

use crate::{DomainKey, ProjectionError, ScalarSyncProjection, SyncProjection};

const MAGIC: &[u8; 8] = b"APCSYNC1";
const VERSION: u16 = 1;
const ID_BYTES: usize = 32;

/// Errors for the explicitly pre-format scalar sync projection codec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncCodecError {
    InvalidMagic,
    UnsupportedVersion { version: u16 },
    UnexpectedEof,
    LengthOverflow,
    TrailingBytes,
    DuplicateDomain,
    DuplicateRevisionInSet,
    Projection(ProjectionError),
    Core(CoreError),
}

impl core::fmt::Display for SyncCodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid scalar sync projection magic"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported scalar sync projection version {version}")
            }
            Self::UnexpectedEof => write!(f, "truncated scalar sync projection"),
            Self::LengthOverflow => write!(f, "scalar sync projection length overflows limits"),
            Self::TrailingBytes => write!(f, "scalar sync projection contains trailing bytes"),
            Self::DuplicateDomain => write!(f, "scalar sync projection repeats a domain key"),
            Self::DuplicateRevisionInSet => {
                write!(f, "scalar sync projection repeats a revision ID in a set")
            }
            Self::Projection(error) => write!(f, "invalid sync projection key: {error}"),
            Self::Core(error) => write!(f, "invalid scalar sync state: {error}"),
        }
    }
}

impl std::error::Error for SyncCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Projection(error) => Some(error),
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CoreError> for SyncCodecError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

impl From<ProjectionError> for SyncCodecError {
    fn from(value: ProjectionError) -> Self {
        Self::Projection(value)
    }
}

/// Deterministically encode one clear scalar projection before protection.
///
/// This is a development/pre-format encoding and may change before `.apc` format
/// freeze. Determinism is useful now for differential tests and authenticated
/// context construction; it is not a compatibility promise yet.
pub fn encode_scalar_projection(
    projection: &ScalarSyncProjection,
) -> Result<Vec<u8>, SyncCodecError> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    write_len(&mut out, projection.len())?;

    for (key, register) in projection.domains() {
        register.validate()?;
        out.extend_from_slice(key.atom_id.as_bytes());
        write_bytes(&mut out, &key.domain)?;
        write_register(&mut out, register)?;
    }

    Ok(out)
}

pub fn decode_scalar_projection(encoded: &[u8]) -> Result<ScalarSyncProjection, SyncCodecError> {
    let mut reader = Reader::new(encoded);
    if reader.read_exact(MAGIC.len())? != MAGIC {
        return Err(SyncCodecError::InvalidMagic);
    }

    let version = reader.read_u16()?;
    if version != VERSION {
        return Err(SyncCodecError::UnsupportedVersion { version });
    }

    let domain_count = reader.read_len()?;
    let mut domains = BTreeMap::new();
    for _ in 0..domain_count {
        let atom_id = reader.read_atom_id()?;
        let domain = reader.read_vec()?;
        let key = DomainKey::new(atom_id, domain)?;
        let register = read_register(&mut reader)?;
        if domains.insert(key, register).is_some() {
            return Err(SyncCodecError::DuplicateDomain);
        }
    }

    if !reader.is_finished() {
        return Err(SyncCodecError::TrailingBytes);
    }

    Ok(SyncProjection::from_domains(domains))
}

fn write_register(
    out: &mut Vec<u8>,
    register: &ScalarRegister<Vec<u8>>,
) -> Result<(), SyncCodecError> {
    write_len(out, register.len())?;
    for revision in register.revisions() {
        out.extend_from_slice(revision.id.as_bytes());
        write_bytes(out, &revision.value)?;
        write_id_set(out, &revision.parents)?;
    }
    Ok(())
}

fn read_register(reader: &mut Reader<'_>) -> Result<ScalarRegister<Vec<u8>>, SyncCodecError> {
    let revision_count = reader.read_len()?;
    let mut revisions = Vec::new();
    for _ in 0..revision_count {
        let id = reader.read_revision_id()?;
        let value = reader.read_vec()?;
        let parents = read_id_set(reader)?;
        revisions.push(ScalarRevision::new(id, value, parents));
    }
    Ok(ScalarRegister::from_revisions(revisions)?)
}

fn write_id_set(out: &mut Vec<u8>, ids: &BTreeSet<RevisionId>) -> Result<(), SyncCodecError> {
    write_len(out, ids.len())?;
    for id in ids {
        out.extend_from_slice(id.as_bytes());
    }
    Ok(())
}

fn read_id_set(reader: &mut Reader<'_>) -> Result<BTreeSet<RevisionId>, SyncCodecError> {
    let count = reader.read_len()?;
    let mut ids = BTreeSet::new();
    for _ in 0..count {
        if !ids.insert(reader.read_revision_id()?) {
            return Err(SyncCodecError::DuplicateRevisionInSet);
        }
    }
    Ok(ids)
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), SyncCodecError> {
    write_len(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_len(out: &mut Vec<u8>, len: usize) -> Result<(), SyncCodecError> {
    let len = u64::try_from(len).map_err(|_| SyncCodecError::LengthOverflow)?;
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

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], SyncCodecError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(SyncCodecError::LengthOverflow)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(SyncCodecError::UnexpectedEof)?;
        self.position = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, SyncCodecError> {
        Ok(u16::from_be_bytes(
            self.read_exact(2)?.try_into().expect("fixed-length u16"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, SyncCodecError> {
        Ok(u64::from_be_bytes(
            self.read_exact(8)?.try_into().expect("fixed-length u64"),
        ))
    }

    fn read_len(&mut self) -> Result<usize, SyncCodecError> {
        usize::try_from(self.read_u64()?).map_err(|_| SyncCodecError::LengthOverflow)
    }

    fn read_vec(&mut self) -> Result<Vec<u8>, SyncCodecError> {
        let len = self.read_len()?;
        Ok(self.read_exact(len)?.to_vec())
    }

    fn read_atom_id(&mut self) -> Result<AtomId, SyncCodecError> {
        let bytes: [u8; ID_BYTES] = self
            .read_exact(ID_BYTES)?
            .try_into()
            .expect("fixed-length atom ID");
        Ok(AtomId::from_bytes(bytes))
    }

    fn read_revision_id(&mut self) -> Result<RevisionId, SyncCodecError> {
        let bytes: [u8; ID_BYTES] = self
            .read_exact(ID_BYTES)?
            .try_into()
            .expect("fixed-length revision ID");
        Ok(RevisionId::from_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: u64) -> [u8; ID_BYTES] {
        let mut bytes = [0_u8; ID_BYTES];
        bytes[ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn atom(value: u64) -> AtomId {
        AtomId::from_bytes(bytes(value))
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(bytes(value))
    }

    fn projection() -> ScalarSyncProjection {
        let body = DomainKey::new(atom(1), b"body".to_vec()).unwrap();
        let title = DomainKey::new(atom(1), b"title".to_vec()).unwrap();

        let mut body_register = ScalarRegister::new();
        body_register.assign(rid(10), b"base".to_vec()).unwrap();
        body_register.assign(rid(20), b"body".to_vec()).unwrap();

        let mut title_register = ScalarRegister::new();
        title_register.assign(rid(30), b"title".to_vec()).unwrap();

        SyncProjection::from_domains(BTreeMap::from([
            (body, body_register),
            (title, title_register),
        ]))
    }

    #[test]
    fn encoding_is_deterministic_and_round_trips() {
        let projection = projection();
        let first = encode_scalar_projection(&projection).unwrap();
        let second = encode_scalar_projection(&projection).unwrap();

        assert_eq!(first, second);
        assert_eq!(decode_scalar_projection(&first).unwrap(), projection);
    }

    #[test]
    fn malformed_magic_version_truncation_and_trailing_data_fail_closed() {
        let encoded = encode_scalar_projection(&projection()).unwrap();

        let mut bad_magic = encoded.clone();
        bad_magic[0] ^= 1;
        assert_eq!(
            decode_scalar_projection(&bad_magic).unwrap_err(),
            SyncCodecError::InvalidMagic
        );

        let mut bad_version = encoded.clone();
        bad_version[MAGIC.len()..MAGIC.len() + 2].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            decode_scalar_projection(&bad_version).unwrap_err(),
            SyncCodecError::UnsupportedVersion { version: 2 }
        );

        assert!(matches!(
            decode_scalar_projection(&encoded[..encoded.len() - 1]),
            Err(SyncCodecError::UnexpectedEof)
        ));

        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            decode_scalar_projection(&trailing).unwrap_err(),
            SyncCodecError::TrailingBytes
        );
    }
}
