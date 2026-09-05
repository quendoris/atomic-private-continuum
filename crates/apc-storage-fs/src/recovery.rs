use core::fmt;

const MAGIC: &[u8; 8] = b"APCDEV01";
const VERSION: u16 = 1;
const HEADER_LEN: usize = MAGIC.len() + 2 + 8;
const CHECKSUM_LEN: usize = 4;

/// Error returned when the development recovery envelope is structurally invalid.
///
/// This envelope is deliberately not the portable `.apc` format and the checksum
/// is deliberately not an authenticity mechanism. It only makes torn/truncated or
/// accidentally modified development snapshots fail closed before the real
/// authenticated portable encoding exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryDecodeError {
    TooShort,
    InvalidMagic,
    UnsupportedVersion { version: u16 },
    LengthOverflow,
    LengthMismatch,
    ChecksumMismatch,
}

impl fmt::Display for RecoveryDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(f, "recovery envelope is too short"),
            Self::InvalidMagic => write!(f, "recovery envelope has invalid magic"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported recovery envelope version {version}")
            }
            Self::LengthOverflow => write!(f, "recovery envelope length overflows host limits"),
            Self::LengthMismatch => write!(f, "recovery envelope length does not match payload"),
            Self::ChecksumMismatch => write!(f, "recovery envelope checksum mismatch"),
        }
    }
}

impl std::error::Error for RecoveryDecodeError {}

pub(crate) fn encode(payload: &[u8]) -> Vec<u8> {
    let payload_len = u64::try_from(payload.len()).expect("usize payload length must fit into u64");
    let mut encoded = Vec::with_capacity(HEADER_LEN + payload.len() + CHECKSUM_LEN);
    encoded.extend_from_slice(MAGIC);
    encoded.extend_from_slice(&VERSION.to_be_bytes());
    encoded.extend_from_slice(&payload_len.to_be_bytes());
    encoded.extend_from_slice(payload);

    let checksum = crc32_ieee(&encoded);
    encoded.extend_from_slice(&checksum.to_be_bytes());
    encoded
}

pub(crate) fn decode(encoded: &[u8]) -> Result<Vec<u8>, RecoveryDecodeError> {
    if encoded.len() < HEADER_LEN + CHECKSUM_LEN {
        return Err(RecoveryDecodeError::TooShort);
    }
    if &encoded[..MAGIC.len()] != MAGIC {
        return Err(RecoveryDecodeError::InvalidMagic);
    }

    let version_offset = MAGIC.len();
    let version = u16::from_be_bytes([encoded[version_offset], encoded[version_offset + 1]]);
    if version != VERSION {
        return Err(RecoveryDecodeError::UnsupportedVersion { version });
    }

    let length_offset = version_offset + 2;
    let payload_len_u64 = u64::from_be_bytes(
        encoded[length_offset..length_offset + 8]
            .try_into()
            .expect("fixed length slice"),
    );
    let payload_len =
        usize::try_from(payload_len_u64).map_err(|_| RecoveryDecodeError::LengthOverflow)?;
    let expected_len = HEADER_LEN
        .checked_add(payload_len)
        .and_then(|value| value.checked_add(CHECKSUM_LEN))
        .ok_or(RecoveryDecodeError::LengthOverflow)?;
    if encoded.len() != expected_len {
        return Err(RecoveryDecodeError::LengthMismatch);
    }

    let checksum_offset = HEADER_LEN + payload_len;
    let stored_checksum = u32::from_be_bytes(
        encoded[checksum_offset..]
            .try_into()
            .expect("checksum slice has exact length"),
    );
    let actual_checksum = crc32_ieee(&encoded[..checksum_offset]);
    if stored_checksum != actual_checksum {
        return Err(RecoveryDecodeError::ChecksumMismatch);
    }

    Ok(encoded[HEADER_LEN..checksum_offset].to_vec())
}

/// IEEE CRC-32 used only as a development corruption/torn-write detector.
///
/// It is not a MAC and must never be treated as cryptographic authentication.
fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320_u32 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_standard_check_vector() {
        assert_eq!(crc32_ieee(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn encoding_is_deterministic_and_round_trips() {
        let first = encode(b"deterministic recovery state");
        let second = encode(b"deterministic recovery state");

        assert_eq!(first, second);
        assert_eq!(decode(&first).unwrap(), b"deterministic recovery state");
    }

    #[test]
    fn truncation_and_trailing_data_fail_closed() {
        let encoded = encode(b"payload");

        assert!(matches!(
            decode(&encoded[..encoded.len() - 1]),
            Err(RecoveryDecodeError::LengthMismatch)
        ));

        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(matches!(
            decode(&trailing),
            Err(RecoveryDecodeError::LengthMismatch)
        ));
    }

    #[test]
    fn payload_corruption_is_detected() {
        let mut encoded = encode(b"payload");
        encoded[HEADER_LEN + 2] ^= 0x40;

        assert_eq!(
            decode(&encoded).unwrap_err(),
            RecoveryDecodeError::ChecksumMismatch
        );
    }

    #[test]
    fn magic_and_version_are_validated() {
        let mut bad_magic = encode(b"payload");
        bad_magic[0] ^= 1;
        assert_eq!(
            decode(&bad_magic).unwrap_err(),
            RecoveryDecodeError::InvalidMagic
        );

        let mut bad_version = encode(b"payload");
        bad_version[MAGIC.len()..MAGIC.len() + 2].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            decode(&bad_version).unwrap_err(),
            RecoveryDecodeError::UnsupportedVersion { version: 2 }
        );
    }
}
