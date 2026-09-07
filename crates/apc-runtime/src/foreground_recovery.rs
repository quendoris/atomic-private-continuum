use std::collections::BTreeMap;

use apc_core::{ContinuumId, RevisionId};
use apc_crypto::ContentKey;
use apc_sync::{
    DomainKey, DurableSyncRecord, ForegroundSyncLifecycle, ForegroundTransport,
    ForegroundTransportError, OpaqueTransport, SyncRecordStore, TransportCursorCodec,
};

use crate::{
    resume_scalar_recovery_outbox_set_bounded, LocalScalarRecoveryState,
    ScalarRecoveryResumeCycleReport, ScalarRecoveryResumeError, TrustedStateCodec,
};

/// Immutable semantic inputs for one lifecycle-gated multi-domain recovery cycle.
///
/// These values do not carry scheduling or transport order. Pre-observation
/// revision identities remain keyed by semantic merge domain.
pub struct ForegroundRecoveryCycleSpec<'a> {
    key: &'a ContentKey,
    continuum_id: ContinuumId,
    pre_observation_revision_ids: &'a BTreeMap<DomainKey, RevisionId>,
}

impl<'a> ForegroundRecoveryCycleSpec<'a> {
    pub fn new(
        key: &'a ContentKey,
        continuum_id: ContinuumId,
        pre_observation_revision_ids: &'a BTreeMap<DomainKey, RevisionId>,
    ) -> Self {
        Self {
            key,
            continuum_id,
            pre_observation_revision_ids,
        }
    }
}

/// Platform-neutral owner of one foreground-gated opaque transport.
///
/// The runtime starts backgrounded. Platform bindings explicitly enter the
/// foreground before asking for a recovery cycle and enter the background as soon
/// as the application leaves the foreground. No worker, timer, daemon or hidden
/// background retry is created here.
///
/// The concrete transport is intentionally not exposed through this type. Network
/// operations reachable through this runtime therefore always cross the shared
/// `ForegroundSyncLifecycle` gate.
pub struct ForegroundRecoveryRuntime<T> {
    lifecycle: ForegroundSyncLifecycle,
    transport: ForegroundTransport<T>,
}

impl<T> ForegroundRecoveryRuntime<T> {
    pub fn new(transport: T) -> Self {
        let lifecycle = ForegroundSyncLifecycle::new();
        let transport = lifecycle.guard(transport);
        Self {
            lifecycle,
            transport,
        }
    }

    pub fn enter_foreground(&self) {
        self.lifecycle.enter_foreground();
    }

    pub fn enter_background(&self) {
        self.lifecycle.enter_background();
    }

    pub fn is_foreground(&self) -> bool {
        self.lifecycle.is_foreground()
    }
}

pub type ForegroundRecoveryCycleResult<R, TransportError, TrustedError, StoreError, CursorError> =
    Result<
        ScalarRecoveryResumeCycleReport<R>,
        ScalarRecoveryResumeError<
            ForegroundTransportError<TransportError>,
            TrustedError,
            StoreError,
            CursorError,
        >,
    >;

impl<T> ForegroundRecoveryRuntime<T>
where
    T: OpaqueTransport,
{
    /// Run one hard-bounded recovery/outbox cycle through the foreground gate.
    ///
    /// If the application is backgrounded, the first attempted transport call
    /// fails before reaching the inner transport. If the application backgrounds
    /// between transport operations, every later operation is independently
    /// gated. Unknown outcomes from an already-running remote mutation remain a
    /// durable outbox/reconciliation problem rather than being guessed here.
    #[allow(clippy::type_complexity)]
    pub fn resume_scalar_recovery<S, TC, CC>(
        &mut self,
        recovery: &mut LocalScalarRecoveryState,
        record: &mut DurableSyncRecord,
        store: &mut S,
        trusted_codec: &TC,
        cursor_codec: &CC,
        spec: ForegroundRecoveryCycleSpec<'_>,
    ) -> ForegroundRecoveryCycleResult<T::Revision, T::Error, TC::Error, S::Error, CC::Error>
    where
        S: SyncRecordStore,
        TC: TrustedStateCodec<LocalScalarRecoveryState>,
        CC: TransportCursorCodec<T::Revision>,
    {
        resume_scalar_recovery_outbox_set_bounded(
            recovery,
            record,
            store,
            trusted_codec,
            cursor_codec,
            &mut self.transport,
            spec.key,
            spec.continuum_id,
            spec.pre_observation_revision_ids,
        )
    }
}

#[cfg(test)]
mod tests {
    use apc_sync::{FetchOutcome, PublishOutcome, TransportCursor};

    use crate::{
        DevelopmentMultiScalarTrustedStateCodec, ScalarRecoveryCatchUpOutcome,
        ScalarRecoveryResumeAction,
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
    }

