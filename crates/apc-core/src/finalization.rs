use std::collections::{BTreeMap, BTreeSet};

use crate::{CoreError, RevisionId, ScalarRegister, ScalarRevision};

/// Immutable portable statement frozen before authentication and transport.
///
/// This type deliberately contains no signature or key-evolution fields yet.
/// Finalization freezes semantic statement contents first; a later cryptographic
/// layer will authenticate that frozen representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalizedStatement<T> {
    pub revision_id: RevisionId,
    pub value: T,
    pub parents: BTreeSet<RevisionId>,
}

impl<T: Clone> From<&ScalarRevision<T>> for FinalizedStatement<T> {
    fn from(revision: &ScalarRevision<T>) -> Self {
        Self {
            revision_id: revision.id,
            value: revision.value.clone(),
            parents: revision.parents.clone(),
        }
    }
}

/// Crash-recovery image for local finalization/exposure bookkeeping.
///
/// The causal register itself is persisted by the owning domain state and is
/// supplied again during restore for validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalizationSnapshot<T> {
    pub local_revision_ids: BTreeSet<RevisionId>,
    pub finalized: BTreeMap<RevisionId, FinalizedStatement<T>>,
    pub exposed_local_ids: BTreeSet<RevisionId>,
    pub handed_off_local_ids: BTreeSet<RevisionId>,
}

/// Tracks which local causal identities are still private, which statements are
/// frozen, and which identities may already be externally observable.
///
/// Transport acknowledgement is intentionally absent: exposure begins at
/// handoff because an acknowledgement can be lost after successful delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalizationLedger<T> {
    local_revision_ids: BTreeSet<RevisionId>,
    finalized: BTreeMap<RevisionId, FinalizedStatement<T>>,
    exposed_local_ids: BTreeSet<RevisionId>,
    handed_off_local_ids: BTreeSet<RevisionId>,
}

impl<T> Default for FinalizationLedger<T> {
    fn default() -> Self {
        Self {
            local_revision_ids: BTreeSet::new(),
            finalized: BTreeMap::new(),
            exposed_local_ids: BTreeSet::new(),
            handed_off_local_ids: BTreeSet::new(),
        }
    }
}

