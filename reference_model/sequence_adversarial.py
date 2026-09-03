"""Adversarial extensions for A.P.C. ordered-sequence research.

This module deliberately sits outside the primary sequence candidate. It adds
measurement helpers and alternative lifecycle semantics used to falsify or
stress ideas before they are promoted into the logical model.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional

from reference_model.apc_model import ModelError
from reference_model.sequence_lab import MovableSequenceLab


@dataclass(frozen=True)
class SequenceMetrics:
    position_count: int
    visible_atom_count: int
    hidden_atom_count: int
    location_revision_count: int
    causal_reference_count: int
    dead_position_count: int
    max_position_depth: int


def sequence_metrics(sequence: MovableSequenceLab) -> SequenceMetrics:
    """Measure logical metadata growth in the current reference candidate.

    Counts are representation-level research metrics, not serialized byte-size
    claims. In particular, `causal_reference_count` measures explicit ancestor
    IDs retained by the deliberately simple ScalarRegister reference model.
    """

    active_positions: set[str] = set()
    visible = 0
    hidden = 0
    revision_count = 0
    causal_reference_count = 0

    for register in sequence.locations.values():
        revision_count += len(register.revisions)
        causal_reference_count += sum(
            len(revision.context) for revision in register.revisions.values()
        )
        position_id = register.materialized_value()
        if position_id is None:
            hidden += 1
        else:
            visible += 1
            active_positions.add(position_id)

    return SequenceMetrics(
        position_count=len(sequence.tree.positions),
        visible_atom_count=visible,
        hidden_atom_count=hidden,
        location_revision_count=revision_count,
        causal_reference_count=causal_reference_count,
        dead_position_count=len(sequence.tree.positions) - len(active_positions),
        max_position_depth=sequence.tree.max_depth(),
    )


@dataclass
class DeleteWinsSequenceLab:
    """Alternative lifecycle experiment with monotonic delete-wins semantics.

    The underlying location register is left untouched by deletion. Visibility
    is instead filtered by a grow-only set of deleted AtomIds. This is useful to
    test the architectural separation of `where is the atom?` from `is the atom
    logically alive?`.

    It is intentionally not normative: a grow-only tombstone cannot express an
    explicit restore of the same AtomId, and safe compaction is unresolved.
    """

    sequence: MovableSequenceLab = field(default_factory=MovableSequenceLab)
    deleted_atoms: frozenset[str] = frozenset()

    def copy(self) -> "DeleteWinsSequenceLab":
        return DeleteWinsSequenceLab(
            sequence=self.sequence.copy(),
            deleted_atoms=frozenset(self.deleted_atoms),
        )

    def place(
        self,
        *,
        atom_id: str,
        position_id: str,
        revision_id: str,
        left_atom_id: Optional[str] = None,
        right_atom_id: Optional[str] = None,
    ) -> "DeleteWinsSequenceLab":
        if atom_id in self.deleted_atoms:
            raise ModelError("ordinary move cannot resurrect a deleted atom")
        for neighbor in (left_atom_id, right_atom_id):
            if neighbor is not None and neighbor in self.deleted_atoms:
                raise ModelError("cannot anchor to an atom deleted on this replica")

        return DeleteWinsSequenceLab(
            sequence=self.sequence.place(
                atom_id=atom_id,
                position_id=position_id,
                revision_id=revision_id,
                left_atom_id=left_atom_id,
                right_atom_id=right_atom_id,
            ),
            deleted_atoms=self.deleted_atoms,
        )

    def delete(self, *, atom_id: str) -> "DeleteWinsSequenceLab":
        if atom_id not in self.sequence.locations:
            raise ModelError("cannot delete unknown atom")
        return DeleteWinsSequenceLab(
            sequence=self.sequence.copy(),
            deleted_atoms=self.deleted_atoms | {atom_id},
        )

    def merge(self, other: "DeleteWinsSequenceLab") -> "DeleteWinsSequenceLab":
        return DeleteWinsSequenceLab(
            sequence=self.sequence.merge(other.sequence),
            deleted_atoms=self.deleted_atoms | other.deleted_atoms,
        )

    def materialize(self) -> list[str]:
        return [
            atom_id
            for atom_id in self.sequence.materialize()
            if atom_id not in self.deleted_atoms
        ]
