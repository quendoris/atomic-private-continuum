use std::collections::BTreeMap;

use crate::{AtomId, ContinuumId, CoreError, MergeState};

/// Stable atom-identity map. Absence is not deletion semantics; lifecycle state
/// belongs inside the atom payload once the lifecycle domain is finalized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtomMap<A> {
    atoms: BTreeMap<AtomId, A>,
}

impl<A> Default for AtomMap<A> {
    fn default() -> Self {
        Self {
            atoms: BTreeMap::new(),
        }
    }
}

impl<A> AtomMap<A> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.atoms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    pub fn get(&self, atom_id: AtomId) -> Option<&A> {
        self.atoms.get(&atom_id)
    }

    pub fn get_mut(&mut self, atom_id: AtomId) -> Option<&mut A> {
        self.atoms.get_mut(&atom_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (AtomId, &A)> {
        self.atoms.iter().map(|(id, atom)| (*id, atom))
    }

    pub fn insert_new(&mut self, atom_id: AtomId, atom: A) -> Result<(), CoreError> {
        if self.atoms.contains_key(&atom_id) {
            return Err(CoreError::DuplicateAtomId { atom_id });
        }
        self.atoms.insert(atom_id, atom);
        Ok(())
    }
}

impl<A: Clone + MergeState> MergeState for AtomMap<A> {
    fn merge_state(&self, other: &Self) -> Result<Self, CoreError> {
        let mut merged = self.atoms.clone();

        for (atom_id, right) in &other.atoms {
            if let Some(left) = merged.get(atom_id) {
                let value = left.merge_state(right)?;
                merged.insert(*atom_id, value);
            } else {
                merged.insert(*atom_id, right.clone());
            }
        }

        Ok(Self { atoms: merged })
    }
}

/// Minimal continuum shell for the real core.
///
/// The atom payload type remains generic in this first implementation slice so
/// the shell can enforce stable continuum/atom identity and merge composition
/// without prematurely freezing lifecycle, sequence or hierarchy structures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContinuumState<A> {
    continuum_id: ContinuumId,
    atoms: AtomMap<A>,
}

impl<A> ContinuumState<A> {
    pub fn new(continuum_id: ContinuumId) -> Self {
        Self {
            continuum_id,
            atoms: AtomMap::new(),
        }
    }

    pub const fn continuum_id(&self) -> ContinuumId {
        self.continuum_id
    }

    pub fn atoms(&self) -> &AtomMap<A> {
        &self.atoms
    }

    pub fn atoms_mut(&mut self) -> &mut AtomMap<A> {
        &mut self.atoms
    }
}

impl<A: Clone + MergeState> MergeState for ContinuumState<A> {
    fn merge_state(&self, other: &Self) -> Result<Self, CoreError> {
        if self.continuum_id != other.continuum_id {
            return Err(CoreError::ContinuumMismatch {
                left: self.continuum_id,
                right: other.continuum_id,
            });
        }

        Ok(Self {
            continuum_id: self.continuum_id,
            atoms: self.atoms.merge_state(&other.atoms)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::id::LOGICAL_ID_BYTES;
    use crate::{RevisionId, ScalarRegister, ScalarRevision};

    fn id_bytes(value: u64) -> [u8; LOGICAL_ID_BYTES] {
        let mut bytes = [0_u8; LOGICAL_ID_BYTES];
        bytes[LOGICAL_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn cid(value: u64) -> ContinuumId {
        ContinuumId::from_bytes(id_bytes(value))
    }

    fn aid(value: u64) -> AtomId {
        AtomId::from_bytes(id_bytes(value))
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(id_bytes(value))
    }

    fn scalar(id: u64, value: &'static str) -> ScalarRegister<&'static str> {
        ScalarRegister::from_revisions([ScalarRevision::new(rid(id), value, BTreeSet::new())])
            .unwrap()
    }

    #[test]
    fn merging_distinct_atoms_preserves_both() {
        let mut left = ContinuumState::new(cid(1));
        left.atoms_mut()
            .insert_new(aid(10), scalar(10, "left"))
            .unwrap();

        let mut right = ContinuumState::new(cid(1));
        right
            .atoms_mut()
            .insert_new(aid(20), scalar(20, "right"))
            .unwrap();

        let merged = left.merge_state(&right).unwrap();
        assert_eq!(merged.atoms().len(), 2);
        assert_eq!(
            merged.atoms().get(aid(10)).unwrap().materialized(),
            Some(&"left")
        );
        assert_eq!(
            merged.atoms().get(aid(20)).unwrap().materialized(),
            Some(&"right")
        );
    }

    #[test]
    fn shared_atom_delegates_to_its_merge_domain() {
        let mut base = ScalarRegister::new();
        base.assign(rid(1), "base").unwrap();

        let mut left_value = base.clone();
        left_value.assign(rid(200), "left").unwrap();
        let mut right_value = base;
        right_value.assign(rid(100), "right").unwrap();

        let mut left = ContinuumState::new(cid(1));
        left.atoms_mut().insert_new(aid(10), left_value).unwrap();
        let mut right = ContinuumState::new(cid(1));
        right.atoms_mut().insert_new(aid(10), right_value).unwrap();

        let merged = left.merge_state(&right).unwrap();
        assert_eq!(
            merged.atoms().get(aid(10)).unwrap().materialized(),
            Some(&"left")
        );
    }

    #[test]
    fn different_continua_cannot_merge() {
        let left: ContinuumState<ScalarRegister<&str>> = ContinuumState::new(cid(1));
        let right: ContinuumState<ScalarRegister<&str>> = ContinuumState::new(cid(2));

        assert_eq!(
            left.merge_state(&right).unwrap_err(),
            CoreError::ContinuumMismatch {
                left: cid(1),
                right: cid(2),
            }
        );
    }

    #[test]
    fn duplicate_local_atom_identity_is_rejected() {
        let mut state = ContinuumState::new(cid(1));
        state
            .atoms_mut()
            .insert_new(aid(10), scalar(1, "a"))
            .unwrap();

        assert_eq!(
            state
                .atoms_mut()
                .insert_new(aid(10), scalar(2, "b"))
                .unwrap_err(),
            CoreError::DuplicateAtomId { atom_id: aid(10) }
        );
    }

    #[test]
    fn continuum_merge_inherits_state_merge_laws_for_sample_states() {
        let mut a = ContinuumState::new(cid(1));
        a.atoms_mut().insert_new(aid(10), scalar(10, "a")).unwrap();
        let mut b = ContinuumState::new(cid(1));
        b.atoms_mut().insert_new(aid(20), scalar(20, "b")).unwrap();
        let mut c = ContinuumState::new(cid(1));
        c.atoms_mut().insert_new(aid(30), scalar(30, "c")).unwrap();

        assert_eq!(a.merge_state(&b).unwrap(), b.merge_state(&a).unwrap());
        assert_eq!(a.merge_state(&a).unwrap(), a);

        let left_grouped = a.merge_state(&b).unwrap().merge_state(&c).unwrap();
        let right_grouped = a.merge_state(&b.merge_state(&c).unwrap()).unwrap();
        assert_eq!(left_grouped, right_grouped);
    }
}
