use crate::{
    CoreError, FinalizationLedger, FinalizationSnapshot, FinalizedStatement, RevisionId,
    ScalarRegister, ScalarRevision, WorkingEpoch, WorkingEpochId, WorkingScalar, WorkingSnapshot,
};

/// Crash-recovery image for one local scalar merge domain.
///
/// The snapshot composes working-state and finalization bookkeeping so a restart
/// cannot restore one boundary while silently forgetting the other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalScalarSnapshot<T> {
    pub working: WorkingSnapshot<T>,
    pub finalization: FinalizationSnapshot<T>,
}

/// Minimal local state machine for one scalar merge domain.
///
/// This type exists to keep the already-validated boundaries coupled correctly:
/// a sealed local revision is registered as local, remote observation seals dirty
/// work before merge, finalization freezes an existing local statement, and
/// transport handoff records exposure before any acknowledgement can exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalScalarDomain<T> {
    working: WorkingScalar<T>,
    finalization: FinalizationLedger<T>,
}

impl<T: Clone + Eq> LocalScalarDomain<T> {
    pub fn from_causal(causal: ScalarRegister<T>) -> Result<Self, CoreError> {
        Ok(Self {
            working: WorkingScalar::from_causal(causal)?,
            finalization: FinalizationLedger::new(),
        })
    }

    pub fn causal(&self) -> &ScalarRegister<T> {
        self.working.causal()
    }

    pub fn pending(&self) -> Option<&WorkingEpoch<T>> {
        self.working.pending()
    }

    pub fn working_value(&self) -> Option<&T> {
        self.working.working_value()
    }

    pub fn finalization(&self) -> &FinalizationLedger<T> {
        &self.finalization
    }

    pub fn begin_epoch(&mut self, epoch_id: WorkingEpochId, value: T) -> Result<(), CoreError> {
        self.working.begin_epoch(epoch_id, value)
    }

    pub fn update_pending(&mut self, value: T) -> Result<(), CoreError> {
        self.working.update_pending(value)
    }

    /// Seals one pending local epoch and atomically updates local ownership
    /// bookkeeping in the in-memory state machine.
    pub fn seal_local(&mut self, revision_id: RevisionId) -> Result<ScalarRevision<T>, CoreError> {
        let revision = self.working.seal(revision_id)?;
        self.finalization
            .register_local(self.working.causal(), revision_id)?;
        Ok(revision)
    }

    /// Applies a remote scalar state at the semantic observation boundary.
    ///
    /// If dirty local work must be sealed first, the newly created revision is
    /// also registered as local before this method returns.
    pub fn observe_remote(
        &mut self,
        remote: &ScalarRegister<T>,
        pre_observation_revision_id: Option<RevisionId>,
    ) -> Result<Option<ScalarRevision<T>>, CoreError> {
        let sealed = self
            .working
            .observe_remote(remote, pre_observation_revision_id)?;

        if let Some(revision) = &sealed {
            self.finalization
                .register_local(self.working.causal(), revision.id)?;
        }

        Ok(sealed)
    }

    pub fn finalize(
        &mut self,
        revision_id: RevisionId,
    ) -> Result<FinalizedStatement<T>, CoreError> {
        self.finalization.finalize(self.working.causal(), revision_id)
    }

    pub fn handoff<I>(&mut self, revision_ids: I) -> Result<(), CoreError>
    where
        I: IntoIterator<Item = RevisionId>,
    {
        self.finalization
            .handoff(self.working.causal(), revision_ids)
    }

    pub fn snapshot(&self) -> LocalScalarSnapshot<T> {
        LocalScalarSnapshot {
            working: self.working.snapshot(),
            finalization: self.finalization.snapshot(),
        }
    }

    pub fn restore(snapshot: LocalScalarSnapshot<T>) -> Result<Self, CoreError> {
        let working = WorkingScalar::restore(snapshot.working)?;
        let finalization =
            FinalizationLedger::restore(snapshot.finalization, working.causal())?;

        Ok(Self {
            working,
            finalization,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::id::LOGICAL_ID_BYTES;

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

    fn base() -> ScalarRegister<String> {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), "base".to_owned()).unwrap();
        causal
    }

    #[test]
    fn remote_observation_registers_the_pre_observation_local_revision() {
        let mut domain = LocalScalarDomain::from_causal(base()).unwrap();
        domain
            .begin_epoch(wid(1), "local-before-remote".to_owned())
            .unwrap();

        let mut remote = base();
        remote.assign(rid(900), "remote".to_owned()).unwrap();

        let sealed = domain
            .observe_remote(&remote, Some(rid(50)))
            .unwrap()
            .unwrap();

        assert_eq!(sealed.parents, BTreeSet::from([rid(100)]));
        assert!(domain
            .finalization()
            .local_revision_ids()
            .contains(&rid(50)));
        assert_eq!(
            domain.causal().frontier_ids(),
            BTreeSet::from([rid(50), rid(900)])
        );
    }

    #[test]
    fn sealed_local_revision_can_finalize_and_cross_handoff_boundary() {
        let mut domain = LocalScalarDomain::from_causal(base()).unwrap();
        domain.begin_epoch(wid(1), "local".to_owned()).unwrap();
        domain.seal_local(rid(200)).unwrap();

        let statement = domain.finalize(rid(200)).unwrap();
        assert_eq!(statement.revision_id, rid(200));

        domain.handoff([rid(200)]).unwrap();
        assert_eq!(
            domain.finalization().exposed_local_ids(),
            &BTreeSet::from([rid(200)])
        );
    }

    #[test]
    fn snapshot_restores_working_and_exposure_bookkeeping_together() {
        let mut domain = LocalScalarDomain::from_causal(base()).unwrap();
        domain.begin_epoch(wid(1), "first".to_owned()).unwrap();
        domain.update_pending("durable-latest".to_owned()).unwrap();
        domain.seal_local(rid(200)).unwrap();
        domain.finalize(rid(200)).unwrap();
        domain.handoff([rid(200)]).unwrap();

        let restored = LocalScalarDomain::restore(domain.snapshot()).unwrap();

        assert_eq!(restored, domain);
        assert_eq!(
            restored.working_value().map(String::as_str),
            Some("durable-latest")
        );
        assert!(restored
            .finalization()
            .exposed_local_ids()
            .contains(&rid(200)));
    }
}
