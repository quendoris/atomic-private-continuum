#![forbid(unsafe_code)]

//! Authenticated symmetric protection for A.P.C. portable/development units.
//!
//! This crate deliberately does not implement passwords, platform key wrapping,
//! replica signatures, transport authentication, replay protection or merge
//! ordering. It protects one already-constructed plaintext unit under an
//! explicit caller context.

use core::fmt;

use chacha20poly1305::{
    aead::{Aead, Generate, Key, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"APCPROT1";
const VERSION: u16 = 1;
const ALGORITHM_XCHACHA20_POLY1305: u8 = 1;
const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;
const HEADER_BYTES: usize = MAGIC.len() + 2 + 1 + NONCE_BYTES + 8;
const AAD_DOMAIN: &[u8] = b"A.P.C. authenticated protection\0v1\0xchacha20poly1305\0";

/// Owned 256-bit content-protection key.
///
/// The bytes are zeroized when this value is dropped. `Debug` never exposes key
/// material. This type is not a replica identity, signing key, password hash or
/// platform keystore handle.
pub struct ContentKey(Zeroizing<[u8; KEY_BYTES]>);

impl ContentKey {
    pub fn generate() -> Result<Self, ProtectionError> {
        let generated = Key::<XChaCha20Poly1305>::try_generate()
            .map_err(|_| ProtectionError::RandomnessUnavailable)?;
        Ok(Self(Zeroizing::new(generated.0)))
    }

    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Explicit key export boundary for future wrapping/persistence layers.
    ///
    /// Callers must not serialize these bytes into ordinary A.P.C. content or
    /// diagnostics.
    pub fn expose_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

impl fmt::Debug for ContentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContentKey([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectionError {
    EmptyContext,
    RandomnessUnavailable,
    InvalidEnvelope,
    UnsupportedVersion { version: u16 },
    UnsupportedAlgorithm { algorithm: u8 },
    AuthenticationFailed,
    LengthOverflow,
}

impl fmt::Display for ProtectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyContext => write!(f, "authenticated protection context must not be empty"),
            Self::RandomnessUnavailable => write!(f, "cryptographic randomness is unavailable"),
            Self::InvalidEnvelope => write!(f, "invalid protected envelope"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported protected envelope version {version}")
            }
            Self::UnsupportedAlgorithm { algorithm } => {
                write!(f, "unsupported protected envelope algorithm {algorithm}")
            }
            Self::AuthenticationFailed => write!(f, "protected envelope authentication failed"),
            Self::LengthOverflow => write!(f, "protected envelope length overflows host limits"),
        }
    }
}

impl std::error::Error for ProtectionError {}

/// Protect plaintext with XChaCha20-Poly1305 using a fresh OS-generated nonce.
///
/// `context` is non-secret associated data and must be a canonical higher-level
/// binding for the unit being protected. The same protected bytes must fail to
/// authenticate under a different context.
pub fn protect(
    key: &ContentKey,
    context: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, ProtectionError> {
    if context.is_empty() {
        return Err(ProtectionError::EmptyContext);
    }

    let nonce = XNonce::try_generate().map_err(|_| ProtectionError::RandomnessUnavailable)?;
    protect_with_nonce(key, context, plaintext, nonce.0)
}

/// Authenticate and decrypt one complete protected envelope.
///
/// Authentication failure never returns partial plaintext.
pub fn unprotect(
    key: &ContentKey,
    context: &[u8],
    envelope: &[u8],
) -> Result<Vec<u8>, ProtectionError> {
    if context.is_empty() {
        return Err(ProtectionError::EmptyContext);
    }
    if envelope.len() < HEADER_BYTES + TAG_BYTES {
        return Err(ProtectionError::InvalidEnvelope);
    }
    if &envelope[..MAGIC.len()] != MAGIC {
        return Err(ProtectionError::InvalidEnvelope);
    }

    let version_offset = MAGIC.len();
    let version = u16::from_be_bytes([
        envelope[version_offset],
        envelope[version_offset + 1],
    ]);
    if version != VERSION {
        return Err(ProtectionError::UnsupportedVersion { version });
    }

    let algorithm_offset = version_offset + 2;
    let algorithm = envelope[algorithm_offset];
    if algorithm != ALGORITHM_XCHACHA20_POLY1305 {
        return Err(ProtectionError::UnsupportedAlgorithm { algorithm });
    }

    let nonce_offset = algorithm_offset + 1;
    let nonce_end = nonce_offset + NONCE_BYTES;
    let nonce_bytes: [u8; NONCE_BYTES] = envelope[nonce_offset..nonce_end]
        .try_into()
        .expect("fixed-length nonce slice");

    let length_offset = nonce_end;
    let ciphertext_len_u64 = u64::from_be_bytes(
        envelope[length_offset..length_offset + 8]
            .try_into()
            .expect("fixed-length ciphertext length"),
    );
    let ciphertext_len = usize::try_from(ciphertext_len_u64)
        .map_err(|_| ProtectionError::LengthOverflow)?;
    if ciphertext_len < TAG_BYTES {
        return Err(ProtectionError::InvalidEnvelope);
    }

    let expected_len = HEADER_BYTES
        .checked_add(ciphertext_len)
        .ok_or(ProtectionError::LengthOverflow)?;
    if envelope.len() != expected_len {
        return Err(ProtectionError::InvalidEnvelope);
    }

    let cipher = XChaCha20Poly1305::new_from_slice(key.expose_bytes())
        .expect("ContentKey always has the required length");
    let nonce = XNonce::from(nonce_bytes);
    let aad = build_aad(context);

    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &envelope[HEADER_BYTES..],
                aad: &aad,
            },
        )
        .map_err(|_| ProtectionError::AuthenticationFailed)
}

fn protect_with_nonce(
    key: &ContentKey,
    context: &[u8],
    plaintext: &[u8],
    nonce_bytes: [u8; NONCE_BYTES],
) -> Result<Vec<u8>, ProtectionError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key.expose_bytes())
        .expect("ContentKey always has the required length");
    let nonce = XNonce::from(nonce_bytes);
    let aad = build_aad(context);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| ProtectionError::LengthOverflow)?;

    let ciphertext_len =
        u64::try_from(ciphertext.len()).map_err(|_| ProtectionError::LengthOverflow)?;
    let mut envelope = Vec::with_capacity(HEADER_BYTES + ciphertext.len());
    envelope.extend_from_slice(MAGIC);
    envelope.extend_from_slice(&VERSION.to_be_bytes());
    envelope.push(ALGORITHM_XCHACHA20_POLY1305);
    envelope.extend_from_slice(&nonce_bytes);
    envelope.extend_from_slice(&ciphertext_len.to_be_bytes());
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

