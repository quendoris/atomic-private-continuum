"""Exposure-aware private causal squashing experiments for A.P.C.

This module asks whether locally created causal nodes that have never crossed an
external boundary can be removed after a later local revision dominates them.

The model is logical only.  It deliberately rewrites parent sets while retaining
opaque revision IDs so equivalence can be tested against the existing oracle.
A production design must account for signatures/content commitments and may need
provisional local epoch IDs until a revision is externally finalized.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import FrozenSet, Iterable

from reference_model.apc_model import ModelError
from reference_model.causality_lab import FrontierCausalRegister, FrontierRevision


@dataclass(frozen=True)
class SquashResult:
    state: "ExposureAwareCausalState"
    removed_ids: FrozenSet[str]


@dataclass
class ExposureAwareCausalState:
    causal: FrontierCausalRegister
    private_local_ids: set[str] = field(default_factory=set)
    exposed_ids: set[str] = field(default_factory=set)

    def __post_init__(self) -> None:
        self.causal.validate()
        known = set(self.causal.revisions)
        if not self.private_local_ids <= known:
            raise ModelError("private causal set contains unknown revision IDs")
        if not self.exposed_ids <= known:
            raise ModelError("exposed causal set contains unknown revision IDs")

    def copy(self) -> "ExposureAwareCausalState":
        return ExposureAwareCausalState(
            causal=self.causal.copy(),
            private_local_ids=set(self.private_local_ids),
            exposed_ids=set(self.exposed_ids),
        )

    def _ancestor_closure(self, revision_ids: Iterable[str]) -> set[str]:
        pending = list(revision_ids)
        closure: set[str] = set()
        while pending:
            revision_id = pending.pop()
            if revision_id in closure:
                continue
            revision = self.causal.revisions.get(revision_id)
            if revision is None:
                raise ModelError("cannot expose unknown causal revision")
            closure.add(revision_id)
            pending.extend(revision.parents)
        return closure

    def mark_exposed(self, revision_ids: Iterable[str]) -> None:
        """Mark revisions handed outside the replica and every named ancestor.

        Exposure happens at transport handoff, not acknowledgement.  Once bytes
        may have left the process, acknowledgement loss cannot prove that another
        replica did not receive them.
        """

        closure = self._ancestor_closure(revision_ids)
        self.exposed_ids.update(closure)
        self.private_local_ids.difference_update(closure)

    def squash_unexposed_dominated(self) -> SquashResult:
        """Bypass private unexposed non-frontier nodes and remove them.

        The transformation preserves the currently retained logical frontier and
        causal reachability among all non-removed revisions.  It is intentionally
        conservative: exposed IDs and frontier IDs are never removed.
        """

        frontier_ids = set(self.causal.frontier())
        removable = (
            set(self.private_local_ids)
            - set(self.exposed_ids)
            - frontier_ids
        )
        if not removable:
            return SquashResult(self.copy(), frozenset())

        original = self.causal

        def boundary_parents(parent_ids: Iterable[str]) -> set[str]:
            pending = list(parent_ids)
            boundary: set[str] = set()
            seen: set[str] = set()
            while pending:
                parent = pending.pop()
                if parent in seen:
                    continue
                seen.add(parent)
                if parent in removable:
                    pending.extend(original.revisions[parent].parents)
                else:
                    boundary.add(parent)

            # Keep only causally maximal boundary parents.  Recursive bypass can
            # reveal both an ancestor and one of its retained descendants.
            reduced = set(boundary)
            for candidate in boundary:
                for other in boundary:
                    if candidate == other:
                        continue
                    if original.is_ancestor(candidate, other):
                        reduced.discard(candidate)
                        break
            return reduced

        rewritten: dict[str, FrontierRevision] = {}
        for revision_id, revision in original.revisions.items():
            if revision_id in removable:
                continue
            new_parents = frozenset(boundary_parents(revision.parents))
            rewritten[revision_id] = FrontierRevision(
                revision_id=revision.revision_id,
                value=revision.value,
                parents=new_parents,
            )

        compact = FrontierCausalRegister(rewritten)
        compact.validate()

        result = ExposureAwareCausalState(
            causal=compact,
            private_local_ids=set(self.private_local_ids) - removable,
            exposed_ids=set(self.exposed_ids),
        )
        return SquashResult(result, frozenset(removable))
