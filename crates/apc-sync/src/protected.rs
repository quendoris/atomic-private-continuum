use std::collections::BTreeMap;

use apc_core::{ContinuumId, CoreError};
use apc_crypto::{protect, unprotect, ContentKey, ProtectionError};

use crate::{
    decode_scalar_projection, encode_scalar_projection, ScalarSyncProjection, SyncCodecError,
};

const PUBLICATION_ID_BYTES: usize = 32;
const SYNC_PART_CONTEXT_DOMAIN: &[u8] = b"A.P.C. sync part context\0v1\0";

/// Opaque multipart publication identity used only for assembly/integrity binding.
///
/// Byte order has no temporal, causal or merge meaning.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicationId([u8; PUBLICATION_ID_BYTES]);

impl PublicationId {
    pub const fn from_bytes(bytes: [u8; PUBLICATION_ID_BYTES]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; PUBLICATION_ID_BYTES] {
        &self.0
    }
}

impl core::fmt::Debug for PublicationId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PublicationId(")?;
        for byte in self.0.iter().take(4) {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "…)")
    }
}

/// Transport-facing opaque part.
///
/// Only multipart bookkeeping is clear. The payload is an authenticated encrypted
/// pre-format scalar projection. The clear bookkeeping is bound into AEAD AAD so
/// it cannot be substituted without authentication failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedSyncPart {
    pub publication_id: PublicationId,
    pub part_index: u32,
    pub total_parts: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub enum SyncPartError {
    InvalidMultipartMetadata,
    MultipartTotalMismatch,
    MultipartPartCollision,
    EmptyProjection,
    Codec(SyncCodecError),
    Protection(ProtectionError),
    Core(CoreError),
}

impl core::fmt::Display for SyncPartError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMultipartMetadata => write!(f, "invalid sync multipart metadata"),
            Self::MultipartTotalMismatch => write!(f, "sync multipart total changed in flight"),
            Self::MultipartPartCollision => {
                write!(f, "sync multipart index contains conflicting authenticated state")
            }
            Self::EmptyProjection => write!(f, "cannot protect an empty sync projection part"),
            Self::Codec(error) => write!(f, "sync projection codec error: {error}"),
            Self::Protection(error) => write!(f, "sync protection error: {error}"),
            Self::Core(error) => write!(f, "sync merge error: {error}"),
        }
    }
}

impl std::error::Error for SyncPartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(error) => Some(error),
            Self::Protection(error) => Some(error),
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

impl From<SyncCodecError> for SyncPartError {
    fn from(value: SyncCodecError) -> Self {
        Self::Codec(value)
    }
}

impl From<ProtectionError> for SyncPartError {
    fn from(value: ProtectionError) -> Self {
        Self::Protection(value)
    }
}

impl From<CoreError> for SyncPartError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

pub fn protect_scalar_part(
    key: &ContentKey,
    continuum_id: ContinuumId,
    publication_id: PublicationId,
    part_index: u32,
    total_parts: u32,
    projection: &ScalarSyncProjection,
) -> Result<ProtectedSyncPart, SyncPartError> {
    validate_part_metadata(part_index, total_parts)?;
    if projection.is_empty() {
        return Err(SyncPartError::EmptyProjection);
    }

    let clear = encode_scalar_projection(projection)?;
    let context = part_context(continuum_id, publication_id, part_index, total_parts);
    let payload = protect(key, &context, &clear)?;
    Ok(ProtectedSyncPart {
        publication_id,
        part_index,
        total_parts,
        payload,
    })
}

pub fn unprotect_scalar_part(
    key: &ContentKey,
    continuum_id: ContinuumId,
    part: &ProtectedSyncPart,
) -> Result<ScalarSyncProjection, SyncPartError> {
    validate_part_metadata(part.part_index, part.total_parts)?;
    let context = part_context(
        continuum_id,
        part.publication_id,
        part.part_index,
        part.total_parts,
    );
    let clear = unprotect(key, &context, &part.payload)?;
    let projection = decode_scalar_projection(&clear)?;
    if projection.is_empty() {
        return Err(SyncPartError::EmptyProjection);
    }
    Ok(projection)
}

fn validate_part_metadata(part_index: u32, total_parts: u32) -> Result<(), SyncPartError> {
    if total_parts == 0 || part_index >= total_parts {
        return Err(SyncPartError::InvalidMultipartMetadata);
    }
    Ok(())
}

fn part_context(
    continuum_id: ContinuumId,
    publication_id: PublicationId,
    part_index: u32,
    total_parts: u32,
) -> Vec<u8> {
    let mut context = Vec::with_capacity(
        SYNC_PART_CONTEXT_DOMAIN.len() + 32 + PUBLICATION_ID_BYTES + 4 + 4,
    );
    context.extend_from_slice(SYNC_PART_CONTEXT_DOMAIN);
    context.extend_from_slice(continuum_id.as_bytes());
    context.extend_from_slice(publication_id.as_bytes());
    context.extend_from_slice(&part_index.to_be_bytes());
    context.extend_from_slice(&total_parts.to_be_bytes());
    context
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingPublication {
    total_parts: u32,
    parts: BTreeMap<u32, ScalarSyncProjection>,
}

/// Authenticates multipart parts and exposes semantic state only when complete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MultipartInbox {
    pending: BTreeMap<PublicationId, PendingPublication>,
}