fn build_aad(context: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(AAD_DOMAIN.len() + context.len());
    aad.extend_from_slice(AAD_DOMAIN);
    aad.extend_from_slice(context);
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; KEY_BYTES] = [0x11; KEY_BYTES];
    const KEY_B: [u8; KEY_BYTES] = [0x22; KEY_BYTES];
    const CONTEXT: &[u8] = b"apc-test/local-recovery/continuum-a";

    fn key_a() -> ContentKey {
        ContentKey::from_bytes(KEY_A)
    }

    #[test]
    fn round_trip_supports_empty_and_nonempty_plaintext() {
        for plaintext in [b"".as_slice(), b"hello protected A.P.C.".as_slice()] {
            let key = key_a();
            let envelope = protect(&key, CONTEXT, plaintext).unwrap();
            assert_ne!(envelope, plaintext);
            assert_eq!(unprotect(&key, CONTEXT, &envelope).unwrap(), plaintext);
        }
    }

    #[test]
    fn same_input_uses_independent_random_nonces() {
        let key = key_a();
        let first = protect(&key, CONTEXT, b"same plaintext").unwrap();
        let second = protect(&key, CONTEXT, b"same plaintext").unwrap();

        assert_ne!(first, second);
        let nonce_offset = MAGIC.len() + 2 + 1;
        assert_ne!(
            &first[nonce_offset..nonce_offset + NONCE_BYTES],
            &second[nonce_offset..nonce_offset + NONCE_BYTES]
        );
    }

    #[test]
    fn wrong_key_and_wrong_context_fail_authentication() {
        let key = key_a();
        let envelope = protect(&key, CONTEXT, b"secret").unwrap();

        assert_eq!(
            unprotect(&ContentKey::from_bytes(KEY_B), CONTEXT, &envelope).unwrap_err(),
            ProtectionError::AuthenticationFailed
        );
        assert_eq!(
            unprotect(&key, b"apc-test/other-context", &envelope).unwrap_err(),
            ProtectionError::AuthenticationFailed
        );
    }

    #[test]
    fn nonce_ciphertext_and_tag_tampering_fail_authentication() {
        let key = key_a();
        let envelope = protect(&key, CONTEXT, b"tamper target").unwrap();
        let nonce_offset = MAGIC.len() + 2 + 1;

        let mut nonce_tampered = envelope.clone();
        nonce_tampered[nonce_offset] ^= 0x01;
        assert_eq!(
            unprotect(&key, CONTEXT, &nonce_tampered).unwrap_err(),
            ProtectionError::AuthenticationFailed
        );

        let mut ciphertext_tampered = envelope.clone();
        ciphertext_tampered[HEADER_BYTES] ^= 0x80;
        assert_eq!(
            unprotect(&key, CONTEXT, &ciphertext_tampered).unwrap_err(),
            ProtectionError::AuthenticationFailed
        );

        let mut tag_tampered = envelope;
        let last = tag_tampered.len() - 1;
        tag_tampered[last] ^= 0x40;
        assert_eq!(
            unprotect(&key, CONTEXT, &tag_tampered).unwrap_err(),
            ProtectionError::AuthenticationFailed
        );
    }

    #[test]
    fn malformed_header_truncation_and_trailing_bytes_fail_closed() {
        let key = key_a();
        let envelope = protect(&key, CONTEXT, b"payload").unwrap();

        let mut bad_magic = envelope.clone();
        bad_magic[0] ^= 1;
        assert_eq!(
            unprotect(&key, CONTEXT, &bad_magic).unwrap_err(),
            ProtectionError::InvalidEnvelope
        );

        let mut bad_version = envelope.clone();
        bad_version[MAGIC.len()..MAGIC.len() + 2].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            unprotect(&key, CONTEXT, &bad_version).unwrap_err(),
            ProtectionError::UnsupportedVersion { version: 2 }
        );

        let algorithm_offset = MAGIC.len() + 2;
        let mut bad_algorithm = envelope.clone();
        bad_algorithm[algorithm_offset] = 99;
        assert_eq!(
            unprotect(&key, CONTEXT, &bad_algorithm).unwrap_err(),
            ProtectionError::UnsupportedAlgorithm { algorithm: 99 }
        );

        assert_eq!(
            unprotect(&key, CONTEXT, &envelope[..envelope.len() - 1]).unwrap_err(),
            ProtectionError::InvalidEnvelope
        );

        let mut trailing = envelope;
        trailing.push(0);
        assert_eq!(
            unprotect(&key, CONTEXT, &trailing).unwrap_err(),
            ProtectionError::InvalidEnvelope
        );
    }

    #[test]
    fn context_is_mandatory() {
        let key = key_a();
        assert_eq!(
            protect(&key, b"", b"plaintext").unwrap_err(),
            ProtectionError::EmptyContext
        );
        assert_eq!(
            unprotect(&key, b"", b"not-an-envelope").unwrap_err(),
            ProtectionError::EmptyContext
        );
    }

    #[test]
    fn key_debug_output_is_redacted() {
        let key = key_a();
        let rendered = format!("{key:?}");
        assert_eq!(rendered, "ContentKey([REDACTED])");
        assert!(!rendered.contains("11"));
    }

    #[test]
    fn fixed_nonce_path_is_deterministic_for_future_interop_vectors() {
        let key = key_a();
        let nonce = [0x33; NONCE_BYTES];
        let first = protect_with_nonce(&key, CONTEXT, b"interop", nonce).unwrap();
        let second = protect_with_nonce(&key, CONTEXT, b"interop", nonce).unwrap();
        assert_eq!(first, second);
        assert_eq!(unprotect(&key, CONTEXT, &first).unwrap(), b"interop");
    }
}