    impl SyncRecordStore for MemoryStore {
        type Error = &'static str;

        fn persist(&mut self, record: &DurableSyncRecord) -> Result<(), Self::Error> {
            self.committed = Some(record.clone());
            Ok(())
        }
    }

    struct CountingTransport {
        head: Revision,
        fetch_calls: usize,
        publish_calls: usize,
    }

    impl OpaqueTransport for CountingTransport {
        type Revision = Revision;
        type Error = &'static str;

        fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
            Ok(Some(self.head))
        }

        fn fetch_since(
            &mut self,
            known_head: Option<&Self::Revision>,
        ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
            self.fetch_calls += 1;
            if known_head == Some(&self.head) {
                Ok(FetchOutcome::UpToDate {
                    head: Some(self.head),
                })
            } else {
                Ok(FetchOutcome::Changed {
                    head: self.head,
                    objects: Vec::new(),
                })
            }
        }

        fn publish(
            &mut self,
            _expected_head: Option<&Self::Revision>,
            _objects: &[Vec<u8>],
        ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
            self.publish_calls += 1;
            Err("publish is not expected in this test")
        }
    }

    fn continuum_id(value: u8) -> ContinuumId {
        ContinuumId::from_bytes([value; 32])
    }

    #[test]
    fn lifecycle_gates_complete_recovery_resume_and_reuses_durable_cursor_on_reentry() {
        let codec = DevelopmentMultiScalarTrustedStateCodec;
        let cursor_codec = RevisionCodec;
        let key = ContentKey::from_bytes([0xA7; 32]);
        let pre_observation = BTreeMap::new();
        let mut recovery = LocalScalarRecoveryState::new();
        let mut record = DurableSyncRecord::new(
            codec.encode(&recovery).unwrap(),
            Some(cursor_codec.encode(&Revision(1)).unwrap()),
        );
        let original_record = record.clone();
        let mut store = MemoryStore::default();
        let mut runtime = ForegroundRecoveryRuntime::new(CountingTransport {
            head: Revision(2),
            fetch_calls: 0,
            publish_calls: 0,
        });

        assert!(!runtime.is_foreground());
        assert!(runtime
            .resume_scalar_recovery(
                &mut recovery,
                &mut record,
                &mut store,
                &codec,
                &cursor_codec,
                ForegroundRecoveryCycleSpec::new(&key, continuum_id(1), &pre_observation),
            )
            .is_err());
        assert_eq!(runtime.transport.inner().fetch_calls, 0);
        assert_eq!(runtime.transport.inner().publish_calls, 0);
        assert_eq!(record, original_record);

        runtime.enter_foreground();
        let first = runtime
            .resume_scalar_recovery(
                &mut recovery,
                &mut record,
                &mut store,
                &codec,
                &cursor_codec,
                ForegroundRecoveryCycleSpec::new(&key, continuum_id(1), &pre_observation),
            )
            .unwrap();
        assert_eq!(
            first.initial.catch_up,
            ScalarRecoveryCatchUpOutcome::CursorAdvancedWithoutSemanticObjects {
                head: Revision(2)
            }
        );
        assert_eq!(first.initial.action, ScalarRecoveryResumeAction::None);
        assert!(first.post_conflict.is_none());
        assert_eq!(runtime.transport.inner().fetch_calls, 1);
        assert_eq!(
            cursor_codec
                .decode(record.applied_cursor().unwrap())
                .unwrap(),
            Revision(2)
        );
        assert_eq!(store.committed.as_ref(), Some(&record));

        runtime.enter_background();
        let foreground_committed = record.clone();
        assert!(runtime
            .resume_scalar_recovery(
                &mut recovery,
                &mut record,
                &mut store,
                &codec,
                &cursor_codec,
                ForegroundRecoveryCycleSpec::new(&key, continuum_id(1), &pre_observation),
            )
            .is_err());
        assert_eq!(runtime.transport.inner().fetch_calls, 1);
        assert_eq!(record, foreground_committed);

        runtime.enter_foreground();
        let second = runtime
            .resume_scalar_recovery(
                &mut recovery,
                &mut record,
                &mut store,
                &codec,
                &cursor_codec,
                ForegroundRecoveryCycleSpec::new(&key, continuum_id(1), &pre_observation),
            )
            .unwrap();
        assert_eq!(
            second.initial.catch_up,
            ScalarRecoveryCatchUpOutcome::UpToDate {
                head: Some(Revision(2))
            }
        );
        assert_eq!(second.initial.action, ScalarRecoveryResumeAction::None);
        assert!(second.post_conflict.is_none());
        assert_eq!(runtime.transport.inner().fetch_calls, 2);
        assert_eq!(runtime.transport.inner().publish_calls, 0);
    }
}