impl MultipartInbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending_publications(&self) -> usize {
        self.pending.len()
    }

    pub fn ingest(
        &mut self,
        key: &ContentKey,
        continuum_id: ContinuumId,
        part: ProtectedSyncPart,
    ) -> Result<Option<ScalarSyncProjection>, SyncPartError> {
        let clear = unprotect_scalar_part(key, continuum_id, &part)?;
        let publication_id = part.publication_id;

        let pending = self
            .pending
            .entry(publication_id)
            .or_insert_with(|| PendingPublication {
                total_parts: part.total_parts,
                parts: BTreeMap::new(),
            });

        if pending.total_parts != part.total_parts {
            return Err(SyncPartError::MultipartTotalMismatch);
        }

        if let Some(previous) = pending.parts.get(&part.part_index) {
            if previous != &clear {
                return Err(SyncPartError::MultipartPartCollision);
            }
        } else {
            pending.parts.insert(part.part_index, clear);
        }

        if pending.parts.len() != usize::try_from(pending.total_parts).unwrap_or(usize::MAX) {
            return Ok(None);
        }

        let complete = self
            .pending
            .remove(&publication_id)
            .expect("completed publication must still exist");
        let mut parts = complete.parts.into_values();
        let mut projection = parts
            .next()
            .expect("valid multipart publication contains at least one part");
        for fragment in parts {
            projection = projection.merge(&fragment)?;
        }
        Ok(Some(projection))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use apc_core::{AtomId, RevisionId, ScalarRegister};

    use super::*;
    use crate::{DomainKey, SyncProjection};

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

    fn projection(atom_value: u64, revision: u64, secret: &str) -> ScalarSyncProjection {
        let key = DomainKey::new(atom(atom_value), b"body".to_vec()).unwrap();
        let mut register = ScalarRegister::new();
        register
            .assign(rid(revision), secret.as_bytes().to_vec())
            .unwrap();
        SyncProjection::from_domains(BTreeMap::from([(key, register)]))
    }

    #[test]
    fn clear_multipart_metadata_is_authenticated_as_context() {
        let key = ContentKey::from_bytes([0x41; 32]);
        let part = protect_scalar_part(&key, cid(1), pid(5), 0, 2, &projection(1, 10, "secret"))
            .unwrap();

        let mut changed_index = part.clone();
        changed_index.part_index = 1;
        assert!(matches!(
            unprotect_scalar_part(&key, cid(1), &changed_index),
            Err(SyncPartError::Protection(ProtectionError::AuthenticationFailed))
        ));

        let mut changed_total = part.clone();
        changed_total.total_parts = 3;
        assert!(matches!(
            unprotect_scalar_part(&key, cid(1), &changed_total),
            Err(SyncPartError::Protection(ProtectionError::AuthenticationFailed))
        ));

        let mut changed_publication = part;
        changed_publication.publication_id = pid(6);
        assert!(matches!(
            unprotect_scalar_part(&key, cid(1), &changed_publication),
            Err(SyncPartError::Protection(ProtectionError::AuthenticationFailed))
        ));
    }

    #[test]
    fn continuum_binding_prevents_cross_continuum_reuse() {
        let key = ContentKey::from_bytes([0x42; 32]);
        let part = protect_scalar_part(&key, cid(1), pid(5), 0, 1, &projection(1, 10, "secret"))
            .unwrap();

        assert!(matches!(
            unprotect_scalar_part(&key, cid(2), &part),
            Err(SyncPartError::Protection(ProtectionError::AuthenticationFailed))
        ));
    }

    #[test]
    fn multipart_inbox_is_invisible_until_complete_and_accepts_duplicates() {
        let key = ContentKey::from_bytes([0x43; 32]);
        let first = protect_scalar_part(&key, cid(1), pid(7), 0, 2, &projection(1, 10, "one"))
            .unwrap();
        let second = protect_scalar_part(&key, cid(1), pid(7), 1, 2, &projection(2, 20, "two"))
            .unwrap();

        let mut inbox = MultipartInbox::new();
        assert!(inbox.ingest(&key, cid(1), second.clone()).unwrap().is_none());
        assert!(inbox.ingest(&key, cid(1), second).unwrap().is_none());

        let complete = inbox.ingest(&key, cid(1), first).unwrap().unwrap();
        assert_eq!(complete.len(), 2);
        assert_eq!(inbox.pending_publications(), 0);
    }

    #[test]
    fn protected_payload_does_not_contain_clear_domain_value() {
        let key = ContentKey::from_bytes([0x44; 32]);
        let secret = b"transport-must-not-see-this";
        let part = protect_scalar_part(
            &key,
            cid(1),
            pid(8),
            0,
            1,
            &projection(1, 10, core::str::from_utf8(secret).unwrap()),
        )
        .unwrap();

        assert!(!part
            .payload
            .windows(secret.len())
            .any(|window| window == secret));
    }
}
