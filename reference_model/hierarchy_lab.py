"""Hierarchy/containment experiments for A.P.C.

This lab treats container membership as another stable-identity location domain:
one AtomId has one causal parent-location register instead of being independently
removed from one container and inserted into another.

The model is intentionally incomplete.  In particular, active parent cycles are
rejected so adversarial tests can expose the need for a deterministic cycle
policy before hierarchical moves become normative.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, Optional

from reference_model.apc_model import ModelError, ScalarRegister, canonical_id


@dataclass
class HierarchyLab:
    parents: Dict[str, ScalarRegister] = field(default_factory=dict)
    deleted_atoms: frozenset[str] = frozenset()

    def copy(self) -> "HierarchyLab":
        return HierarchyLab(
            parents={atom_id: register.copy() for atom_id, register in self.parents.items()},
            deleted_atoms=frozenset(self.deleted_atoms),
        )

    def place(
        self,
        *,
        atom_id: str,
        revision_id: str,
        parent_atom_id: Optional[str],
    ) -> "HierarchyLab":
        canonical_id(atom_id)
        canonical_id(revision_id)
        if parent_atom_id is not None:
            canonical_id(parent_atom_id)
            if parent_atom_id == atom_id:
                raise ModelError("atom cannot be its own parent")
            if parent_atom_id not in self.parents:
                raise ModelError("unknown parent atom")
            if parent_atom_id in self.deleted_atoms:
                raise ModelError("cannot place under a parent deleted on this replica")
        if atom_id in self.deleted_atoms:
            raise ModelError("ordinary placement cannot resurrect a deleted atom")

        result = self.copy()
        register = result.parents.get(atom_id, ScalarRegister())
        result.parents[atom_id] = register.assign(revision_id, parent_atom_id)
        return result

    def delete(self, atom_id: str) -> "HierarchyLab":
        if atom_id not in self.parents:
            raise ModelError("cannot delete unknown atom")
        return HierarchyLab(
            parents={key: value.copy() for key, value in self.parents.items()},
            deleted_atoms=self.deleted_atoms | {atom_id},
        )

    def merge(self, other: "HierarchyLab") -> "HierarchyLab":
        merged: Dict[str, ScalarRegister] = {}
        for atom_id in set(self.parents) | set(other.parents):
            if atom_id in self.parents and atom_id in other.parents:
                merged[atom_id] = self.parents[atom_id].merge(other.parents[atom_id])
            else:
                merged[atom_id] = (self.parents.get(atom_id) or other.parents[atom_id]).copy()
        return HierarchyLab(
            parents=merged,
            deleted_atoms=self.deleted_atoms | other.deleted_atoms,
        )

    def active_parent(self, atom_id: str) -> Optional[str]:
        register = self.parents.get(atom_id)
        if register is None:
            raise ModelError("unknown atom")
        parent = register.materialized_value()
        if parent is not None and parent not in self.parents:
            raise ModelError("active parent references an unknown atom")
        return parent

    def active_parent_revision_id(self, atom_id: str) -> Optional[str]:
        register = self.parents.get(atom_id)
        if register is None:
            raise ModelError("unknown atom")
        revision = register.materialized_revision()
        return None if revision is None else revision.revision_id

    def _visibility(self, atom_id: str, visiting: set[str], memo: Dict[str, bool]) -> bool:
        if atom_id in memo:
            return memo[atom_id]
        if atom_id in visiting:
            raise ModelError("active hierarchy contains a parent cycle")
        if atom_id in self.deleted_atoms:
            memo[atom_id] = False
            return False

        visiting.add(atom_id)
        parent = self.active_parent(atom_id)
        if parent is None:
            visible = True
        else:
            visible = self._visibility(parent, visiting, memo)
        visiting.remove(atom_id)
        memo[atom_id] = visible
        return visible

    def visible_atoms(self) -> set[str]:
        memo: Dict[str, bool] = {}
        return {
            atom_id
            for atom_id in self.parents
            if self._visibility(atom_id, set(), memo)
        }

    def hidden_by_ancestor(self) -> set[str]:
        visible = self.visible_atoms()
        return {
            atom_id
            for atom_id in self.parents
            if atom_id not in visible and atom_id not in self.deleted_atoms
        }
