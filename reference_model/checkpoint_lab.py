"""Checkpoint/causal-horizon experiments for A.P.C.

This module asks a narrower question than ``causality_lab``:

Can we discard old causal nodes while still accepting a branch that was created
from a long-offline historical baseline?

The model deliberately uses an exact set of covered opaque IDs.  That is not a
production proposal; it is an oracle for the information that compaction must
somehow preserve or prove.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, FrozenSet, Iterable, Optional

from reference_model.apc_model import ModelError, canonical_id
from reference_model.causality_lab import FrontierCausalRegister, FrontierRevision


@dataclass(frozen=True)
class CheckpointMetrics:
    retained_revision_count: int
    covered_id_count: int
    retained_direct_reference_count: int
    frontier_width: int

    @property
    def exact_coverage_payload_bytes_at_256_bits(self) -> int:
        """Information payload of opaque 256-bit covered IDs, ignoring overhead."""

        return self.covered_id_count * 32


@dataclass
class ExactCoverageRegister:
    """Compacted causal register with exact historical-ID membership.

    Current causal frontier revision IDs are retained unchanged.  Dominated
    historical nodes may be removed from the active DAG, but their IDs remain in
    ``covered_ids`` so a returning stale branch can prove that its missing parent
    belonged to a previously validated local history.

    Preserving the original frontier IDs is essential because A.P.C. scalar
    materialization uses those IDs as a deterministic tie-break only for genuine
    concurrency.  Replacing a frontier revision with a fresh checkpoint ID could
    change a future conflict winner and would therefore change semantics.

    This research model assumes incoming revisions are already authenticated and
    valid.  ``covered_ids`` is only a membership oracle; it does not bind old ID
    values to discarded revision contents.
    """

    retained: Dict[str, FrontierRevision] = field(default_factory=dict)
    covered_ids: FrozenSet[str] = frozenset()

    def copy(self) -> "ExactCoverageRegister":
        return ExactCoverageRegister(dict(self.retained), frozenset(self.covered_ids))

    @classmethod
    def from_full(cls, register: FrontierCausalRegister) -> "ExactCoverageRegister":
        register.validate()
        frontier_ids = set(register.frontier())
        return cls(
            retained={
                revision_id: register.revisions[revision_id]
                for revision_id in frontier_ids
            },
            covered_ids=frozenset(set(register.revisions) - frontier_ids),
        )

    def _validate(self) -> None:
        if set(self.retained) & set(self.covered_ids):
            raise ModelError("a revision cannot be both retained and covered")

        for revision_id in self.covered_ids:
            canonical_id(revision_id)

        available = set(self.retained) | set(self.covered_ids)
        for revision in self.retained.values():
            missing = set(revision.parents) - available
            if missing:
                raise ModelError(
                    f"compacted causal parent(s) are unknown: {sorted(missing)!r}"
                )

        visiting: set[str] = set()
        visited: set[str] = set()

        def visit(revision_id: str) -> None:
            if revision_id in visited:
                return
            if revision_id in visiting:
                raise ModelError("retained causal graph contains a cycle")
            visiting.add(revision_id)
            for parent in self.retained[revision_id].parents:
                if parent in self.retained:
                    visit(parent)
            visiting.remove(revision_id)
            visited.add(revision_id)

        for revision_id in self.retained:
            visit(revision_id)

    def frontier(self) -> Dict[str, FrontierRevision]:
        referenced_retained: set[str] = set()
        for revision in self.retained.values():
            referenced_retained.update(
                parent for parent in revision.parents if parent in self.retained
            )
        return {
            revision_id: revision
            for revision_id, revision in self.retained.items()
            if revision_id not in referenced_retained
        }

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

    def assign(self, revision_id: str, value: Any) -> "ExactCoverageRegister":
        canonical_id(revision_id)
        if revision_id in self.covered_ids:
            raise ModelError("cannot reuse an ID already covered by a checkpoint")

        revision = FrontierRevision(
            revision_id=revision_id,
            value=value,
            parents=frozenset(self.frontier()),
        )
        result = self.copy()
        previous = result.retained.get(revision_id)
        if previous is not None and previous != revision:
            raise ModelError("revision ID collision with different content")
        result.retained[revision_id] = revision
        result._validate()
        return result

    def import_revisions(
        self,
        revisions: Dict[str, FrontierRevision] | Iterable[FrontierRevision],
    ) -> "ExactCoverageRegister":
        """Import authenticated revisions, allowing parents covered by history.

        A revision whose own ID is already covered is treated as historical data
        already represented by the checkpoint.  A *new* revision may name an old
        covered ID as a direct parent; this is how a long-offline branch reconnects
        without resurrecting the full historical DAG.
        """

        if isinstance(revisions, dict):
            incoming = dict(revisions)
        else:
            incoming = {revision.revision_id: revision for revision in revisions}

        for revision_id, revision in incoming.items():
            if revision_id != revision.revision_id:
                raise ModelError("incoming revision mapping key mismatch")

        result = self.copy()
        genuinely_new: Dict[str, FrontierRevision] = {}
        for revision_id, revision in incoming.items():
            if revision_id in result.covered_ids:
                # The checkpoint membership oracle says this historical ID was
                # already incorporated.  Content binding is outside this lab.
                continue
            previous = result.retained.get(revision_id)
            if previous is not None:
                if previous != revision:
                    raise ModelError("revision ID collision with different content")
                continue
            genuinely_new[revision_id] = revision

        available = (
            set(result.retained)
            | set(result.covered_ids)
            | set(genuinely_new)
        )
        for revision in genuinely_new.values():
            missing = set(revision.parents) - available
            if missing:
                raise ModelError(
                    f"historical causal parent is not retained or covered: {sorted(missing)!r}"
                )

        result.retained.update(genuinely_new)
        result._validate()
        return result

    def compact(self) -> "ExactCoverageRegister":
        """Discard retained nodes dominated by the current retained frontier."""

        frontier_ids = set(self.frontier())
        newly_covered = set(self.covered_ids) | (set(self.retained) - frontier_ids)
        result = ExactCoverageRegister(
            retained={
                revision_id: self.retained[revision_id]
                for revision_id in frontier_ids
            },
            covered_ids=frozenset(newly_covered),
        )
        result._validate()
        return result

    def metrics(self) -> CheckpointMetrics:
        return CheckpointMetrics(
            retained_revision_count=len(self.retained),
            covered_id_count=len(self.covered_ids),
            retained_direct_reference_count=sum(
                len(revision.parents) for revision in self.retained.values()
            ),
            frontier_width=len(self.frontier()),
        )
