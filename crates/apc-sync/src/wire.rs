use crate::{ProtectedSyncPart, PublicationId};

const MAGIC: &[u8; 8] = b"APCSPRT1";
const VERSION: u16 = 1;
const PUBLICATION_ID_BYTES: usize = 32;
const HEADER_BYTES: usize = MAGIC.len() + 2 + PUBLICATION_ID_BYTES + 4 + 4 + 8;

/// Errors for the transport-facing development encoding of one protected sync part.
///
/// This encoding carries only already-protected payload bytes plus clear multipart
/// bookkeeping. It is explicitly pre-format and may change before portable sync
/// encoding is frozen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectedPartCodecError {
    InvalidMagic,
    UnsupportedVersion { version: u16 },
    UnexpectedEof,
    LengthOverflow,
    InvalidMultipartMetadata,
    LengthMismatch,
}

impl core::fmt::Display for ProtectedPartCodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid protected sync part magic"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported protected sync part version {version}")
            }
            Self::UnexpectedEof => write!(f, "truncated protected sync part"),
            Self::LengthOverflow => write!(f, "protected sync part length overflows host limits"),
            Self::InvalidMultipartMetadata => {
                write!(f, "invalid protected sync multipart metadata")
            }
            Self::LengthMismatch => write!(f, "protected sync part length does not match payload"),
        }
    }
}

impl std::error::Error for ProtectedPartCodecError {}

pub fn encode_protected_sync_part(
    part: &ProtectedSyncPart,
) -> Result<Vec<u8>, ProtectedPartCodecError> {
    validate_metadata(part.part_index, part.total_parts)?;
    let payload_len =
        u64::try_from(part.payload.len()).map_err(|_| ProtectedPartCodecError::LengthOverflow)?;

    let mut encoded = Vec::with_capacity(HEADER_BYTES + part.payload.len());
    encoded.extend_from_slice(MAGIC);
    encoded.extend_from_slice(&VERSION.to_be_bytes());
    encoded.extend_from_slice(part.publication_id.as_bytes());
    encoded.extend_from_slice(&part.part_index.to_be_bytes());
    encoded.extend_from_slice(&part.total_parts.to_be_bytes());
    encoded.extend_from_slice(&payload_len.to_be_bytes());
    encoded.extend_from_slice(&part.payload);
    Ok(encoded)
}

pub fn decode_protected_sync_part(
    encoded: &[u8],
) -> Result<ProtectedSyncPart, ProtectedPartCodecError> {
    if encoded.len() < HEADER_BYTES {
        return Err(ProtectedPartCodecError::UnexpectedEof);
    }
    if &encoded[..MAGIC.len()] != MAGIC {
        return Err(ProtectedPartCodecError::InvalidMagic);
    }

    let mut reader = Reader::new(&encoded[MAGIC.len()..]);
    let version = reader.read_u16()?;
    if version != VERSION {
        return Err(ProtectedPartCodecError::UnsupportedVersion { version });
    }

    let publication_id = PublicationId::from_bytes(
        reader
            .read_exact(PUBLICATION_ID_BYTES)?
            .try_into()
            .expect("fixed-length publication ID"),
    );
    let part_index = reader.read_u32()?;
    let total_parts = reader.read_u32()?;
    validate_metadata(part_index, total_parts)?;

    let payload_len =
        usize::try_from(reader.read_u64()?).map_err(|_| ProtectedPartCodecError::LengthOverflow)?;
    if reader.remaining() != payload_len {
        return Err(ProtectedPartCodecError::LengthMismatch);
    }
    let payload = reader.read_exact(payload_len)?.to_vec();

    Ok(ProtectedSyncPart {
        publication_id,
        part_index,
        total_parts,
        payload,
    })
}

