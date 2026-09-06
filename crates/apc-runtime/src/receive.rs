use apc_core::{
    ContinuumId, CoreError, LocalScalarDomain, LocalScalarSnapshot, RevisionId, ScalarRegister,
    ScalarRevision,
};
use apc_crypto::ContentKey;
use apc_sync::{
    commit_received, decode_protected_sync_part, unprotect_scalar_part, DomainKey, DurableSyncRecord,
    ProtectedPartCodecError, SessionCommitError, SyncPartError, SyncRecordStore,
    TransportCursorCodec,
};

use crate::publication::TrustedStateCodec;

#[derive(Debug)]
pub enum ScalarObjectDecodeError {
    Wire(ProtectedPartCodecError),
    Protection(SyncPartError),
    MultipartUnsupported,
    UnexpectedDomainCount { count: usize },
    MissingExpectedDomain,
}

impl core::fmt::Display for ScalarObjectDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Wire(error) => write!(f, "protected scalar wire error: {error}"),
            Self::Protection(error) => write!(f, "protected scalar authentication error: {error}"),
            Self::MultipartUnsupported => {
                write!(f, "single-object scalar receive path cannot accept multipart state")
            }
            Self::UnexpectedDomainCount { count } => {
                write!(f, "single-domain scalar object contains {count} domains")
            }
            Self::MissingExpectedDomain => {
                write!(f, "protected scalar object does not contain the expected domain")
            }
        }
    }
}

impl std::error::Error for ScalarObjectDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Wire(error) => Some(error),
            Self::Protection(error) => Some(error),
            Self::MultipartUnsupported
            | Self::UnexpectedDomainCount { .. }
            | Self::MissingExpectedDomain => None,
        }
    }
}

impl From<ProtectedPartCodecError> for ScalarObjectDecodeError {
    fn from(value: ProtectedPartCodecError) -> Self {
        Self::Wire(value)
    }
}

impl From<SyncPartError> for ScalarObjectDecodeError {
    fn from(value: SyncPartError) -> Self {
        Self::Protection(value)
    }
}

#[derive(Debug)]
pub enum ScalarReceiveCommitError<TrustedError, StoreError, CursorError> {
    Core(CoreError),
    TrustedState(TrustedError),
    Sync(SessionCommitError<StoreError, CursorError>),
}

/// Decode and authenticate one complete single-domain scalar sync object.
///
/// This is the inverse of the current one-part `prepare_scalar_handoff()` path.
/// Multipart assembly remains owned by `MultipartInbox`; this helper deliberately
/// refuses to make an incomplete or multi-domain object observable by accident.
pub fn decode_single_scalar_domain_object(
    key: &ContentKey,
    continuum_id: ContinuumId,
    expected_domain: &DomainKey,
    encoded: &[u8],
) -> Result<ScalarRegister<Vec<u8>>, ScalarObjectDecodeError> {
    let part = decode_protected_sync_part(encoded)?;
    if part.part_index != 0 || part.total_parts != 1 {
        return Err(ScalarObjectDecodeError::MultipartUnsupported);
    }

    let projection = unprotect_scalar_part(key, continuum_id, &part)?;
    if projection.len() != 1 {
        return Err(ScalarObjectDecodeError::UnexpectedDomainCount {
            count: projection.len(),
        });
    }

    projection
        .get(expected_domain)
        .cloned()
        .ok_or(ScalarObjectDecodeError::MissingExpectedDomain)
}

