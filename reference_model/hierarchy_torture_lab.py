"""Stress/measurement harness for A.P.C. hierarchy validity research.

The hierarchy validity candidate resolves parent cycles by rejecting one active
placement revision and falling back to retained historical placement state.  This
module keeps that semantic shape but records how expensive the fallback becomes
under adversarial merge states.

It is not a production hierarchy algorithm or performance benchmark.  Counts are
logical metadata/iteration measurements over the Python reference model.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, Iterable, Optional

from reference_model.apc_model import ModelError, ScalarRegister, canonical_id
from reference_model.hierarchy_lab import HierarchyLab
from reference_model.hierarchy_validity_lab import ResolvedHierarchy


@dataclass(frozen=True)
class HierarchyTortureMetrics:
    atom_count: int
    total_placement_revisions: int
    historical_placement_revisions: int
    initial_cycle_count: int
    resolution_iterations: int
    rejected_revision_count: int
    max_fallback_depth: int
    total_fallback_steps: int


@dataclass(frozen=True)
class TracedHierarchyResolution:
    resolved: ResolvedHierarchy
    metrics: HierarchyTortureMetrics


def _materialize_register(register: ScalarRegister, rejected: set[str]):
    filtered = ScalarRegister(
        {
            revision_id: revision
            for revision_id, revision in register.revisions.items()
            if revision_id not in rejected
        }
    )
    return filtered.materialized_revision()


def _materialize_all(
    hierarchy: HierarchyLab,
    rejected: set[str],
) -> tuple[Dict[str, Optional[str]], Dict[str, str]]:
    parents: Dict[str, Optional[str]] = {}
    active_ids: Dict[str, str] = {}

    for atom_id, register in hierarchy.parents.items():
        revision = _materialize_register(register, rejected)
        if revision is None:
            raise ModelError("cycle resolution exhausted every placement for an atom")
        parent = revision.value
        if parent is not None and parent not in hierarchy.parents:
            raise ModelError("resolved parent references an unknown atom")
        if parent == atom_id:
            raise ModelError("resolved hierarchy contains self-parent placement")
        parents[atom_id] = parent
        active_ids[atom_id] = revision.revision_id

    return parents, active_ids


def _all_cycles(active_parents: Dict[str, Optional[str]]) -> list[list[str]]:
    """Return all disjoint cycles in one materialized functional parent graph."""

    globally_done: set[str] = set()
    cycles: list[list[str]] = []

    for start in active_parents:
        if start in globally_done:
            continue

        path: list[str] = []
        index: Dict[str, int] = {}
        current: Optional[str] = start

        while current is not None and current in active_parents:
            if current in index:
                cycles.append(path[index[current]:])
                break
            if current in globally_done:
                break
            index[current] = len(path)
            path.append(current)
            current = active_parents[current]

        globally_done.update(path)

    return cycles


def _cycle_key(cycle: Iterable[str]) -> tuple[bytes, ...]:
    return tuple(sorted(canonical_id(atom_id) for atom_id in cycle))


def resolve_acyclic_with_metrics(hierarchy: HierarchyLab) -> TracedHierarchyResolution:
    """Resolve cycles while measuring rejection/fallback cost.

    The rejection policy matches the current validity experiment: among the
    active placement revisions in an invalid cycle, the lowest canonical opaque
    RevisionId is rejected.  When several disjoint cycles exist simultaneously,
    this harness chooses the cycle with the lowest canonical atom-set key so the
    trace itself is deterministic.
    """

    rejected: set[str] = set()
    rejected_per_atom: Dict[str, int] = {}

    parents, active_ids = _materialize_all(hierarchy, rejected)
    initial_cycle_count = len(_all_cycles(parents))
    iterations = 0

    while True:
        cycles = _all_cycles(parents)
        if not cycles:
            break

        cycle = min(cycles, key=_cycle_key)
        loser_atom = min(
            cycle,
            key=lambda atom_id: (
                canonical_id(active_ids[atom_id]),
                canonical_id(atom_id),
            ),
        )
        loser_revision = active_ids[loser_atom]
        if loser_revision in rejected:
            raise ModelError("cycle resolver made no progress")

        rejected.add(loser_revision)
        rejected_per_atom[loser_atom] = rejected_per_atom.get(loser_atom, 0) + 1
        iterations += 1
        parents, active_ids = _materialize_all(hierarchy, rejected)

    resolved = ResolvedHierarchy(
        active_parents=parents,
        active_revision_ids=active_ids,
        rejected_revision_ids=frozenset(rejected),
    )

    total_revisions = sum(
        len(register.revisions) for register in hierarchy.parents.values()
    )
    atom_count = len(hierarchy.parents)
    fallback_steps = sum(rejected_per_atom.values())

    return TracedHierarchyResolution(
        resolved=resolved,
        metrics=HierarchyTortureMetrics(
            atom_count=atom_count,
            total_placement_revisions=total_revisions,
            historical_placement_revisions=max(0, total_revisions - atom_count),
            initial_cycle_count=initial_cycle_count,
            resolution_iterations=iterations,
            rejected_revision_count=len(rejected),
            max_fallback_depth=max(rejected_per_atom.values(), default=0),
            total_fallback_steps=fallback_steps,
        ),
    )
