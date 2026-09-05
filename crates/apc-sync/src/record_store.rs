use apc_core::{commit_durable, DurabilityBackend};
use apc_crypto::{protect, unprotect, ContentKey, ProtectionError};

use crate::{
    decode_durable_sync_record, encode_durable_sync_record, DurableSyncRecord, SyncRecordStore,
    SyncRecoveryError,
};

const RECORD_CONTEXT_DOMAIN: &[u8] = b"A.P.C. durable sync recovery\0v1\0";

/// Error returned by the protected durable synchronization recovery store.
#[derive(Debug)]
pub enum ProtectedSyncStoreError<E> {
    Backend(E),
    Recovery(SyncRecoveryError),
    Protection(ProtectionError),
    EmptyContext,
}

impl<E: core::fmt::Display> core::fmt::Display for ProtectedSyncStoreError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Backend(error) => write!(f, "sync recovery backend error: {error}"),
            Self::Recovery(error) => write!(f, "sync recovery codec error: {error}"),
            Self::Protection(error) => write!(f, "sync recovery protection error: {error}"),
            Self::EmptyContext => write!(f, "sync recovery context must not be empty"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ProtectedSyncStoreError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(error) => Some(error),
            Self::Recovery(error) => Some(error),
            Self::Protection(error) => Some(error),
            Self::EmptyContext => None,
        }
    }
}

/// Protects and durably commits the complete `DurableSyncRecord` through any
/// backend that satisfies the A.P.C. durability protocol for opaque bytes.
///
/// This adapter deliberately owns no semantic merge logic and no transport. The
/// caller-provided context should bind the record to its local continuum/store
/// identity; a domain separator is added internally before AEAD protection.
pub struct ProtectedSyncRecordStore<B> {
    backend: B,
    key: ContentKey,
    context: Vec<u8>,
}

