"""Durable working-state / causal-boundary experiments for A.P.C.

This module models one scalar merge domain.  It separates frequent local durable
working-state updates from portable causal revisions and asks what must happen
when remote causal state becomes observable while a local edit is still pending.

It is not a storage engine, UI implementation, transport implementation or
production format.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, FrozenSet, Optional

from reference_model.apc_model import ModelError, canonical_id
from reference_model.causality_lab import FrontierCausalRegister, FrontierRevision


@dataclass(frozen=True)
class WorkingMetrics:
    durable_write_count: int
    locally_created_causal_revisions: int
    retained_causal_nodes: int
    pending_dirty: bool


@dataclass
class DurableWorkingSnapshot:
    """Test-only crash-recovery image of one local working merge domain."""

    causal: FrontierCausalRegister
    working_value: Any
    dirty: bool
    observed_frontier: FrozenSet[str]
    durable_write_count: int
    locally_created_causal_revisions: int
    unsent_revision_ids: FrozenSet[str]


@dataclass
class WorkingScalar:
    """One local durable working value over a portable causal scalar register.

    The first local working edit captures the causal frontier actually observed
    at the start of that pending edit epoch.  Further local writes are durable but
    do not create portable causal revisions.

    If remote state is about to become semantically observable while the local
    epoch is dirty, the local epoch is sealed *before* the remote state is merged.
    This prevents a later coalesced revision from falsely claiming to have
    observed a remote revision that arrived only after the local work was done.
    """

    causal: FrontierCausalRegister = field(default_factory=FrontierCausalRegister)
    working_value: Any = None
    dirty: bool = False
    observed_frontier: FrozenSet[str] = frozenset()
    durable_write_count: int = 0
    locally_created_causal_revisions: int = 0
    unsent_revision_ids: set[str] = field(default_factory=set)

    @classmethod
    def from_causal(cls, causal: FrontierCausalRegister) -> "WorkingScalar":
        causal.validate()
        return cls(causal=causal.copy(), working_value=causal.materialized_value())

    def copy(self) -> "WorkingScalar":
        return WorkingScalar(
            causal=self.causal.copy(),
            working_value=self.working_value,
            dirty=self.dirty,
            observed_frontier=frozenset(self.observed_frontier),
            durable_write_count=self.durable_write_count,
            locally_created_causal_revisions=self.locally_created_causal_revisions,
            unsent_revision_ids=set(self.unsent_revision_ids),
        )

    def durable_edit(self, value: Any) -> None:
        """Persist a new local working value without creating a causal revision."""

        if not self.dirty:
            self.observed_frontier = frozenset(self.causal.frontier())
            self.dirty = True
        self.working_value = value
        self.durable_write_count += 1

    def _append_revision(
        self,
        *,
        revision_id: str,
        value: Any,
        parents: FrozenSet[str],
    ) -> FrontierRevision:
        canonical_id(revision_id)
        if revision_id in self.causal.revisions:
            raise ModelError("working-state seal reuses an existing revision ID")

        missing = set(parents) - set(self.causal.revisions)
        if missing:
            raise ModelError(
                f"working-state observed parent(s) are no longer available: {sorted(missing)!r}"
            )

        revision = FrontierRevision(
            revision_id=revision_id,
            value=value,
            parents=parents,
        )
        revisions = dict(self.causal.revisions)
        revisions[revision_id] = revision
        updated = FrontierCausalRegister(revisions)
        updated.validate()
        self.causal = updated
        return revision

    def seal(self, revision_id: str) -> Optional[FrontierRevision]:
        """Turn the current durable working epoch into one portable revision."""

        if not self.dirty:
            return None

        revision = self._append_revision(
            revision_id=revision_id,
            value=self.working_value,
            parents=self.observed_frontier,
        )
        self.dirty = False
        self.observed_frontier = frozenset()
        self.locally_created_causal_revisions += 1
        self.unsent_revision_ids.add(revision_id)
        self.working_value = self.causal.materialized_value()
        return revision

    def observe_remote(
        self,
        remote: FrontierCausalRegister,
        *,
        pre_observation_revision_id: Optional[str] = None,
    ) -> None:
        """Make remote causal state observable without falsifying local causality.

        A dirty local epoch must be sealed before the remote state is merged.  A
        caller therefore supplies a fresh revision ID whenever dirty work exists.
        Downloading opaque transport data alone need not call this method; this is
        the semantic observation/apply boundary.
        """

        remote.validate()
        if self.dirty:
            if pre_observation_revision_id is None:
                raise ModelError(
                    "dirty working state must be sealed before remote observation"
                )
            self.seal(pre_observation_revision_id)
        elif pre_observation_revision_id is not None:
            raise ModelError("pre-observation revision ID supplied with no dirty state")

        self.causal = self.causal.merge(remote)
        self.working_value = self.causal.materialized_value()

    def snapshot(self) -> DurableWorkingSnapshot:
        """Return a test-only durable crash-recovery image."""

        return DurableWorkingSnapshot(
            causal=self.causal.copy(),
            working_value=self.working_value,
            dirty=self.dirty,
            observed_frontier=frozenset(self.observed_frontier),
            durable_write_count=self.durable_write_count,
            locally_created_causal_revisions=self.locally_created_causal_revisions,
            unsent_revision_ids=frozenset(self.unsent_revision_ids),
        )

    @classmethod
    def restore(cls, snapshot: DurableWorkingSnapshot) -> "WorkingScalar":
        snapshot.causal.validate()
        return cls(
            causal=snapshot.causal.copy(),
            working_value=snapshot.working_value,
            dirty=snapshot.dirty,
            observed_frontier=frozenset(snapshot.observed_frontier),
            durable_write_count=snapshot.durable_write_count,
            locally_created_causal_revisions=snapshot.locally_created_causal_revisions,
            unsent_revision_ids=set(snapshot.unsent_revision_ids),
        )

    def metrics(self) -> WorkingMetrics:
        return WorkingMetrics(
            durable_write_count=self.durable_write_count,
            locally_created_causal_revisions=self.locally_created_causal_revisions,
            retained_causal_nodes=len(self.causal.revisions),
            pending_dirty=self.dirty,
        )


def naive_publish_using_latest_frontier(
    *,
    causal_after_remote: FrontierCausalRegister,
    revision_id: str,
    pending_value: Any,
) -> FrontierCausalRegister:
    """Counterexample helper: incorrectly causalize old work against new context.

    This is deliberately wrong for a pending edit that was produced before the
    remote frontier became observable.  It exists only so tests can demonstrate
    the semantic error caused by using the latest frontier at publication time.
    """

    return causal_after_remote.assign(revision_id, pending_value)
