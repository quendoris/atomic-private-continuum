"""Merge-domain-local causal observation experiments for A.P.C.

The existing working-state lab models one scalar merge domain.  This module asks
whether a remote change in one independent domain must force a causal boundary in
other dirty domains.

It is a logical experiment, not a transaction/storage implementation.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, Mapping, Optional

from reference_model.apc_model import ModelError
from reference_model.causality_lab import FrontierCausalRegister
from reference_model.working_state_lab import WorkingScalar


@dataclass(frozen=True)
class DomainMetrics:
    durable_writes: int
    locally_created_causal_revisions: int
    dirty_domains: int


@dataclass
class DomainWorkingSet:
    """Independent working scalars keyed by semantic merge-domain name."""

    domains: Dict[str, WorkingScalar] = field(default_factory=dict)

    def copy(self) -> "DomainWorkingSet":
        return DomainWorkingSet({name: state.copy() for name, state in self.domains.items()})

    def ensure_domain(
        self,
        name: str,
        causal: Optional[FrontierCausalRegister] = None,
    ) -> WorkingScalar:
        existing = self.domains.get(name)
        if existing is not None:
            if causal is not None:
                raise ModelError("domain already exists")
            return existing
        state = WorkingScalar.from_causal(causal or FrontierCausalRegister())
        self.domains[name] = state
        return state

    def durable_edit(self, name: str, value: Any) -> None:
        self.ensure_domain(name).durable_edit(value)

    def observe_remote_domain(
        self,
        name: str,
        remote: FrontierCausalRegister,
        *,
        pre_observation_revision_id: Optional[str] = None,
    ) -> None:
        """Apply an observation boundary only to the touched merge domain."""

        self.ensure_domain(name).observe_remote(
            remote,
            pre_observation_revision_id=pre_observation_revision_id,
        )

    def observe_remote_projection(
        self,
        remotes: Mapping[str, FrontierCausalRegister],
        *,
        pre_observation_revision_ids: Mapping[str, str] | None = None,
    ) -> None:
        """Observe several changed domains without sealing unrelated dirty ones."""

        ids = dict(pre_observation_revision_ids or {})
        unused = set(ids) - set(remotes)
        if unused:
            raise ModelError("pre-observation ID supplied for untouched domain")

        for name, remote in remotes.items():
            state = self.ensure_domain(name)
            revision_id = ids.get(name)
            if state.dirty and revision_id is None:
                raise ModelError(
                    f"dirty touched domain {name!r} needs a pre-observation revision ID"
                )
            if not state.dirty and revision_id is not None:
                raise ModelError(
                    f"clean touched domain {name!r} received unnecessary revision ID"
                )
            state.observe_remote(
                remote,
                pre_observation_revision_id=revision_id,
            )

    def seal_domain(self, name: str, revision_id: str):
        return self.ensure_domain(name).seal(revision_id)

    def metrics(self) -> DomainMetrics:
        states = list(self.domains.values())
        return DomainMetrics(
            durable_writes=sum(state.durable_write_count for state in states),
            locally_created_causal_revisions=sum(
                state.locally_created_causal_revisions for state in states
            ),
            dirty_domains=sum(1 for state in states if state.dirty),
        )


def naive_global_observation_boundary(
    working: DomainWorkingSet,
    *,
    touched_domain: str,
    remote: FrontierCausalRegister,
    revision_ids_for_all_dirty: Mapping[str, str],
) -> None:
    """Counterexample policy: seal every dirty domain before any remote observation.

    This function is intentionally over-broad.  It lets tests quantify the causal
    metadata cost of treating the whole continuum as one observation domain even
    when the logical merge domains are independent.
    """

    dirty_names = {name for name, state in working.domains.items() if state.dirty}
    if dirty_names != set(revision_ids_for_all_dirty):
        raise ModelError("global policy needs one revision ID for every dirty domain")

    for name in sorted(dirty_names):
        working.domains[name].seal(revision_ids_for_all_dirty[name])

    state = working.ensure_domain(touched_domain)
    state.observe_remote(remote)
