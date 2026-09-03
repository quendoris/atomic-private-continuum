"""ID-only compact causality experiments for A.P.C.

The production format is not frozen here. This module compares a compact
direct-frontier DAG against the explicit-all-ancestors ScalarRegister oracle.

No wall clock, Lamport timestamp, per-replica sequence number, or server order
participates in causal correctness.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, FrozenSet, Iterable, Optional

from reference_model.apc_model import ModelError, canonical_id


@dataclass(frozen=True)
class FrontierRevision:
    revision_id: str
    value: Any
    parents: FrozenSet[str] = frozenset()

    def __post_init__(self) -> None:
        canonical_id(self.revision_id)
        if self.revision_id in self.parents:
            raise ModelError("a revision cannot parent itself")
        for parent in self.parents:
            canonical_id(parent)


@dataclass(frozen=True)
class CausalityMetrics:
    revision_count: int
    direct_reference_count: int
    frontier_width: int
    max_parent_count: int


@dataclass
class FrontierCausalRegister:
    """State-based causal register storing only the observed causal frontier.

    A newly assigned revision references the current frontier, not every known
    ancestor. The full direct-parent DAG is retained in this research model so
    reachability can be checked exactly.

    This is ID-only causality: IDs identify nodes; they do not encode time.
    """

    revisions: Dict[str, FrontierRevision] = field(default_factory=dict)

    def copy(self) -> "FrontierCausalRegister":
        return FrontierCausalRegister(dict(self.revisions))

    def validate(self) -> None:
        known_ids = set(self.revisions)
        for revision in self.revisions.values():
            missing = set(revision.parents) - known_ids
            if missing:
                raise ModelError(
                    f"causal parent(s) missing from complete register: {sorted(missing)!r}"
                )

        visiting: set[str] = set()
        visited: set[str] = set()

        def visit(revision_id: str) -> None:
            if revision_id in visited:
                return
            if revision_id in visiting:
                raise ModelError("causal graph contains a cycle")
            visiting.add(revision_id)
            for parent in self.revisions[revision_id].parents:
                visit(parent)
            visiting.remove(revision_id)
            visited.add(revision_id)

        for revision_id in self.revisions:
            visit(revision_id)

    def frontier(self) -> Dict[str, FrontierRevision]:
        referenced: set[str] = set()
        for revision in self.revisions.values():
            referenced.update(revision.parents)
        return {
            revision_id: revision
            for revision_id, revision in self.revisions.items()
            if revision_id not in referenced
        }

    def assign(self, revision_id: str, value: Any) -> "FrontierCausalRegister":
        revision = FrontierRevision(
            revision_id=revision_id,
            value=value,
            parents=frozenset(self.frontier()),
        )
        result = self.copy()
        previous = result.revisions.get(revision_id)
        if previous is not None and previous != revision:
            raise ModelError("revision ID collision with different content")
        result.revisions[revision_id] = revision
        result.validate()
        return result

    def merge(self, other: "FrontierCausalRegister") -> "FrontierCausalRegister":
        merged = dict(self.revisions)
        for revision_id, revision in other.revisions.items():
            previous = merged.get(revision_id)
            if previous is not None and previous != revision:
                raise ModelError("revision ID collision with different content")
            merged[revision_id] = revision
        result = FrontierCausalRegister(merged)
        result.validate()
        return result

    def is_ancestor(self, ancestor_id: str, descendant_id: str) -> bool:
        if ancestor_id == descendant_id:
            return False
        if ancestor_id not in self.revisions or descendant_id not in self.revisions:
            raise ModelError("cannot compare unknown causal revisions")

        pending = list(self.revisions[descendant_id].parents)
        seen: set[str] = set()
        while pending:
            current = pending.pop()
            if current == ancestor_id:
                return True
            if current in seen:
                continue
            seen.add(current)
            pending.extend(self.revisions[current].parents)
        return False

    def materialized_revision(self) -> Optional[FrontierRevision]:
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

    def metrics(self) -> CausalityMetrics:
        frontier = self.frontier()
        parent_counts = [len(revision.parents) for revision in self.revisions.values()]
        return CausalityMetrics(
            revision_count=len(self.revisions),
            direct_reference_count=sum(parent_counts),
            frontier_width=len(frontier),
            max_parent_count=max(parent_counts, default=0),
        )

    def missing_from(self, known_revision_ids: Iterable[str]) -> "FrontierDelta":
        """Return nodes absent from a receiver that already has a valid baseline.

        The delta may reference parent IDs not included in the delta when those
        parents are declared in ``known_revision_ids``. A receiver must not
        apply it unless every external parent is already present locally.
        """

        known = frozenset(known_revision_ids)
        unknown_known = known - set(self.revisions)
        if unknown_known:
            raise ModelError("known baseline contains revisions absent at sender")

        missing = {
            revision_id: revision
            for revision_id, revision in self.revisions.items()
            if revision_id not in known
        }
        return FrontierDelta(
            revisions=missing,
            external_parents=frozenset(
                parent
                for revision in missing.values()
                for parent in revision.parents
                if parent not in missing
            ),
        )

    def apply_delta(self, delta: "FrontierDelta") -> "FrontierCausalRegister":
        available = set(self.revisions) | set(delta.revisions)
        required = set(delta.external_parents)
        if not required <= set(self.revisions):
            raise ModelError("delta baseline dependency is missing")

        for revision in delta.revisions.values():
            if not set(revision.parents) <= available:
                raise ModelError("delta contains unresolved causal parent")

        merged = dict(self.revisions)
        for revision_id, revision in delta.revisions.items():
            previous = merged.get(revision_id)
            if previous is not None and previous != revision:
                raise ModelError("revision ID collision with different content")
            merged[revision_id] = revision

        result = FrontierCausalRegister(merged)
        result.validate()
        return result


@dataclass(frozen=True)
class FrontierDelta:
    revisions: Dict[str, FrontierRevision]
    external_parents: FrozenSet[str] = frozenset()

    @property
    def direct_reference_count(self) -> int:
        return sum(len(revision.parents) for revision in self.revisions.values())
