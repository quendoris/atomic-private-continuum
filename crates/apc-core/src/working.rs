use std::collections::BTreeSet;

use crate::{CoreError, RevisionId, ScalarRegister, ScalarRevision, WorkingEpochId};

/// One crash-safe local editing epoch that has not yet become a portable causal
/// revision.
///
/// `observed_frontier` is captured when the epoch begins. Later transport
/// receipt or unrelated process activity must not rewrite that causal context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingEpoch<T> {
    pub id: WorkingEpochId,
    pub value: T,
    pub observed_frontier: BTreeSet<RevisionId>,
}

/// Crash-recovery image for one working scalar domain.
///
/// This is an in-memory semantic snapshot only. The physical durable encoding
/// remains deliberately unspecified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingSnapshot<T> {
    pub causal: ScalarRegister<T>,
    pub pending: Option<WorkingEpoch<T>>,
}

/// Local working state over one portable scalar merge domain.
///
/// Frequent durable edits update `pending` without minting portable causal
/// revisions. A revision is created only when the epoch is sealed. If remote
/// state is about to become semantically observable while an epoch is pending,
/// that epoch must be sealed first using the frontier it actually observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingScalar<T> {
    causal: ScalarRegister<T>,
    pending: Option<WorkingEpoch<T>>,
}

impl<T: Clone + Eq> WorkingScalar<T> {
    pub fn from_causal(causal: ScalarRegister<T>) -> Result<Self, CoreError> {
        causal.validate()?;
        Ok(Self {
            causal,
            pending: None,
        })
    }

    pub fn causal(&self) -> &ScalarRegister<T> {
        &self.causal
    }

    pub fn pending(&self) -> Option<&WorkingEpoch<T>> {
        self.pending.as_ref()
    }

    pub fn is_dirty(&self) -> bool {
        self.pending.is_some()
    }

    pub fn working_value(&self) -> Option<&T> {
        self.pending
            .as_ref()
            .map(|epoch| &epoch.value)
            .or_else(|| self.causal.materialized())
    }

    /// Starts one local durable edit epoch and captures the causal frontier that
    /// was actually observable at that moment.
    pub fn begin_epoch(&mut self, epoch_id: WorkingEpochId, value: T) -> Result<(), CoreError> {
        if let Some(pending) = &self.pending {
            return Err(CoreError::WorkingEpochAlreadyOpen {
                epoch_id: pending.id,
            });
        }

        self.pending = Some(WorkingEpoch {
            id: epoch_id,
            value,
            observed_frontier: self.causal.frontier_ids(),
        });
        Ok(())
    }

    /// Replaces the crash-safe local value inside the current epoch without
    /// changing causal metadata or creating a portable revision.
    pub fn update_pending(&mut self, value: T) -> Result<(), CoreError> {
        let Some(pending) = self.pending.as_mut() else {
            return Err(CoreError::NoWorkingEpoch);
        };
        pending.value = value;
        Ok(())
    }

    /// Converts the pending working epoch into one portable causal revision.
    pub fn seal(&mut self, revision_id: RevisionId) -> Result<ScalarRevision<T>, CoreError> {
        if self.causal.revision(revision_id).is_some() {
            return Err(CoreError::RevisionAlreadyKnown { revision_id });
        }

        let Some(pending) = self.pending.as_ref() else {
            return Err(CoreError::NoWorkingEpoch);
        };

        let revision = ScalarRevision::new(
            revision_id,
            pending.value.clone(),
            pending.observed_frontier.clone(),
        );
        self.causal.insert_revision(revision.clone())?;
        self.pending = None;
        Ok(revision)
    }

    /// Makes remote state semantically observable.
    ///
    /// A pending local epoch must be sealed before the merge. Supplying the
    /// seal identity at this boundary prevents old local work from falsely
    /// claiming a remote revision that arrived later as its causal parent.
    pub fn observe_remote(
        &mut self,
        remote: &ScalarRegister<T>,
        pre_observation_revision_id: Option<RevisionId>,
    ) -> Result<Option<ScalarRevision<T>>, CoreError> {
        remote.validate()?;

        let sealed = match (&self.pending, pre_observation_revision_id) {
            (Some(_), Some(revision_id)) => Some(self.seal(revision_id)?),
            (Some(_), None) => return Err(CoreError::DirtyObservationRequiresSeal),
            (None, Some(revision_id)) => {
                return Err(CoreError::UnexpectedPreObservationRevision { revision_id })
            }
            (None, None) => None,
        };

        self.causal = self.causal.merge(remote)?;
        Ok(sealed)
    }

    pub fn snapshot(&self) -> WorkingSnapshot<T> {
        WorkingSnapshot {
            causal: self.causal.clone(),
            pending: self.pending.clone(),
        }
    }

