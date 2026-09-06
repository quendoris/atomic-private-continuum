use apc_core::{CoreError, LocalScalarDomain, RevisionId, ScalarRegister, ScalarRevision};
use apc_sync::{
    commit_received, DomainKey, DurableSyncRecord, SessionCommitError, SyncRecordStore,
    TransportCursorCodec,
};

use crate::{LocalScalarRecoveryState, LocalScalarRecoveryStateError, TrustedStateCodec};

/// Authenticated remote state for exactly one merge domain inside a complete local
/// recovery container.
///
/// The selected `DomainKey` is semantic routing only. The transport head remains
/// opaque bookkeeping, and the optional local revision identity is consumed only
/// by this selected domain if dirty work must be sealed before observation.
pub struct ReceivedRecoveryScalarState<'a, R> {
    domain_key: &'a DomainKey,
    remote: &'a ScalarRegister<Vec<u8>>,
    pre_observation_revision_id: Option<RevisionId>,
    new_head: &'a R,
}

impl<'a, R> ReceivedRecoveryScalarState<'a, R> {
    pub fn new(
        domain_key: &'a DomainKey,
        remote: &'a ScalarRegister<Vec<u8>>,
        pre_observation_revision_id: Option<RevisionId>,
        new_head: &'a R,
    ) -> Self {
        Self {
            domain_key,
            remote,
            pre_observation_revision_id,
            new_head,
        }
    }

    pub fn domain_key(&self) -> &DomainKey {
        self.domain_key
    }

    pub fn remote(&self) -> &ScalarRegister<Vec<u8>> {
        self.remote
    }

    pub fn pre_observation_revision_id(&self) -> Option<RevisionId> {
        self.pre_observation_revision_id
    }

    pub fn new_head(&self) -> &R {
        self.new_head
    }
}

#[derive(Debug)]
pub enum RecoveryScalarReceiveCommitError<TrustedError, StoreError, CursorError> {
    MissingDomain,
    Core(CoreError),
    RecoveryState(LocalScalarRecoveryStateError),
    TrustedState(TrustedError),
    Sync(SessionCommitError<StoreError, CursorError>),
}

pub type RecoveryScalarReceiveResult<TrustedError, StoreError, CursorError> = Result<
    Option<ScalarRevision<Vec<u8>>>,
    RecoveryScalarReceiveCommitError<TrustedError, StoreError, CursorError>,
>;

