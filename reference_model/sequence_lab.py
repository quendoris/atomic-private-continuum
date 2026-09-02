"""Experimental ordered-sequence models for A.P.C.

This module is a research harness, not portable format semantics.
It exists to falsify sequence candidates before any implementation is
promoted into docs/LOGIC.md as normative structure.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from functools import cmp_to_key
from typing import Dict, Optional

from reference_model.apc_model import ModelError, ScalarRegister, canonical_id


@dataclass(frozen=True)
class FuguePosition:
    position_id: str
    parent_position_id: Optional[str]
    left_child: bool

    def __post_init__(self) -> None:
        canonical_id(self.position_id)
        if self.parent_position_id is not None:
            canonical_id(self.parent_position_id)
        if self.parent_position_id == self.position_id:
            raise ModelError("position cannot be its own parent")


@dataclass
class FuguePositionTree:
    """Small state-based Fugue-style position tree.

    The tree follows Fugue's parent + left/right-side representation, but uses
    opaque canonical IDs as the sibling tie-break in this experiment. It does
    not claim the exact metadata encoding or proof obligations of Fugue/FugueMax.
    """

    positions: Dict[str, FuguePosition] = field(default_factory=dict)

    def copy(self) -> "FuguePositionTree":
        return FuguePositionTree(dict(self.positions))

    def merge(self, other: "FuguePositionTree") -> "FuguePositionTree":
        merged = dict(self.positions)
        for position_id, position in other.positions.items():
            previous = merged.get(position_id)
            if previous is not None and previous != position:
                raise ModelError("position ID collision with different content")
            merged[position_id] = position
        result = FuguePositionTree(merged)
        result.validate()
        return result

    def validate(self) -> None:
        for position in self.positions.values():
            if (
                position.parent_position_id is not None
                and position.parent_position_id not in self.positions
            ):
                raise ModelError("position parent is missing")

        visiting: set[str] = set()
        visited: set[str] = set()

        def visit(position_id: str) -> None:
            if position_id in visited:
                return
            if position_id in visiting:
                raise ModelError("position tree contains a cycle")
            visiting.add(position_id)
            parent = self.positions[position_id].parent_position_id
            if parent is not None:
                visit(parent)
            visiting.remove(position_id)
            visited.add(position_id)

        for position_id in self.positions:
            visit(position_id)

    def _depth(self, position_id: str) -> int:
        depth = 1
        current = self.positions[position_id]
        while current.parent_position_id is not None:
            depth += 1
            current = self.positions[current.parent_position_id]
        return depth

    def _is_ancestor(self, ancestor_id: str, descendant_id: str) -> bool:
        current_id: Optional[str] = descendant_id
        while current_id is not None:
            if current_id == ancestor_id:
                return True
            current_id = self.positions[current_id].parent_position_id
        return False

    def create_between(
        self,
        position_id: str,
        left_position_id: Optional[str],
        right_position_id: Optional[str],
    ) -> "FuguePositionTree":
        canonical_id(position_id)
        if position_id in self.positions:
            raise ModelError("position ID already exists")

        for neighbor in (left_position_id, right_position_id):
            if neighbor is not None and neighbor not in self.positions:
                raise ModelError("unknown neighboring position")

        if (
            left_position_id is not None
            and right_position_id is not None
            and self.compare(left_position_id, right_position_id) >= 0
        ):
            raise ModelError("left position must sort before right position")

        left_is_ancestor = (
            right_position_id is not None
            and (
                left_position_id is None
                or self._is_ancestor(left_position_id, right_position_id)
            )
        )

        if left_is_ancestor:
            parent = right_position_id
            left_child = True
        else:
            parent = left_position_id
            left_child = False

        result = self.copy()
        result.positions[position_id] = FuguePosition(
            position_id=position_id,
            parent_position_id=parent,
            left_child=left_child,
        )
        result.validate()

        if left_position_id is not None:
            if result.compare(left_position_id, position_id) >= 0:
                raise ModelError("created position is not after left neighbor")
        if right_position_id is not None:
            if result.compare(position_id, right_position_id) >= 0:
                raise ModelError("created position is not before right neighbor")
        return result

    def compare(self, left_id: str, right_id: str) -> int:
        if left_id == right_id:
            return 0
        if left_id not in self.positions or right_id not in self.positions:
            raise ModelError("cannot compare unknown positions")

        a_id = left_id
        b_id = right_id
        a_depth = self._depth(a_id)
        b_depth = self._depth(b_id)
        last_move: Optional[tuple[str, bool]] = None

        while a_depth > b_depth:
            a = self.positions[a_id]
            last_move = ("a", a.left_child)
            if a.parent_position_id is None:
                raise ModelError("invalid position depth")
            a_id = a.parent_position_id
            a_depth -= 1

        while b_depth > a_depth:
            b = self.positions[b_id]
            last_move = ("b", b.left_child)
            if b.parent_position_id is None:
                raise ModelError("invalid position depth")
            b_id = b.parent_position_id
            b_depth -= 1

        if a_id == b_id:
            if last_move is None:
                raise ModelError("invalid ancestor comparison")
            descendant, is_left = last_move
            factor = 1 if descendant == "a" else -1
            return factor * (-1 if is_left else 1)

        while (
            self.positions[a_id].parent_position_id
            != self.positions[b_id].parent_position_id
        ):
            a_parent = self.positions[a_id].parent_position_id
            b_parent = self.positions[b_id].parent_position_id
            if a_parent is None or b_parent is None:
                raise ModelError("position roots do not share a common root")
            a_id = a_parent
            b_id = b_parent

        a = self.positions[a_id]
        b = self.positions[b_id]
        if a.left_child != b.left_child:
            return -1 if a.left_child else 1

        a_key = canonical_id(a.position_id)
        b_key = canonical_id(b.position_id)
        return -1 if a_key < b_key else 1

    def sorted_positions(self, position_ids: list[str]) -> list[str]:
        if len(position_ids) != len(set(position_ids)):
            raise ModelError("duplicate active position")
        return sorted(position_ids, key=cmp_to_key(self.compare))

    def max_depth(self) -> int:
        if not self.positions:
            return 0
        return max(self._depth(position_id) for position_id in self.positions)


@dataclass
class MovableSequenceLab:
    """Experimental composition: stable positions + causal location register.

    Each atom has identity independent from position. A move allocates a new
    immutable position and updates the atom's location register. Concurrent
    moves therefore select one visible location instead of duplicating the atom.

    Delete is represented experimentally as assigning None. Delete-vs-move/edit
    concurrency policy is not normative yet.
    """

    tree: FuguePositionTree = field(default_factory=FuguePositionTree)
    locations: Dict[str, ScalarRegister] = field(default_factory=dict)

    def copy(self) -> "MovableSequenceLab":
        return MovableSequenceLab(
            tree=self.tree.copy(),
            locations={
                atom_id: register.copy()
                for atom_id, register in self.locations.items()
            },
        )

    def _active_position(self, atom_id: str) -> Optional[str]:
        register = self.locations.get(atom_id)
        if register is None:
            return None
        value = register.materialized_value()
        if value is not None and value not in self.tree.positions:
            raise ModelError("active location references an unknown position")
        return value

    def _neighbor_position(self, atom_id: Optional[str]) -> Optional[str]:
        if atom_id is None:
            return None
        if atom_id not in self.locations:
            raise ModelError("unknown neighboring atom")
        position_id = self._active_position(atom_id)
        if position_id is None:
            raise ModelError("neighboring atom is not visible")
        return position_id

    def place(
        self,
        *,
        atom_id: str,
        position_id: str,
        revision_id: str,
        left_atom_id: Optional[str] = None,
        right_atom_id: Optional[str] = None,
    ) -> "MovableSequenceLab":
        canonical_id(atom_id)
        canonical_id(revision_id)
        result = self.copy()

        left_position = result._neighbor_position(left_atom_id)
        right_position = result._neighbor_position(right_atom_id)
        result.tree = result.tree.create_between(
            position_id,
            left_position,
            right_position,
        )

        register = result.locations.get(atom_id, ScalarRegister())
        result.locations[atom_id] = register.assign(revision_id, position_id)
        return result

    def delete(self, *, atom_id: str, revision_id: str) -> "MovableSequenceLab":
        if atom_id not in self.locations:
            raise ModelError("cannot delete unknown atom")
        result = self.copy()
        result.locations[atom_id] = result.locations[atom_id].assign(
            revision_id,
            None,
        )
        return result

    def merge(self, other: "MovableSequenceLab") -> "MovableSequenceLab":
        tree = self.tree.merge(other.tree)
        locations: Dict[str, ScalarRegister] = {}
        for atom_id in set(self.locations) | set(other.locations):
            if atom_id in self.locations and atom_id in other.locations:
                locations[atom_id] = self.locations[atom_id].merge(
                    other.locations[atom_id]
                )
            else:
                locations[atom_id] = (
                    self.locations.get(atom_id) or other.locations[atom_id]
                ).copy()
        result = MovableSequenceLab(tree=tree, locations=locations)
        result.materialize()
        return result

    def materialize(self) -> list[str]:
        visible: list[tuple[str, str]] = []
        active_positions: set[str] = set()
        for atom_id, register in self.locations.items():
            position_id = register.materialized_value()
            if position_id is None:
                continue
            if position_id not in self.tree.positions:
                raise ModelError("location references unknown position")
            if position_id in active_positions:
                raise ModelError("two atoms materialize at one position")
            active_positions.add(position_id)
            visible.append((atom_id, position_id))

        ordered_positions = self.tree.sorted_positions(
            [position_id for _, position_id in visible]
        )
        atom_by_position = {
            position_id: atom_id for atom_id, position_id in visible
        }
        return [atom_by_position[position_id] for position_id in ordered_positions]

    def active_location_revision(self, atom_id: str):
        register = self.locations.get(atom_id)
        return None if register is None else register.materialized_revision()