impl<B> ProtectedSyncRecordStore<B> {
    pub fn new(
        backend: B,
        key: ContentKey,
        context: impl Into<Vec<u8>>,
    ) -> Result<Self, ProtectedSyncStoreError<core::convert::Infallible>> {
        let context = context.into();
        if context.is_empty() {
            return Err(ProtectedSyncStoreError::EmptyContext);
        }
        Ok(Self {
            backend,
            key,
            context,
        })
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    fn protection_context(&self) -> Vec<u8> {
        let mut context = Vec::with_capacity(RECORD_CONTEXT_DOMAIN.len() + self.context.len());
        context.extend_from_slice(RECORD_CONTEXT_DOMAIN);
        context.extend_from_slice(&self.context);
        context
    }
}

impl<B> ProtectedSyncRecordStore<B>
where
    B: DurabilityBackend<Vec<u8>>,
{
    /// Recover the last complete durably committed sync record.
    ///
    /// Authentication and strict recovery decoding happen before a record is
    /// returned. A wrong key/context or malformed committed bytes fail closed.
    pub fn load_committed(
        &self,
    ) -> Result<Option<DurableSyncRecord>, ProtectedSyncStoreError<B::Error>> {
        let Some(protected) = self
            .backend
            .load_committed()
            .map_err(ProtectedSyncStoreError::Backend)?
        else {
            return Ok(None);
        };

        let clear = unprotect(&self.key, &self.protection_context(), &protected)
            .map_err(ProtectedSyncStoreError::Protection)?;
        let record =
            decode_durable_sync_record(&clear).map_err(ProtectedSyncStoreError::Recovery)?;
        Ok(Some(record))
    }
}

impl<B> SyncRecordStore for ProtectedSyncRecordStore<B>
where
    B: DurabilityBackend<Vec<u8>>,
{
    type Error = ProtectedSyncStoreError<B::Error>;

    fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
        let clear =
            encode_durable_sync_record(record).map_err(ProtectedSyncStoreError::Recovery)?;
        let protected = protect(&self.key, &self.protection_context(), &clear)
            .map_err(ProtectedSyncStoreError::Protection)?;
        commit_durable(&mut self.backend, &protected).map_err(ProtectedSyncStoreError::Backend)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::convert::Infallible;

    use super::*;
    use crate::{PublicationId, TransportCursor};

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Handle(u64);

    #[derive(Default)]
    struct MemoryBackend {
        next: u64,
        candidates: BTreeMap<Handle, Vec<u8>>,
        root: Option<Handle>,
    }

    impl DurabilityBackend<Vec<u8>> for MemoryBackend {
        type Candidate = Handle;
        type Error = Infallible;

        fn load_committed(&self) -> Result<Option<Vec<u8>>, Self::Error> {
            Ok(self
                .root
                .and_then(|root| self.candidates.get(&root).cloned()))
        }

        fn write_candidate(&mut self, state: &Vec<u8>) -> Result<Self::Candidate, Self::Error> {
            let candidate = Handle(self.next);
            self.next += 1;
            self.candidates.insert(candidate, state.clone());
            Ok(candidate)
        }

        fn sync_candidate(&mut self, _candidate: &Self::Candidate) -> Result<(), Self::Error> {
            Ok(())
        }

        fn publish_candidate(&mut self, candidate: &Self::Candidate) -> Result<(), Self::Error> {
            self.root = Some(*candidate);
            Ok(())
        }

        fn sync_committed_root(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn cursor(value: &str) -> TransportCursor {
        TransportCursor::new(value.as_bytes().to_vec()).unwrap()
    }

    fn pid(value: u64) -> PublicationId {
        let mut bytes = [0_u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        PublicationId::from_bytes(bytes)
    }

    #[test]
    fn protected_store_round_trips_complete_state_cursor_and_outbox() {
        let key = ContentKey::from_bytes([0x51; 32]);
        let mut store =
            ProtectedSyncRecordStore::new(MemoryBackend::default(), key, b"continuum-a".to_vec())
                .unwrap();

        let mut record = DurableSyncRecord::new(b"trusted".to_vec(), Some(cursor("R0")));
        record
            .prepare_outbox(
                b"trusted-exposed".to_vec(),
                pid(3),
                Some(cursor("R0")),
                vec![b"protected-wire".to_vec()],
            )
            .unwrap();

        store.persist(&record).unwrap();
        assert_eq!(store.load_committed().unwrap(), Some(record));
    }

    #[test]
    fn wrong_context_cannot_open_committed_sync_record() {
        let key = ContentKey::from_bytes([0x52; 32]);
        let mut store =
            ProtectedSyncRecordStore::new(MemoryBackend::default(), key, b"continuum-a".to_vec())
                .unwrap();
        let record = DurableSyncRecord::new(b"trusted".to_vec(), Some(cursor("R0")));
        store.persist(&record).unwrap();

        let backend = store.into_backend();
        let wrong = ProtectedSyncRecordStore::new(
            backend,
            ContentKey::from_bytes([0x52; 32]),
            b"continuum-b".to_vec(),
        )
        .unwrap();
        assert!(matches!(
            wrong.load_committed(),
            Err(ProtectedSyncStoreError::Protection(
                ProtectionError::AuthenticationFailed
            ))
        ));
    }

    #[test]
    fn committed_backend_bytes_do_not_contain_trusted_plaintext() {
        let key = ContentKey::from_bytes([0x53; 32]);
        let mut store =
            ProtectedSyncRecordStore::new(MemoryBackend::default(), key, b"continuum-a".to_vec())
                .unwrap();
        let secret = b"trusted-sync-secret";
        let record = DurableSyncRecord::new(secret.to_vec(), Some(cursor("R0")));
        store.persist(&record).unwrap();

        let protected = store.backend().load_committed().unwrap().unwrap();
        assert!(!protected
            .windows(secret.len())
            .any(|window| window == secret));
    }
}