/// Make authenticated remote scalar state semantically observable and advance
/// the durable transport cursor as one crash-safe runtime transition.
///
/// If local work is dirty, `pre_observation_revision_id` seals it on a cloned
/// candidate before the remote state is merged. The resulting local revision is
/// therefore based on the frontier that the working epoch actually observed.
/// The candidate trusted state and new cursor are then persisted together through
/// `commit_received()`. Only after that durability barrier succeeds is the live
/// semantic domain replaced.
pub fn commit_received_scalar_domain<T, S, TC, CC, R>(
    domain: &mut LocalScalarDomain<T>,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    remote: &ScalarRegister<T>,
    pre_observation_revision_id: Option<RevisionId>,
    new_head: &R,
) -> Result<Option<ScalarRevision<T>>, ScalarReceiveCommitError<TC::Error, S::Error, CC::Error>>
where
    T: Clone + Eq,
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarSnapshot<T>>,
    CC: TransportCursorCodec<R>,
{
    let mut candidate = domain.clone();
    let sealed = candidate
        .observe_remote(remote, pre_observation_revision_id)
        .map_err(ScalarReceiveCommitError::Core)?;
    let merged_trusted_state = trusted_codec
        .encode(&candidate.snapshot())
        .map_err(ScalarReceiveCommitError::TrustedState)?;

    commit_received(
        record,
        store,
        cursor_codec,
        merged_trusted_state,
        new_head,
    )
    .map_err(ScalarReceiveCommitError::Sync)?;

    *domain = candidate;
    Ok(sealed)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, WorkingEpochId};
    use apc_sync::{PublicationId, TransportCursor};

    use crate::{
        prepare_scalar_handoff, DevelopmentScalarTrustedStateCodec, TrustedStateCodec,
    };

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Revision(u64);

    struct RevisionCodec;

    impl TransportCursorCodec<Revision> for RevisionCodec {
        type Error = &'static str;

        fn encode(&self, revision: &Revision) -> Result<TransportCursor, Self::Error> {
            TransportCursor::new(revision.0.to_be_bytes().to_vec()).map_err(|_| "encode")
        }

        fn decode(&self, cursor: &TransportCursor) -> Result<Revision, Self::Error> {
            let bytes: [u8; 8] = cursor.as_bytes().try_into().map_err(|_| "decode")?;
            Ok(Revision(u64::from_be_bytes(bytes)))
        }
    }

    #[derive(Default)]
    struct MemoryStore {
        committed: Option<DurableSyncRecord>,
        fail: bool,
    }

    impl SyncRecordStore for MemoryStore {
        type Error = &'static str;

        fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
            if self.fail {
                return Err("durability failure");
            }
            self.committed = Some(record.clone());
            Ok(())
        }
    }

    fn bytes(value: u64) -> [u8; LOGICAL_ID_BYTES] {
        let mut bytes = [0_u8; LOGICAL_ID_BYTES];
        bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(bytes(value))
    }

    fn wid(value: u64) -> WorkingEpochId {
        WorkingEpochId::from_bytes(bytes(value))
    }

    fn pid(value: u64) -> PublicationId {
        PublicationId::from_bytes(bytes(value))
    }

    fn cid(value: u64) -> ContinuumId {
        ContinuumId::from_bytes(bytes(value))
    }

    fn atom(value: u64) -> AtomId {
        AtomId::from_bytes(bytes(value))
    }

    fn domain_key() -> DomainKey {
        DomainKey::new(atom(1), b"body".to_vec()).unwrap()
    }

    fn base_register() -> ScalarRegister<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), b"base".to_vec()).unwrap();
        causal
    }

    fn sender() -> LocalScalarDomain<Vec<u8>> {
        let mut domain = LocalScalarDomain::from_causal(base_register()).unwrap();
        domain.begin_epoch(wid(9), b"remote".to_vec()).unwrap();
        domain.seal_local(rid(900)).unwrap();
        domain.finalize(rid(900)).unwrap();
        domain
    }

    fn dirty_receiver() -> LocalScalarDomain<Vec<u8>> {
        let mut domain = LocalScalarDomain::from_causal(base_register()).unwrap();
        domain.begin_epoch(wid(1), b"local-draft".to_vec()).unwrap();
        domain
    }

    #[test]
    fn protected_object_decodes_to_exact_expected_scalar_domain() {
        let key = ContentKey::from_bytes([0x81; 32]);
        let semantic_key = domain_key();
        let sender = sender();
        let prepared = prepare_scalar_handoff(
            &sender,
            semantic_key.clone(),
            cid(1),
            pid(1),
            &key,
            [rid(900)],
        )
        .unwrap();

        let decoded = decode_single_scalar_domain_object(
            &key,
            cid(1),
            &semantic_key,
            &prepared.publication().objects()[0],
        )
        .unwrap();

        assert!(decoded.revision(rid(100)).is_some());
        assert!(decoded.revision(rid(900)).is_some());
        assert_eq!(decoded.len(), 2);
    }

    #[test]
    fn remote_observation_and_cursor_advance_commit_atomically() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let mut receiver = dirty_receiver();
        let trusted = trusted_codec.encode(&receiver.snapshot()).unwrap();
        let mut record = DurableSyncRecord::new(
            trusted,
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();

        let remote = sender().causal().clone();
        let sealed = commit_received_scalar_domain(
            &mut receiver,
            &mut record,
            &mut store,
            &trusted_codec,
            &cursor_codec,
            &remote,
            Some(rid(200)),
            &Revision(2),
        )
        .unwrap()
        .unwrap();

        assert_eq!(sealed.parents, BTreeSet::from([rid(100)]));
        assert_eq!(
            receiver.causal().frontier_ids(),
            BTreeSet::from([rid(200), rid(900)])
        );
        assert!(receiver
            .finalization()
            .local_revision_ids()
            .contains(&rid(200)));
        assert!(receiver.pending().is_none());
        assert_eq!(
            cursor_codec.decode(record.applied_cursor().unwrap()).unwrap(),
            Revision(2)
        );
        assert_eq!(store.committed.as_ref(), Some(&record));

        let recovered = trusted_codec.decode(record.trusted_state()).unwrap();
        assert_eq!(LocalScalarDomain::restore(recovered).unwrap(), receiver);
    }

    #[test]
    fn failed_receive_persistence_leaves_dirty_domain_and_old_cursor_unchanged() {
        let trusted_codec = DevelopmentScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let mut receiver = dirty_receiver();
        let trusted = trusted_codec.encode(&receiver.snapshot()).unwrap();
        let mut record = DurableSyncRecord::new(
            trusted,
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let before_domain = receiver.clone();
        let before_record = record.clone();
        let mut store = MemoryStore {
            fail: true,
            ..MemoryStore::default()
        };

        assert!(commit_received_scalar_domain(
            &mut receiver,
            &mut record,
            &mut store,
            &trusted_codec,
            &cursor_codec,
            &sender().causal().clone(),
            Some(rid(200)),
            &Revision(2),
        )
        .is_err());

        assert_eq!(receiver, before_domain);
        assert_eq!(record, before_record);
        assert!(receiver.pending().is_some());
        assert!(receiver.causal().revision(rid(200)).is_none());
    }
}