/// Observe authenticated remote state in exactly one local scalar merge domain and
/// advance the shared durable transport cursor with the resulting complete local
/// recovery container.
///
/// This function deliberately separates **physical crash atomicity** from
/// **semantic atomicity**:
///
/// - only `received.domain_key` is restored and semantically observed;
/// - a dirty working epoch in another domain is neither sealed nor made to observe
///   the remote state;
/// - the complete recovery container is nevertheless encoded and persisted with
///   the new transport cursor as one physical recovery unit, because a cursor may
///   never outrun any local state required to restart.
///
/// If semantic validation, trusted-state encoding, cursor encoding or persistence
/// fails, both the caller's live recovery container and durable sync record remain
/// unchanged.
pub fn commit_received_recovery_scalar_domain<S, TC, CC, R>(
    recovery: &mut LocalScalarRecoveryState,
    record: &mut DurableSyncRecord,
    store: &mut S,
    trusted_codec: &TC,
    cursor_codec: &CC,
    received: ReceivedRecoveryScalarState<'_, R>,
) -> RecoveryScalarReceiveResult<TC::Error, S::Error, CC::Error>
where
    S: SyncRecordStore,
    TC: TrustedStateCodec<LocalScalarRecoveryState>,
    CC: TransportCursorCodec<R>,
{
    let mut candidate = recovery.clone();
    let mut selected = candidate
        .restore_domain(received.domain_key)
        .map_err(RecoveryScalarReceiveCommitError::Core)?
        .ok_or(RecoveryScalarReceiveCommitError::MissingDomain)?;

    let sealed = selected
        .observe_remote(received.remote, received.pre_observation_revision_id)
        .map_err(RecoveryScalarReceiveCommitError::Core)?;

    candidate
        .replace_domain(received.domain_key.clone(), selected.snapshot())
        .map_err(RecoveryScalarReceiveCommitError::RecoveryState)?;

    let trusted_state = trusted_codec
        .encode(&candidate)
        .map_err(RecoveryScalarReceiveCommitError::TrustedState)?;

    commit_received(
        record,
        store,
        cursor_codec,
        trusted_state,
        received.new_head,
    )
    .map_err(RecoveryScalarReceiveCommitError::Sync)?;

    *recovery = candidate;
    Ok(sealed)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use apc_core::id::LOGICAL_ID_BYTES;
    use apc_core::{AtomId, LocalScalarDomain, ScalarRegister, WorkingEpochId};
    use apc_sync::{SyncRecordStore, TransportCursor};

    use crate::DevelopmentMultiScalarTrustedStateCodec;

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

    fn atom(value: u64) -> AtomId {
        AtomId::from_bytes(bytes(value))
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(bytes(value))
    }

    fn wid(value: u64) -> WorkingEpochId {
        WorkingEpochId::from_bytes(bytes(value))
    }

    fn key(domain: &str) -> DomainKey {
        DomainKey::new(atom(1), domain.as_bytes()).unwrap()
    }

    fn dirty_domain(base_revision: u64, epoch: u64, base: &str, draft: &str) -> LocalScalarDomain<Vec<u8>> {
        let mut causal = ScalarRegister::new();
        causal
            .assign(rid(base_revision), base.as_bytes().to_vec())
            .unwrap();
        let mut domain = LocalScalarDomain::from_causal(causal).unwrap();
        domain
            .begin_epoch(wid(epoch), draft.as_bytes().to_vec())
            .unwrap();
        domain
    }

    fn remote_from_base(base_revision: u64, remote_revision: u64, base: &str, value: &str) -> ScalarRegister<Vec<u8>> {
        let mut remote = ScalarRegister::new();
        remote
            .assign(rid(base_revision), base.as_bytes().to_vec())
            .unwrap();
        remote
            .assign(rid(remote_revision), value.as_bytes().to_vec())
            .unwrap();
        remote
    }

    #[test]
    fn observing_title_seals_only_title_and_preserves_dirty_body() {
        let body_key = key("body");
        let title_key = key("title");
        let body = dirty_domain(100, 1, "body-base", "body-draft");
        let title = dirty_domain(300, 2, "title-base", "title-draft");
        let original_body = body.snapshot();

        let mut recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (body_key.clone(), original_body.clone()),
            (title_key.clone(), title.snapshot()),
        ]))
        .unwrap();

        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let mut store = MemoryStore::default();
        let remote_title = remote_from_base(300, 900, "title-base", "remote-title");

        let sealed = commit_received_recovery_scalar_domain(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            ReceivedRecoveryScalarState::new(&title_key, &remote_title, Some(rid(400)), &Revision(2)),
        )
        .unwrap()
        .unwrap();

        assert_eq!(sealed.id, rid(400));
        assert_eq!(sealed.parents, BTreeSet::from([rid(300)]));

        // Domain A stays byte/logically identical: remote observation in B does
        // not seal A and does not inject either B revision into A's graph.
        assert_eq!(recovery.get(&body_key), Some(&original_body));
        let recovered_body = recovery.restore_domain(&body_key).unwrap().unwrap();
        assert_eq!(recovered_body.pending().unwrap().id, wid(1));
        assert!(recovered_body.causal().revision(rid(400)).is_none());
        assert!(recovered_body.causal().revision(rid(900)).is_none());

        let recovered_title = recovery.restore_domain(&title_key).unwrap().unwrap();
        assert!(recovered_title.pending().is_none());
        assert_eq!(
            recovered_title.causal().frontier_ids(),
            BTreeSet::from([rid(400), rid(900)])
        );
        assert!(!recovered_title.causal().is_ancestor(rid(400), rid(900)));
        assert!(!recovered_title.causal().is_ancestor(rid(900), rid(400)));
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
        assert_eq!(store.committed.as_ref(), Some(&record));
    }

    #[test]
    fn persistence_failure_keeps_every_domain_and_cursor_unchanged() {
        let body_key = key("body");
        let title_key = key("title");
        let mut recovery = LocalScalarRecoveryState::from_domains(BTreeMap::from([
            (
                body_key.clone(),
                dirty_domain(100, 1, "body-base", "body-draft").snapshot(),
            ),
            (
                title_key.clone(),
                dirty_domain(300, 2, "title-base", "title-draft").snapshot(),
            ),
        ]))
        .unwrap();
        let before_recovery = recovery.clone();

        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let before_record = record.clone();
        let mut store = MemoryStore {
            fail: true,
            ..MemoryStore::default()
        };
        let remote_title = remote_from_base(300, 900, "title-base", "remote-title");

        assert!(commit_received_recovery_scalar_domain(
            &mut recovery,
            &mut record,
            &mut store,
            &codec,
            &cursor_codec,
            ReceivedRecoveryScalarState::new(&title_key, &remote_title, Some(rid(400)), &Revision(2)),
        )
        .is_err());

        assert_eq!(recovery, before_recovery);
        assert_eq!(record, before_record);
        assert_eq!(recovery.get(&body_key), before_recovery.get(&body_key));
        assert_eq!(recovery.get(&title_key), before_recovery.get(&title_key));
    }
}