impl<T: Clone + Eq> FinalizationLedger<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn local_revision_ids(&self) -> &BTreeSet<RevisionId> {
        &self.local_revision_ids
    }

    pub fn finalized(&self) -> &BTreeMap<RevisionId, FinalizedStatement<T>> {
        &self.finalized
    }

    pub fn exposed_local_ids(&self) -> &BTreeSet<RevisionId> {
        &self.exposed_local_ids
    }

    pub fn handed_off_local_ids(&self) -> &BTreeSet<RevisionId> {
        &self.handed_off_local_ids
    }

    /// Registers ownership of a causal revision created by this replica.
    ///
    /// This does not freeze the statement and therefore does not prevent a
    /// future pre-finalization canonicalization step from replacing private
    /// causal structure, provided the revision identity remains valid.
    pub fn register_local(
        &mut self,
        causal: &ScalarRegister<T>,
        revision_id: RevisionId,
    ) -> Result<(), CoreError> {
        if causal.revision(revision_id).is_none() {
            return Err(CoreError::UnknownRevision { revision_id });
        }
        self.local_revision_ids.insert(revision_id);
        Ok(())
    }

    /// Freezes one local causal statement. Repeating finalization for the exact
    /// same immutable statement is idempotent.
    pub fn finalize(
        &mut self,
        causal: &ScalarRegister<T>,
        revision_id: RevisionId,
    ) -> Result<FinalizedStatement<T>, CoreError> {
        if !self.local_revision_ids.contains(&revision_id) {
            return Err(CoreError::UnknownLocalRevision { revision_id });
        }

        let revision = causal
            .revision(revision_id)
            .ok_or(CoreError::UnknownRevision { revision_id })?;
        let statement = FinalizedStatement::from(revision);

        if let Some(existing) = self.finalized.get(&revision_id) {
            if existing != &statement {
                return Err(CoreError::FinalizedStatementConflict { revision_id });
            }
            return Ok(existing.clone());
        }

        self.finalized.insert(revision_id, statement.clone());
        Ok(statement)
    }

    /// Validates that no already-finalized local statement has been removed or
    /// rewritten in the supplied causal state.
    pub fn validate_against(&self, causal: &ScalarRegister<T>) -> Result<(), CoreError> {
        causal.validate()?;

        for (revision_id, statement) in &self.finalized {
            let revision = causal
                .revision(*revision_id)
                .ok_or(CoreError::UnknownRevision {
                    revision_id: *revision_id,
                })?;
            if statement != &FinalizedStatement::from(revision) {
                return Err(CoreError::FinalizedStatementConflict {
                    revision_id: *revision_id,
                });
            }
        }

        Ok(())
    }

    /// Marks a set of revisions as handed to an external transport.
    ///
    /// Every locally owned revision in the transitive causal dependency closure
    /// must already be finalized. Those local identities become permanently
    /// exposed for the private-squashing class at this boundary.
    pub fn handoff<I>(
        &mut self,
        causal: &ScalarRegister<T>,
        revision_ids: I,
    ) -> Result<(), CoreError>
    where
        I: IntoIterator<Item = RevisionId>,
    {
        self.validate_against(causal)?;

        let direct: BTreeSet<RevisionId> = revision_ids.into_iter().collect();
        let mut closure = BTreeSet::new();
        let mut stack: Vec<RevisionId> = direct.iter().copied().collect();

        while let Some(revision_id) = stack.pop() {
            if !closure.insert(revision_id) {
                continue;
            }
            let revision = causal
                .revision(revision_id)
                .ok_or(CoreError::UnknownRevision { revision_id })?;
            stack.extend(revision.parents.iter().copied());
        }

        for revision_id in closure
            .iter()
            .copied()
            .filter(|id| self.local_revision_ids.contains(id))
        {
            if !self.finalized.contains_key(&revision_id) {
                return Err(CoreError::HandoffRequiresFinalizedRevision { revision_id });
            }
        }

        self.exposed_local_ids.extend(
            closure
                .iter()
                .copied()
                .filter(|id| self.local_revision_ids.contains(id)),
        );
        self.handed_off_local_ids.extend(
            direct
                .iter()
                .copied()
                .filter(|id| self.local_revision_ids.contains(id)),
        );
        Ok(())
    }

    pub fn snapshot(&self) -> FinalizationSnapshot<T> {
        FinalizationSnapshot {
            local_revision_ids: self.local_revision_ids.clone(),
            finalized: self.finalized.clone(),
            exposed_local_ids: self.exposed_local_ids.clone(),
            handed_off_local_ids: self.handed_off_local_ids.clone(),
        }
    }

    pub fn restore(
        snapshot: FinalizationSnapshot<T>,
        causal: &ScalarRegister<T>,
    ) -> Result<Self, CoreError> {
        let ledger = Self {
            local_revision_ids: snapshot.local_revision_ids,
            finalized: snapshot.finalized,
            exposed_local_ids: snapshot.exposed_local_ids,
            handed_off_local_ids: snapshot.handed_off_local_ids,
        };

        if !ledger
            .exposed_local_ids
            .is_subset(&ledger.local_revision_ids)
            || !ledger
                .handed_off_local_ids
                .is_subset(&ledger.exposed_local_ids)
            || !ledger
                .finalized
                .keys()
                .all(|id| ledger.local_revision_ids.contains(id))
        {
            return Err(CoreError::InvalidFinalizationSnapshot);
        }

        for revision_id in &ledger.local_revision_ids {
            if causal.revision(*revision_id).is_none() {
                return Err(CoreError::UnknownRevision {
                    revision_id: *revision_id,
                });
            }
        }

        ledger.validate_against(causal)?;
        Ok(ledger)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::id::LOGICAL_ID_BYTES;

    fn rid(value: u64) -> RevisionId {
        let mut bytes = [0_u8; LOGICAL_ID_BYTES];
        bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        RevisionId::from_bytes(bytes)
    }

    fn chain() -> ScalarRegister<&'static str> {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), "remote-base").unwrap();
        causal.assign(rid(200), "local-parent").unwrap();
        causal.assign(rid(300), "local-child").unwrap();
        causal
    }

    #[test]
    fn finalization_is_idempotent_and_preserves_revision_identity() {
        let causal = chain();
        let before = causal.materialized_revision().unwrap().id;
        let mut ledger = FinalizationLedger::new();
        ledger.register_local(&causal, rid(300)).unwrap();

        let first = ledger.finalize(&causal, rid(300)).unwrap();
        let second = ledger.finalize(&causal, rid(300)).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.revision_id, rid(300));
        assert_eq!(causal.materialized_revision().unwrap().id, before);
        assert_eq!(ledger.finalized().len(), 1);
    }

    #[test]
    fn handoff_requires_finalized_local_ancestor_closure() {
        let causal = chain();
        let mut ledger = FinalizationLedger::new();
        ledger.register_local(&causal, rid(200)).unwrap();
        ledger.register_local(&causal, rid(300)).unwrap();
        ledger.finalize(&causal, rid(300)).unwrap();

        assert_eq!(
            ledger.handoff(&causal, [rid(300)]).unwrap_err(),
            CoreError::HandoffRequiresFinalizedRevision {
                revision_id: rid(200),
            }
        );
        assert!(ledger.exposed_local_ids().is_empty());

        ledger.finalize(&causal, rid(200)).unwrap();
        ledger.handoff(&causal, [rid(300)]).unwrap();

        assert_eq!(
            ledger.exposed_local_ids(),
            &BTreeSet::from([rid(200), rid(300)])
        );
        assert_eq!(ledger.handed_off_local_ids(), &BTreeSet::from([rid(300)]));
    }

    #[test]
    fn remote_ancestors_do_not_require_local_finalization() {
        let mut causal = ScalarRegister::new();
        causal.assign(rid(100), "remote").unwrap();
        causal.assign(rid(200), "local").unwrap();

        let mut ledger = FinalizationLedger::new();
        ledger.register_local(&causal, rid(200)).unwrap();
        ledger.finalize(&causal, rid(200)).unwrap();
        ledger.handoff(&causal, [rid(200)]).unwrap();

        assert_eq!(ledger.exposed_local_ids(), &BTreeSet::from([rid(200)]));
    }

    #[test]
    fn finalized_statement_cannot_be_rewritten() {
        let mut original = ScalarRegister::new();
        original.assign(rid(100), "base").unwrap();
        original.assign(rid(200), "original").unwrap();

        let mut ledger = FinalizationLedger::new();
        ledger.register_local(&original, rid(200)).unwrap();
        ledger.finalize(&original, rid(200)).unwrap();

        let forged = ScalarRegister::from_revisions([
            ScalarRevision::new(rid(100), "base", BTreeSet::new()),
            ScalarRevision::new(rid(200), "rewritten", BTreeSet::from([rid(100)])),
        ])
        .unwrap();

        assert_eq!(
            ledger.validate_against(&forged).unwrap_err(),
            CoreError::FinalizedStatementConflict {
                revision_id: rid(200),
            }
        );
    }

    #[test]
    fn crash_snapshot_restores_exposure_without_ack_semantics() {
        let causal = chain();
        let mut ledger = FinalizationLedger::new();
        ledger.register_local(&causal, rid(200)).unwrap();
        ledger.register_local(&causal, rid(300)).unwrap();
        ledger.finalize(&causal, rid(200)).unwrap();
        ledger.finalize(&causal, rid(300)).unwrap();
        ledger.handoff(&causal, [rid(300)]).unwrap();

        let restored = FinalizationLedger::restore(ledger.snapshot(), &causal).unwrap();

        assert_eq!(restored, ledger);
        assert_eq!(
            restored.exposed_local_ids(),
            &BTreeSet::from([rid(200), rid(300)])
        );
    }
}
