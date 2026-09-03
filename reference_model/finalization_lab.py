"""Portable revision finalization experiments for A.P.C.

This module studies the boundary between crash-safe local working/causal state and
final portable authenticated revisions.  It does not implement signatures or a
production key-evolution construction.  ``signing_transition_count`` is only a
logical cost counter used to expose architectural consequences of finalizing too
early.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, FrozenSet, Iterable
import copy

from reference_model.apc_model import ModelError, canonical_id
from reference_model.causality_lab import FrontierCausalRegister, FrontierRevision
from reference_model.private_causal_lab import ExposureAwareCausalState, SquashResult


@dataclass(frozen=True)
class FinalizedStatement:
    """Test-only immutable snapshot of the statement that would be authenticated."""

    revision_id: str
    value: Any
    parents: FrozenSet[str]

    @classmethod
    def from_revision(cls, revision: FrontierRevision) -> "FinalizedStatement":
        return cls(
            revision_id=revision.revision_id,
            value=copy.deepcopy(revision.value),
            parents=frozenset(revision.parents),
        )


@dataclass(frozen=True)
class FinalizationMetrics:
    local_revision_ids: int
    finalized_local_statements: int
    signing_transition_count: int
    handed_off_local_ids: int
    private_unfinalized_local_ids: int


@dataclass(frozen=True)
class FinalizationSnapshot:
    """Crash-recovery image for the finalization boundary experiment."""

    causal: FrontierCausalRegister
    local_ids: FrozenSet[str]
    private_local_ids: FrozenSet[str]
    exposed_ids: FrozenSet[str]
    finalized: dict[str, FinalizedStatement]
    handed_off_ids: FrozenSet[str]
    signing_transition_count: int


@dataclass
class FinalizationLedger:
    """Track private local causal identities before/after portable finalization.

    Local causal IDs may exist before an authenticated portable statement is
    minted.  Finalization freezes the revision value and parent set.  Transport
    handoff then makes the identity externally exposed even if acknowledgement is
    later lost.

    The model intentionally counts one signing/key-evolution transition per newly
    finalized local statement.  That is not a claim about a future cryptographic
    primitive; it is a stress model for constructions in which each authenticated
    local state advances signing state.
    """

    state: ExposureAwareCausalState
    local_ids: set[str] = field(default_factory=set)
    finalized: dict[str, FinalizedStatement] = field(default_factory=dict)
    handed_off_ids: set[str] = field(default_factory=set)
    signing_transition_count: int = 0

    def __post_init__(self) -> None:
        known = set(self.state.causal.revisions)
        if not self.local_ids <= known:
            raise ModelError("local finalization set contains unknown revision IDs")
        if not set(self.finalized) <= self.local_ids:
            raise ModelError("only locally owned revisions are finalized in this lab")
        if not self.handed_off_ids <= set(self.finalized):
            raise ModelError("transport handoff requires finalized local statements")
        self._validate_finalized_statements()

    @classmethod
    def from_causal(cls, causal: FrontierCausalRegister) -> "FinalizationLedger":
        causal.validate()
        return cls(state=ExposureAwareCausalState(causal=causal.copy()))

    def copy(self) -> "FinalizationLedger":
        return FinalizationLedger(
            state=self.state.copy(),
            local_ids=set(self.local_ids),
            finalized=dict(self.finalized),
            handed_off_ids=set(self.handed_off_ids),
            signing_transition_count=self.signing_transition_count,
        )

    def _validate_finalized_statements(self) -> None:
        for revision_id, statement in self.finalized.items():
            revision = self.state.causal.revisions.get(revision_id)
            if revision is None:
                raise ModelError("finalized revision was removed")
            if statement != FinalizedStatement.from_revision(revision):
                raise ModelError("finalized revision statement was rewritten")

    def append_local(
        self,
        *,
        revision_id: str,
        value: Any,
        parents: Iterable[str] | None = None,
    ) -> FrontierRevision:
        """Create an unfinalized private local causal revision.

        If parents are omitted the current causal frontier is captured.  The ID is
        already the stable causal/conflict identity, but no authentication or key
        transition is modeled yet.
        """

        canonical_id(revision_id)
        if revision_id in self.state.causal.revisions:
            raise ModelError("local causal revision ID already exists")

        parent_set = (
            frozenset(self.state.causal.frontier())
            if parents is None
            else frozenset(parents)
        )
        missing = set(parent_set) - set(self.state.causal.revisions)
        if missing:
            raise ModelError(f"local revision has unknown parent(s): {sorted(missing)!r}")

        revision = FrontierRevision(
            revision_id=revision_id,
            value=copy.deepcopy(value),
            parents=parent_set,
        )
        revisions = dict(self.state.causal.revisions)
        revisions[revision_id] = revision
        causal = FrontierCausalRegister(revisions)
        causal.validate()
        self.state.causal = causal
        self.local_ids.add(revision_id)
        self.state.private_local_ids.add(revision_id)
        return revision

    def merge_remote(self, remote: FrontierCausalRegister) -> None:
        """Merge already authenticated external causal state.

        Remote revisions are treated as externally meaningful and therefore are
        marked exposed in the local exposure model.  They are not counted as local
        signing transitions.
        """

        remote.validate()
        self.state.causal = self.state.causal.merge(remote)
        self.state.exposed_ids.update(remote.revisions)
        self.state.private_local_ids.difference_update(remote.revisions)
        self._validate_finalized_statements()

    def finalize(self, revision_id: str) -> FinalizedStatement:
        """Freeze one local revision statement before portable authentication."""

        if revision_id not in self.local_ids:
            raise ModelError("cannot finalize a non-local revision in this lab")
        revision = self.state.causal.revisions.get(revision_id)
        if revision is None:
            raise ModelError("cannot finalize a removed revision")

        statement = FinalizedStatement.from_revision(revision)
        previous = self.finalized.get(revision_id)
        if previous is not None:
            if previous != statement:
                raise ModelError("finalized statement changed after finalization")
            return previous

        self.finalized[revision_id] = statement
        self.signing_transition_count += 1
        return statement

    def _local_ancestor_closure(self, revision_ids: Iterable[str]) -> set[str]:
        pending = list(revision_ids)
        closure: set[str] = set()
        while pending:
            revision_id = pending.pop()
            if revision_id in closure:
                continue
            revision = self.state.causal.revisions.get(revision_id)
            if revision is None:
                raise ModelError("cannot hand off unknown revision")
            closure.add(revision_id)
            pending.extend(revision.parents)
        return closure & self.local_ids

    def handoff(self, revision_ids: Iterable[str]) -> None:
        """Model external transport handoff, not acknowledgement.

        Every locally owned causal identity named directly or transitively by the
        handed-off revisions must already have an immutable finalized statement.
        Once handed off, exposure is permanent for this compaction class.
        """

        ids = set(revision_ids)
        local_closure = self._local_ancestor_closure(ids)
        missing_finalization = local_closure - set(self.finalized)
        if missing_finalization:
            raise ModelError(
                "cannot hand off a revision that depends on unfinalized local causal IDs"
            )

        self.state.mark_exposed(ids)
        self.handed_off_ids.update(ids & self.local_ids)
        self._validate_finalized_statements()

    def squash_private(self) -> SquashResult:
        """Squash only if doing so cannot mutate an already finalized statement."""

        candidate = self.state.squash_unexposed_dominated()
        if not candidate.removed_ids:
            return candidate

        for revision_id, statement in self.finalized.items():
            revision = candidate.state.causal.revisions.get(revision_id)
            if revision is None or statement != FinalizedStatement.from_revision(revision):
                raise ModelError(
                    "private squashing would rewrite or remove an already finalized statement"
                )

        self.state = candidate.state
        self.local_ids.difference_update(candidate.removed_ids)
        return candidate

    def snapshot(self) -> FinalizationSnapshot:
        return FinalizationSnapshot(
            causal=self.state.causal.copy(),
            local_ids=frozenset(self.local_ids),
            private_local_ids=frozenset(self.state.private_local_ids),
            exposed_ids=frozenset(self.state.exposed_ids),
            finalized=dict(self.finalized),
            handed_off_ids=frozenset(self.handed_off_ids),
            signing_transition_count=self.signing_transition_count,
        )

    @classmethod
    def restore(cls, snapshot: FinalizationSnapshot) -> "FinalizationLedger":
        return cls(
            state=ExposureAwareCausalState(
                causal=snapshot.causal.copy(),
                private_local_ids=set(snapshot.private_local_ids),
                exposed_ids=set(snapshot.exposed_ids),
            ),
            local_ids=set(snapshot.local_ids),
            finalized=dict(snapshot.finalized),
            handed_off_ids=set(snapshot.handed_off_ids),
            signing_transition_count=snapshot.signing_transition_count,
        )

    def metrics(self) -> FinalizationMetrics:
        return FinalizationMetrics(
            local_revision_ids=len(self.local_ids),
            finalized_local_statements=len(self.finalized),
            signing_transition_count=self.signing_transition_count,
            handed_off_local_ids=len(self.handed_off_ids),
            private_unfinalized_local_ids=len(
                (self.local_ids - set(self.finalized)) - set(self.state.exposed_ids)
            ),
        )


def conflict_value_with_identity(
    *,
    base: FrontierCausalRegister,
    remote: FrontierCausalRegister,
    local_identity: str,
    local_value: Any,
) -> Any:
    """Materialize a concurrent local candidate using one stable conflict ID.

    This helper exists to test whether changing a provisional identity during
    finalization can change scalar semantics.  ``local_identity`` has no time
    meaning; it is only the existing deterministic concurrency tie-break.
    """

    local = base.assign(local_identity, local_value)
    return local.merge(remote).materialized_value()
