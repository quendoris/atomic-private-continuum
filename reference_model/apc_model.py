"""Executable reference model for A.P.C. logical merge semantics.

This module is deliberately small and explicit. It is not a production core,
not a binary-format implementation and not a cryptographic implementation.
Its purpose is to make the current logical rules executable and testable.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, FrozenSet, Optional
import copy
import heapq


class ModelError(ValueError):
    """Raised when a logical state violates reference-model invariants."""


def canonical_id(value: str) -> bytes:
    """Return canonical bytes for a lowercase/uppercase hexadecimal ID.

    The reference model uses hex strings only for readability in tests. The
    production encoding is intentionally not standardized here.
    """

    text = value.strip()
    if not text:
        raise ModelError("empty identifier")
    if len(text) % 2:
        text = "0" + text
    try:
        return bytes.fromhex(text)
    except ValueError as exc:
        raise ModelError(f"invalid hexadecimal identifier: {value!r}") from exc


@dataclass(frozen=True)
class ScalarRevision:
    revision_id: str
    value: Any
    context: FrozenSet[str] = frozenset()

    def __post_init__(self) -> None:
        canonical_id(self.revision_id)
        if self.revision_id in self.context:
            raise ModelError("a revision cannot causally observe itself")
        for observed in self.context:
            canonical_id(observed)


@dataclass
class ScalarRegister:
    """Reference multi-value causal register with deterministic materialization.

    The model keeps explicit ancestor revision IDs. That representation is
    intentionally simple and intentionally not claimed to be production-scale.
    """

    revisions: Dict[str, ScalarRevision] = field(default_factory=dict)

    def copy(self) -> "ScalarRegister":
        return ScalarRegister(dict(self.revisions))

    def observed_context(self) -> FrozenSet[str]:
        observed: set[str] = set()
        for revision in self.revisions.values():
            observed.add(revision.revision_id)
            observed.update(revision.context)
        return frozenset(observed)

    def assign(self, revision_id: str, value: Any) -> "ScalarRegister":
        revision = ScalarRevision(
            revision_id=revision_id,
            value=value,
            context=self.observed_context(),
        )
        result = self.copy()
        previous = result.revisions.get(revision_id)
        if previous is not None and previous != revision:
            raise ModelError("revision ID collision with different content")
        result.revisions[revision_id] = revision
        return result

    def merge(self, other: "ScalarRegister") -> "ScalarRegister":
        merged = dict(self.revisions)
        for revision_id, revision in other.revisions.items():
            previous = merged.get(revision_id)
            if previous is not None and previous != revision:
                raise ModelError("revision ID collision with different content")
            merged[revision_id] = revision
        return ScalarRegister(merged)

    def frontier(self) -> Dict[str, ScalarRevision]:
        """Return revisions not causally superseded by another known revision."""

        dominated: set[str] = set()
        for revision in self.revisions.values():
            dominated.update(revision.context)
        return {
            revision_id: revision
            for revision_id, revision in self.revisions.items()
            if revision_id not in dominated
        }

    def materialized_revision(self) -> Optional[ScalarRevision]:
        frontier = self.frontier()
        if not frontier:
            return None
        return max(
            frontier.values(),
            key=lambda revision: canonical_id(revision.revision_id),
        )

    def materialized_value(self) -> Any:
        revision = self.materialized_revision()
        return None if revision is None else revision.value


@dataclass(frozen=True)
class Placement:
    """Insertion-only placement constraints for the reference sequence model."""

    placement_id: str
    atom_id: str
    left_atom_id: Optional[str] = None
    right_atom_id: Optional[str] = None

    def __post_init__(self) -> None:
        canonical_id(self.placement_id)
        canonical_id(self.atom_id)
        if self.left_atom_id is not None:
            canonical_id(self.left_atom_id)
        if self.right_atom_id is not None:
            canonical_id(self.right_atom_id)
        if self.left_atom_id == self.atom_id or self.right_atom_id == self.atom_id:
            raise ModelError("a placement cannot anchor to itself")
        if (
            self.left_atom_id is not None
            and self.left_atom_id == self.right_atom_id
        ):
            raise ModelError("left and right anchors cannot be equal")


@dataclass
class OrderedCollection:
    """Insertion-only ordered collection used to test convergence.

    A placement carries relations to its immediate local left/right anchors.
    Merging is set union by placement ID. Materialization computes a
    deterministic topological order of the accumulated constraints.

    Move and delete semantics are intentionally absent from this prototype.
    """

    placements: Dict[str, Placement] = field(default_factory=dict)

    def copy(self) -> "OrderedCollection":
        return OrderedCollection(dict(self.placements))

    def insert(self, placement: Placement) -> "OrderedCollection":
        result = self.copy()
        previous = result.placements.get(placement.placement_id)
        if previous is not None and previous != placement:
            raise ModelError("placement ID collision with different content")

        for existing in result.placements.values():
            if existing.atom_id == placement.atom_id and existing != placement:
                raise ModelError(
                    "multiple placements for one atom are not supported "
                    "before move semantics are defined"
                )

        atom_ids = {existing.atom_id for existing in result.placements.values()}
        for anchor in (placement.left_atom_id, placement.right_atom_id):
            if anchor is not None and anchor not in atom_ids:
                raise ModelError(f"missing local placement anchor: {anchor}")

        result.placements[placement.placement_id] = placement
        return result

    def merge(self, other: "OrderedCollection") -> "OrderedCollection":
        merged = dict(self.placements)
        for placement_id, placement in other.placements.items():
            previous = merged.get(placement_id)
            if previous is not None and previous != placement:
                raise ModelError("placement ID collision with different content")
            merged[placement_id] = placement

        by_atom: Dict[str, Placement] = {}
        for placement in merged.values():
            previous = by_atom.get(placement.atom_id)
            if previous is not None and previous != placement:
                raise ModelError(
                    "same atom has multiple placements; move semantics are open"
                )
            by_atom[placement.atom_id] = placement

        return OrderedCollection(merged)

    def materialize(self) -> list[str]:
        by_atom = {placement.atom_id: placement for placement in self.placements.values()}
        nodes = set(by_atom)
        outgoing = {node: set() for node in nodes}
        indegree = {node: 0 for node in nodes}

        def add_edge(left: str, right: str) -> None:
            if left == right:
                raise ModelError("self ordering edge")
            if right not in outgoing[left]:
                outgoing[left].add(right)
                indegree[right] += 1

        for placement in self.placements.values():
            if placement.left_atom_id is not None:
                if placement.left_atom_id not in nodes:
                    raise ModelError("left anchor missing after merge")
                add_edge(placement.left_atom_id, placement.atom_id)
            if placement.right_atom_id is not None:
                if placement.right_atom_id not in nodes:
                    raise ModelError("right anchor missing after merge")
                add_edge(placement.atom_id, placement.right_atom_id)

        priority = {
            placement.atom_id: canonical_id(placement.placement_id)
            for placement in self.placements.values()
        }

        ready = [
            (priority[node], canonical_id(node), node)
            for node in nodes
            if indegree[node] == 0
        ]
        heapq.heapify(ready)

        result: list[str] = []
        while ready:
            _, _, node = heapq.heappop(ready)
            result.append(node)
            for successor in sorted(
                outgoing[node],
                key=lambda item: (priority[item], canonical_id(item)),
            ):
                indegree[successor] -= 1
                if indegree[successor] == 0:
                    heapq.heappush(
                        ready,
                        (priority[successor], canonical_id(successor), successor),
                    )

        if len(result) != len(nodes):
            raise ModelError("ordering constraints contain a cycle")
        return result


@dataclass
class Atom:
    atom_id: str
    type_id: str
    scalars: Dict[str, ScalarRegister] = field(default_factory=dict)
    collections: Dict[str, OrderedCollection] = field(default_factory=dict)

    def __post_init__(self) -> None:
        canonical_id(self.atom_id)

    def merge(self, other: "Atom") -> "Atom":
        if self.atom_id != other.atom_id:
            raise ModelError("atom ID mismatch")
        if self.type_id != other.type_id:
            raise ModelError("atom type mismatch")

        scalars: Dict[str, ScalarRegister] = {}
        for name in set(self.scalars) | set(other.scalars):
            if name in self.scalars and name in other.scalars:
                scalars[name] = self.scalars[name].merge(other.scalars[name])
            else:
                scalars[name] = (self.scalars.get(name) or other.scalars[name]).copy()

        collections: Dict[str, OrderedCollection] = {}
        for name in set(self.collections) | set(other.collections):
            if name in self.collections and name in other.collections:
                collections[name] = self.collections[name].merge(other.collections[name])
            else:
                collections[name] = (
                    self.collections.get(name) or other.collections[name]
                ).copy()

        return Atom(
            atom_id=self.atom_id,
            type_id=self.type_id,
            scalars=scalars,
            collections=collections,
        )


@dataclass
class ContinuumState:
    continuum_id: str
    atoms: Dict[str, Atom] = field(default_factory=dict)

    def __post_init__(self) -> None:
        canonical_id(self.continuum_id)

    def merge(self, other: "ContinuumState") -> "ContinuumState":
        if self.continuum_id != other.continuum_id:
            raise ModelError("continuum mismatch")

        atoms: Dict[str, Atom] = {}
        for atom_id in set(self.atoms) | set(other.atoms):
            if atom_id in self.atoms and atom_id in other.atoms:
                atoms[atom_id] = self.atoms[atom_id].merge(other.atoms[atom_id])
            else:
                atoms[atom_id] = copy.deepcopy(
                    self.atoms.get(atom_id) or other.atoms[atom_id]
                )

        return ContinuumState(continuum_id=self.continuum_id, atoms=atoms)


def logical_snapshot(state: ContinuumState) -> dict[str, Any]:
    """Canonical test-only projection for logical-state equality."""

    return {
        "continuum_id": state.continuum_id,
        "atoms": {
            atom_id: {
                "type": atom.type_id,
                "scalars": {
                    name: {
                        "value": register.materialized_value(),
                        "frontier": tuple(sorted(register.frontier())),
                    }
                    for name, register in sorted(atom.scalars.items())
                },
                "collections": {
                    name: tuple(collection.materialize())
                    for name, collection in sorted(atom.collections.items())
                },
            }
            for atom_id, atom in sorted(state.atoms.items())
        },
    }