fn validate_metadata(part_index: u32, total_parts: u32) -> Result<(), ProtectedPartCodecError> {
    if total_parts == 0 || part_index >= total_parts {
        return Err(ProtectedPartCodecError::InvalidMultipartMetadata);
    }
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

    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], ProtectedPartCodecError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(ProtectedPartCodecError::LengthOverflow)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(ProtectedPartCodecError::UnexpectedEof)?;
        self.position = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, ProtectedPartCodecError> {
        Ok(u16::from_be_bytes(
            self.read_exact(2)?.try_into().expect("fixed-length u16"),
        ))
    }

    fn read_u32(&mut self) -> Result<u32, ProtectedPartCodecError> {
        Ok(u32::from_be_bytes(
            self.read_exact(4)?.try_into().expect("fixed-length u32"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, ProtectedPartCodecError> {
        Ok(u64::from_be_bytes(
            self.read_exact(8)?.try_into().expect("fixed-length u64"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use apc_core::{AtomId, ContinuumId, RevisionId, ScalarRegister};
    use apc_crypto::ContentKey;

    use super::*;
    use crate::{protect_scalar_part, unprotect_scalar_part, DomainKey, SyncProjection};

    fn bytes(value: u64) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn cid(value: u64) -> ContinuumId {
        ContinuumId::from_bytes(bytes(value))
    }

    fn atom(value: u64) -> AtomId {
        AtomId::from_bytes(bytes(value))
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(bytes(value))
    }

    fn pid(value: u64) -> PublicationId {
        PublicationId::from_bytes(bytes(value))
    }

    fn protected_part() -> (ContentKey, ProtectedSyncPart) {
        let key = ContentKey::from_bytes([0xa1; 32]);
        let domain = DomainKey::new(atom(1), b"body".to_vec()).unwrap();
        let mut register = ScalarRegister::new();
        register.assign(rid(1), b"wire-secret".to_vec()).unwrap();
        let projection = SyncProjection::from_domains(BTreeMap::from([(domain, register)]));
        let part = protect_scalar_part(&key, cid(7), pid(9), 1, 3, &projection).unwrap();
        (key, part)
    }

    #[test]
    fn deterministic_wire_encoding_round_trips_protected_part() {
        let (_key, part) = protected_part();
        let first = encode_protected_sync_part(&part).unwrap();
        let second = encode_protected_sync_part(&part).unwrap();

        assert_eq!(first, second);
        assert_eq!(decode_protected_sync_part(&first).unwrap(), part);
        assert!(!first
            .windows(b"wire-secret".len())
            .any(|window| window == b"wire-secret"));
    }

    #[test]
    fn decoded_part_still_requires_aead_authentication() {
        let (key, part) = protected_part();
        let mut encoded = encode_protected_sync_part(&part).unwrap();

        let part_index_offset = MAGIC.len() + 2 + PUBLICATION_ID_BYTES;
        encoded[part_index_offset + 3] ^= 1;
        let changed = decode_protected_sync_part(&encoded).unwrap();

        assert!(unprotect_scalar_part(&key, cid(7), &changed).is_err());
    }

    #[test]
    fn malformed_version_metadata_lengths_and_trailing_bytes_fail_closed() {
        let (_key, part) = protected_part();
        let encoded = encode_protected_sync_part(&part).unwrap();

        let mut bad_magic = encoded.clone();
        bad_magic[0] ^= 1;
        assert_eq!(
            decode_protected_sync_part(&bad_magic).unwrap_err(),
            ProtectedPartCodecError::InvalidMagic
        );

        let mut bad_version = encoded.clone();
        bad_version[MAGIC.len()..MAGIC.len() + 2].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            decode_protected_sync_part(&bad_version).unwrap_err(),
            ProtectedPartCodecError::UnsupportedVersion { version: 2 }
        );

        let total_offset = MAGIC.len() + 2 + PUBLICATION_ID_BYTES + 4;
        let mut zero_total = encoded.clone();
        zero_total[total_offset..total_offset + 4].copy_from_slice(&0_u32.to_be_bytes());
        assert_eq!(
            decode_protected_sync_part(&zero_total).unwrap_err(),
            ProtectedPartCodecError::InvalidMultipartMetadata
        );

        assert!(matches!(
            decode_protected_sync_part(&encoded[..HEADER_BYTES - 1]),
            Err(ProtectedPartCodecError::UnexpectedEof)
        ));

        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            decode_protected_sync_part(&trailing).unwrap_err(),
            ProtectedPartCodecError::LengthMismatch
        );
    }
}
