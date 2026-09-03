"""Cross-domain atom interaction experiments for A.P.C.

This lab composes three ideas already explored independently:

- stable AtomId;
- delete-wins lifecycle separate from sequence location;
- durable working content with domain-local causal observation.

It asks whether common user operations that look multi-domain actually require a
strong atomic transaction.

This is research code, not production format semantics.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, Optional

from reference_model.apc_model import ModelError
from reference_model.causality_lab import FrontierCausalRegister
from reference_model.sequence_adversarial import DeleteWinsSequenceLab
from reference_model.working_state_lab import WorkingScalar


@dataclass
class AtomInteractionLab:
    structure: DeleteWinsSequenceLab = field(default_factory=DeleteWinsSequenceLab)
    content: Dict[str, WorkingScalar] = field(default_factory=dict)

    def copy(self) -> "AtomInteractionLab":
        return AtomInteractionLab(
            structure=self.structure.copy(),
            content={atom_id: state.copy() for atom_id, state in self.content.items()},
        )

    def add_atom(
        self,
        *,
        atom_id: str,
        position_id: str,
        location_revision_id: str,
        content_revision_id: str,
        content_value: Any,
        left_atom_id: Optional[str] = None,
        right_atom_id: Optional[str] = None,
    ) -> None:
        if atom_id in self.content:
            raise ModelError("atom already exists in interaction lab")
        self.structure = self.structure.place(
            atom_id=atom_id,
            position_id=position_id,
            revision_id=location_revision_id,
            left_atom_id=left_atom_id,
            right_atom_id=right_atom_id,
        )
        causal = FrontierCausalRegister().assign(content_revision_id, content_value)
        self.content[atom_id] = WorkingScalar.from_causal(causal)

    def durable_edit_content(self, atom_id: str, value: Any) -> None:
        if atom_id not in self.content:
            raise ModelError("unknown atom content")
        if atom_id in self.structure.deleted_atoms:
            raise ModelError("cannot edit an atom deleted on this replica")
        self.content[atom_id].durable_edit(value)

    def seal_content(self, atom_id: str, revision_id: str) -> None:
        if atom_id not in self.content:
            raise ModelError("unknown atom content")
        self.content[atom_id].seal(revision_id)

    def move_atom(
        self,
        *,
        atom_id: str,
        position_id: str,
        location_revision_id: str,
        left_atom_id: Optional[str] = None,
        right_atom_id: Optional[str] = None,
    ) -> None:
        self.structure = self.structure.place(
            atom_id=atom_id,
            position_id=position_id,
            revision_id=location_revision_id,
            left_atom_id=left_atom_id,
            right_atom_id=right_atom_id,
        )

    def delete_atom(self, atom_id: str) -> None:
        self.structure = self.structure.delete(atom_id=atom_id)

    def observe_remote_structure(self, remote: DeleteWinsSequenceLab) -> None:
        """Merge location/lifecycle without manufacturing content causality."""

        self.structure = self.structure.merge(remote)

    def observe_remote_content(
        self,
        atom_id: str,
        remote: FrontierCausalRegister,
        *,
        pre_observation_revision_id: Optional[str] = None,
    ) -> None:
        state = self.content.get(atom_id)
        if state is None:
            raise ModelError("unknown atom content")
        state.observe_remote(
            remote,
            pre_observation_revision_id=pre_observation_revision_id,
        )

    def visible_atoms(self) -> list[str]:
        return self.structure.materialize()

    def visible_content(self) -> Dict[str, Any]:
        visible = set(self.visible_atoms())
        return {
            atom_id: state.working_value
            for atom_id, state in self.content.items()
            if atom_id in visible
        }

    def retained_content_value(self, atom_id: str) -> Any:
        state = self.content.get(atom_id)
        if state is None:
            raise ModelError("unknown atom content")
        return state.working_value