    pub fn restore(snapshot: WorkingSnapshot<T>) -> Result<Self, CoreError> {
        snapshot.causal.validate()?;
        if let Some(pending) = &snapshot.pending {
            if pending.observed_frontier != snapshot.causal.frontier_ids() {
                return Err(CoreError::InvalidWorkingSnapshot);
            }
        }

        Ok(Self {
            causal: snapshot.causal,
            pending: snapshot.pending,
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
        let mut register = ScalarRegister::new();
        register.assign(rid(100), "base".to_owned()).unwrap();
        register
    }

    #[test]
    fn many_durable_updates_coalesce_into_one_causal_revision() {
        let mut working = WorkingScalar::from_causal(base()).unwrap();
        working.begin_epoch(wid(1), "edit-0".to_owned()).unwrap();

        for index in 1..10_000 {
            working.update_pending(format!("edit-{index}")).unwrap();
        }

        assert_eq!(working.causal().len(), 1);
        assert_eq!(
            working.pending().unwrap().observed_frontier,
            BTreeSet::from([rid(100)])
        );

        let revision = working.seal(rid(50)).unwrap();
        assert_eq!(revision.parents, BTreeSet::from([rid(100)]));
        assert_eq!(revision.value, "edit-9999");
        assert_eq!(working.causal().len(), 2);
        assert_eq!(
            working.working_value().map(String::as_str),
            Some("edit-9999")
        );
    }

    #[test]
    fn dirty_remote_observation_requires_pre_observation_seal() {
        let mut working = WorkingScalar::from_causal(base()).unwrap();
        working.begin_epoch(wid(1), "local".to_owned()).unwrap();

        let mut remote = base();
        remote.assign(rid(900), "remote".to_owned()).unwrap();

        assert_eq!(
            working.observe_remote(&remote, None).unwrap_err(),
            CoreError::DirtyObservationRequiresSeal
        );
        assert!(working.is_dirty());
    }

    #[test]
    fn remote_observation_preserves_true_concurrency() {
        let mut working = WorkingScalar::from_causal(base()).unwrap();
        working
            .begin_epoch(wid(1), "local-before-remote".to_owned())
            .unwrap();

        let mut remote = base();
        remote.assign(rid(900), "remote".to_owned()).unwrap();

        let sealed = working
            .observe_remote(&remote, Some(rid(50)))
            .unwrap()
            .unwrap();

        assert_eq!(sealed.parents, BTreeSet::from([rid(100)]));
        assert_eq!(
            working.causal().frontier_ids(),
            BTreeSet::from([rid(50), rid(900)])
        );
        assert!(!working.causal().is_ancestor(rid(900), rid(50)));
        assert!(!working.causal().is_ancestor(rid(50), rid(900)));
        assert_eq!(working.working_value().map(String::as_str), Some("remote"));

        working
            .begin_epoch(wid(2), "local-after-remote".to_owned())
            .unwrap();
        let joined = working.seal(rid(10)).unwrap();
        assert_eq!(joined.parents, BTreeSet::from([rid(50), rid(900)]));
        assert_eq!(
            working.working_value().map(String::as_str),
            Some("local-after-remote")
        );
    }

    #[test]
    fn snapshot_restores_pending_value_and_original_frontier() {
        let mut working = WorkingScalar::from_causal(base()).unwrap();
        working.begin_epoch(wid(7), "first".to_owned()).unwrap();
        working.update_pending("durable-latest".to_owned()).unwrap();

        let snapshot = working.snapshot();
        let mut restored = WorkingScalar::restore(snapshot).unwrap();

        assert_eq!(restored.pending().unwrap().id, wid(7));
        assert_eq!(
            restored.pending().unwrap().observed_frontier,
            BTreeSet::from([rid(100)])
        );
        assert_eq!(
            restored.working_value().map(String::as_str),
            Some("durable-latest")
        );

        let sealed = restored.seal(rid(200)).unwrap();
        assert_eq!(sealed.parents, BTreeSet::from([rid(100)]));
    }

    #[test]
    fn restore_rejects_pending_frontier_not_matching_causal_state() {
        let snapshot = WorkingSnapshot {
            causal: base(),
            pending: Some(WorkingEpoch {
                id: wid(1),
                value: "forged".to_owned(),
                observed_frontier: BTreeSet::from([rid(999)]),
            }),
        };

        assert_eq!(
            WorkingScalar::restore(snapshot).unwrap_err(),
            CoreError::InvalidWorkingSnapshot
        );
    }

    #[test]
    fn sealing_cannot_reuse_an_existing_revision_identity() {
        let mut working = WorkingScalar::from_causal(base()).unwrap();
        working.begin_epoch(wid(1), "local".to_owned()).unwrap();

        assert_eq!(
            working.seal(rid(100)).unwrap_err(),
            CoreError::RevisionAlreadyKnown {
                revision_id: rid(100),
            }
        );
        assert!(working.is_dirty());
    }
}
