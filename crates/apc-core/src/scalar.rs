use std::collections::{BTreeMap, BTreeSet};

use crate::{CoreError, RevisionId};

/// One immutable scalar statement in the current direct-frontier causal model.
///
/// `parents` contains the causal frontier observed when this statement was
/// created. The representation is deliberately in-memory and pre-format: the
/// portable encoding and long-term checkpoint representation are still open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarRevision<T> {
    pub id: RevisionId,
    pub value: T,
    pub parents: BTreeSet<RevisionId>,
}

impl<T> ScalarRevision<T> {
    pub fn new(id: RevisionId, value: T, parents: BTreeSet<RevisionId>) -> Self {
        Self { id, value, parents }
    }
}

/// State-based scalar register using explicit direct causal parents.
///
/// Causal descendants dominate ancestors. Genuinely concurrent frontier
/// revisions are materialized using canonical `RevisionId` byte ordering. ID
/// order is therefore only a deterministic tie-break and never a clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarRegister<T> {
    revisions: BTreeMap<RevisionId, ScalarRevision<T>>,
}

impl<T> Default for ScalarRegister<T> {
    fn default() -> Self {
        Self {
            revisions: BTreeMap::new(),
        }
    }
}

impl<T: Clone + Eq> ScalarRegister<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_revisions<I>(revisions: I) -> Result<Self, CoreError>
    where
        I: IntoIterator<Item = ScalarRevision<T>>,
    {
        let mut map = BTreeMap::new();

        for revision in revisions {
            match map.get(&revision.id) {
                Some(existing) if existing == &revision => {}
                Some(_) => {
                    return Err(CoreError::DuplicateRevisionConflict {
                        revision_id: revision.id,
                    })
                }
                None => {
                    map.insert(revision.id, revision);
                }
            }
        }

        let register = Self { revisions: map };
        register.validate()?;
        Ok(register)
    }

    pub fn len(&self) -> usize {
        self.revisions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.revisions.is_empty()
    }

    pub fn revision(&self, id: RevisionId) -> Option<&ScalarRevision<T>> {
        self.revisions.get(&id)
    }

    pub fn revisions(&self) -> impl Iterator<Item = &ScalarRevision<T>> {
        self.revisions.values()
    }

    /// Creates a local successor from exactly the currently observed frontier.
    pub fn assign(&mut self, id: RevisionId, value: T) -> Result<(), CoreError> {
        let parents = self.frontier_ids();
        self.insert_revision(ScalarRevision::new(id, value, parents))
    }

    /// Inserts one complete revision into an already valid register.
    ///
    /// This method intentionally requires every direct parent to be present in
    /// the register. Baseline-aware sync capsules and checkpoint coverage will
    /// use a separate import boundary rather than making missing dependencies
    /// silently valid here.
    pub fn insert_revision(&mut self, revision: ScalarRevision<T>) -> Result<(), CoreError> {
        if let Some(existing) = self.revisions.get(&revision.id) {
            return if existing == &revision {
                Ok(())
            } else {
                Err(CoreError::DuplicateRevisionConflict {
                    revision_id: revision.id,
                })
            };
        }

        for parent in &revision.parents {
            if !self.revisions.contains_key(parent) {
                return Err(CoreError::MissingParent {
                    revision_id: revision.id,
                    parent_id: *parent,
                });
            }
        }

        self.revisions.insert(revision.id, revision);
        Ok(())
    }

    /// State merge. Duplicate delivery of an identical statement is harmless;
    /// reuse of one `RevisionId` for a different statement is rejected.
    pub fn merge(&self, other: &Self) -> Result<Self, CoreError> {
        let mut merged = self.revisions.clone();

        for (id, revision) in &other.revisions {
            match merged.get(id) {
                Some(existing) if existing == revision => {}
                Some(_) => {
                    return Err(CoreError::DuplicateRevisionConflict { revision_id: *id })
                }
                None => {
                    merged.insert(*id, revision.clone());
                }
            }
        }

        let result = Self { revisions: merged };
        result.validate()?;
        Ok(result)
    }

    /// Returns the set of causally maximal revision identities.
    pub fn frontier_ids(&self) -> BTreeSet<RevisionId> {
        let mut dominated = BTreeSet::new();

        for revision in self.revisions.values() {
            let mut stack: Vec<RevisionId> = revision.parents.iter().copied().collect();
            while let Some(id) = stack.pop() {
                if !dominated.insert(id) {
                    continue;
                }
                if let Some(parent_revision) = self.revisions.get(&id) {
                    stack.extend(parent_revision.parents.iter().copied());
                }
            }
        }

        self.revisions
            .keys()
            .copied()
            .filter(|id| !dominated.contains(id))
            .collect()
    }

    /// Materializes the visible revision.
    ///
    /// Every member of a valid frontier is concurrent with every other member,
    /// so canonical ID ordering is used only at this final tie-break step.
    pub fn materialized_revision(&self) -> Option<&ScalarRevision<T>> {
        let winner = self.frontier_ids().into_iter().next_back()?;
        self.revisions.get(&winner)
    }

    pub fn materialized(&self) -> Option<&T> {
        self.materialized_revision().map(|revision| &revision.value)
    }

    /// Strict causal ancestry: a revision is not considered its own ancestor.
    pub fn is_ancestor(&self, ancestor: RevisionId, descendant: RevisionId) -> bool {
        let Some(descendant_revision) = self.revisions.get(&descendant) else {
            return false;
        };

        let mut seen = BTreeSet::new();
        let mut stack: Vec<RevisionId> = descendant_revision.parents.iter().copied().collect();

        while let Some(id) = stack.pop() {
            if id == ancestor {
                return true;
            }
            if !seen.insert(id) {
                continue;
            }
            if let Some(revision) = self.revisions.get(&id) {
                stack.extend(revision.parents.iter().copied());
            }
        }

        false
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        for revision in self.revisions.values() {
            for parent in &revision.parents {
                if !self.revisions.contains_key(parent) {
                    return Err(CoreError::MissingParent {
                        revision_id: revision.id,
                        parent_id: *parent,
                    });
                }
            }
        }

        let mut visiting = BTreeSet::new();
        let mut complete = BTreeSet::new();

        for id in self.revisions.keys().copied() {
            self.visit_for_cycle(id, &mut visiting, &mut complete)?;
        }

        Ok(())
    }

    fn visit_for_cycle(
        &self,
        id: RevisionId,
        visiting: &mut BTreeSet<RevisionId>,
        complete: &mut BTreeSet<RevisionId>,
    ) -> Result<(), CoreError> {
        if complete.contains(&id) {
            return Ok(());
        }
        if !visiting.insert(id) {
            return Err(CoreError::CausalCycle { revision_id: id });
        }

        if let Some(revision) = self.revisions.get(&id) {
            for parent in &revision.parents {
                self.visit_for_cycle(*parent, visiting, complete)?;
            }
        }

        visiting.remove(&id);
        complete.insert(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::LOGICAL_ID_BYTES;

    fn rid(value: u64) -> RevisionId {
        let mut bytes = [0_u8; LOGICAL_ID_BYTES];
        bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        RevisionId::from_bytes(bytes)
    }

    fn singleton(id: RevisionId) -> BTreeSet<RevisionId> {
        BTreeSet::from([id])
    }

    #[test]
    fn causal_successor_beats_larger_ancestor_id() {
        let mut register = ScalarRegister::new();
        register.assign(rid(900), "ancestor").unwrap();
        register.assign(rid(50), "successor").unwrap();

        assert_eq!(register.materialized(), Some(&"successor"));
        assert!(register.is_ancestor(rid(900), rid(50)));
    }

    #[test]
    fn concurrent_frontier_uses_only_canonical_id_tie_break() {
        let mut base = ScalarRegister::new();
        base.assign(rid(1), "base").unwrap();

        let mut left = base.clone();
        left.assign(rid(200), "left").unwrap();

        let mut right = base;
        right.assign(rid(100), "right").unwrap();

        let merged = left.merge(&right).unwrap();

        assert_eq!(merged.frontier_ids(), BTreeSet::from([rid(100), rid(200)]));
        assert_eq!(merged.materialized(), Some(&"left"));
        assert!(!merged.is_ancestor(rid(100), rid(200)));
        assert!(!merged.is_ancestor(rid(200), rid(100)));
    }

    #[test]
    fn join_revision_observes_complete_concurrent_frontier() {
        let mut base = ScalarRegister::new();
        base.assign(rid(1), "base").unwrap();

        let mut left = base.clone();
        left.assign(rid(800), "left").unwrap();
        let mut right = base;
        right.assign(rid(900), "right").unwrap();

        let mut joined = left.merge(&right).unwrap();
        joined.assign(rid(2), "joined").unwrap();

        assert_eq!(joined.frontier_ids(), BTreeSet::from([rid(2)]));
        assert_eq!(joined.revision(rid(2)).unwrap().parents, BTreeSet::from([rid(800), rid(900)]));
        assert_eq!(joined.materialized(), Some(&"joined"));
    }

    #[test]
    fn merge_is_commutative_associative_and_idempotent_for_valid_states() {
        let mut base = ScalarRegister::new();
        base.assign(rid(1), "base").unwrap();

        let mut a = base.clone();
        a.assign(rid(10), "a").unwrap();
        let mut b = base.clone();
        b.assign(rid(20), "b").unwrap();
        let mut c = base;
        c.assign(rid(30), "c").unwrap();

        assert_eq!(a.merge(&b).unwrap(), b.merge(&a).unwrap());
        assert_eq!(a.merge(&a).unwrap(), a);

        let left_grouped = a.merge(&b).unwrap().merge(&c).unwrap();
        let right_grouped = a.merge(&b.merge(&c).unwrap()).unwrap();
        assert_eq!(left_grouped, right_grouped);
    }

    #[test]
    fn stale_state_cannot_roll_back_causal_descendant() {
        let mut old = ScalarRegister::new();
        old.assign(rid(500), "old").unwrap();

        let mut current = old.clone();
        current.assign(rid(10), "current").unwrap();

        assert_eq!(current.merge(&old).unwrap().materialized(), Some(&"current"));
        assert_eq!(old.merge(&current).unwrap().materialized(), Some(&"current"));
    }

    #[test]
    fn missing_parent_is_rejected() {
        let revision = ScalarRevision::new(rid(2), "orphan", singleton(rid(1)));
        let error = ScalarRegister::from_revisions([revision]).unwrap_err();

        assert_eq!(
            error,
            CoreError::MissingParent {
                revision_id: rid(2),
                parent_id: rid(1),
            }
        );
    }

    #[test]
    fn conflicting_reuse_of_revision_id_is_rejected() {
        let left = ScalarRegister::from_revisions([ScalarRevision::new(
            rid(1),
            "left",
            BTreeSet::new(),
        )])
        .unwrap();
        let right = ScalarRegister::from_revisions([ScalarRevision::new(
            rid(1),
            "right",
            BTreeSet::new(),
        )])
        .unwrap();

        assert_eq!(
            left.merge(&right).unwrap_err(),
            CoreError::DuplicateRevisionConflict {
                revision_id: rid(1),
            }
        );
    }

    #[test]
    fn causal_cycle_is_rejected_on_import() {
        let a = ScalarRevision::new(rid(1), "a", singleton(rid(2)));
        let b = ScalarRevision::new(rid(2), "b", singleton(rid(1)));

        let error = ScalarRegister::from_revisions([a, b]).unwrap_err();
        assert!(matches!(error, CoreError::CausalCycle { .. }));
    }
}
