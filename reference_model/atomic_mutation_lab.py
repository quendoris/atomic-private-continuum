"""Atomic multi-domain mutation experiments for A.P.C.

This lab asks when independent merge-domain causality is insufficient because a
single semantic mutation requires all of its domain effects to become visible as
one unit.

It is deliberately small.  It does not define a production transaction protocol.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, FrozenSet, Mapping, Optional

from reference_model.apc_model import ModelError, canonical_id
from reference_model.causality_lab import FrontierCausalRegister, FrontierRevision


@dataclass(frozen=True)
class AtomicMutation:
    mutation_id: str
    members: Mapping[str, FrontierRevision]

    def __post_init__(self) -> None:
        canonical_id(self.mutation_id)
        if not self.members:
            raise ModelError("atomic mutation must touch at least one domain")
        for name, revision in self.members.items():
            if not name:
                raise ModelError("merge-domain name must not be empty")
            canonical_id(revision.revision_id)

    @property
    def domains(self) -> FrozenSet[str]:
        return frozenset(self.members)


@dataclass
class AtomicDomainState:
    domains: Dict[str, FrontierCausalRegister] = field(default_factory=dict)

    def copy(self) -> "AtomicDomainState":
        return AtomicDomainState({name: state.copy() for name, state in self.domains.items()})

    def ensure_domain(self, name: str) -> FrontierCausalRegister:
        if not name:
            raise ModelError("merge-domain name must not be empty")
        return self.domains.setdefault(name, FrontierCausalRegister())

    def seed(self, name: str, revision_id: str, value: Any) -> None:
        self.domains[name] = self.ensure_domain(name).assign(revision_id, value)

    def build_mutation(
        self,
        mutation_id: str,
        changes: Mapping[str, tuple[str, Any]],
    ) -> AtomicMutation:
        members: Dict[str, FrontierRevision] = {}
        for name, (revision_id, value) in changes.items():
            causal = self.ensure_domain(name)
            members[name] = FrontierRevision(
                revision_id=revision_id,
                value=value,
                parents=frozenset(causal.frontier()),
            )
        return AtomicMutation(mutation_id=mutation_id, members=members)

    def apply_complete(self, mutation: AtomicMutation) -> None:
        """Apply all mutation members as one logical visibility step.

        Every referenced parent must already be available in the corresponding
        domain.  The method prepares all updated domain registers first and only
        swaps them into visible state if every member validates.
        """

        prepared: Dict[str, FrontierCausalRegister] = {}
        for name, revision in mutation.members.items():
            current = self.ensure_domain(name)
            missing = set(revision.parents) - set(current.revisions)
            if missing:
                raise ModelError(
                    f"atomic member for {name!r} has unavailable parent(s): {sorted(missing)!r}"
                )
            revisions = dict(current.revisions)
            previous = revisions.get(revision.revision_id)
            if previous is not None and previous != revision:
                raise ModelError("atomic member revision ID collision")
            revisions[revision.revision_id] = revision
            candidate = FrontierCausalRegister(revisions)
            candidate.validate()
            prepared[name] = candidate

        for name, candidate in prepared.items():
            self.domains[name] = candidate

    def merge_domains_independently(self, other: "AtomicDomainState") -> "AtomicDomainState":
        """Ordinary domain-local merge, intentionally unaware of mutation groups."""

        names = set(self.domains) | set(other.domains)
        merged = AtomicDomainState()
        for name in names:
            left = self.domains.get(name, FrontierCausalRegister())
            right = other.domains.get(name, FrontierCausalRegister())
            merged.domains[name] = left.merge(right)
        return merged

    def materialized_values(self) -> Dict[str, Any]:
        return {
            name: state.materialized_value()
            for name, state in sorted(self.domains.items())
        }


@dataclass
class AtomicMutationInbox:
    """Test-only multipart receiver that exposes no partial atomic mutation."""

    expected_domains: Dict[str, FrozenSet[str]] = field(default_factory=dict)
    received_members: Dict[str, Dict[str, FrontierRevision]] = field(default_factory=dict)

    def announce(self, mutation_id: str, domains: FrozenSet[str]) -> None:
        canonical_id(mutation_id)
        if not domains:
            raise ModelError("atomic mutation announcement must name domains")
        previous = self.expected_domains.get(mutation_id)
        if previous is not None and previous != domains:
            raise ModelError("atomic mutation domain-set mismatch")
        self.expected_domains[mutation_id] = domains

    def receive(self, mutation_id: str, domain: str, revision: FrontierRevision) -> None:
        expected = self.expected_domains.get(mutation_id)
        if expected is None:
            raise ModelError("atomic mutation must be announced before member delivery")
        if domain not in expected:
            raise ModelError("received member outside announced atomic domain set")
        bucket = self.received_members.setdefault(mutation_id, {})
        previous = bucket.get(domain)
        if previous is not None and previous != revision:
            raise ModelError("conflicting duplicate atomic member")
        bucket[domain] = revision

    def complete(self, mutation_id: str) -> Optional[AtomicMutation]:
        expected = self.expected_domains.get(mutation_id)
        if expected is None:
            raise ModelError("unknown atomic mutation")
        bucket = self.received_members.get(mutation_id, {})
        if set(bucket) != set(expected):
            return None
        return AtomicMutation(mutation_id=mutation_id, members=dict(bucket))


def mutation_consistent(values: Mapping[str, Any], expected_tuple: Mapping[str, Any]) -> bool:
    """Return whether every named domain matches one complete semantic mutation."""

    return all(values.get(name) == value for name, value in expected_tuple.items())
