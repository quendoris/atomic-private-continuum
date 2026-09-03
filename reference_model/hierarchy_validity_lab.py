"""Deterministic hierarchy validity-resolution experiments for A.P.C.

The base hierarchy lab intentionally allows independently merged parent registers
to materialize a parent cycle.  This module tests one conservative candidate:

1. materialize each parent register normally;
2. if the active parent graph contains a cycle, reject one active placement
   revision using a deterministic opaque-ID tie-break;
3. let that atom fall back to the next materializable historical placement;
4. repeat until the active parent graph is acyclic.

This is not a production proposal.  It deliberately retains historical placement
revisions so the semantic and metadata costs of fallback are explicit.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, FrozenSet, Optional

from reference_model.apc_model import ModelError, ScalarRegister, canonical_id
from reference_model.hierarchy_lab import HierarchyLab


@dataclass(frozen=True)
class ResolvedHierarchy:
    active_parents: Dict[str, Optional[str]]
    active_revision_ids: Dict[str, str]
    rejected_revision_ids: FrozenSet[str]


def _materialize_register(register: ScalarRegister, rejected: set[str]):
    filtered = ScalarRegister(
        {
            revision_id: revision
            for revision_id, revision in register.revisions.items()
            if revision_id not in rejected
        }
    )
    return filtered.materialized_revision()


def _find_cycle(active_parents: Dict[str, Optional[str]]) -> Optional[list[str]]:
    globally_done: set[str] = set()
    for start in active_parents:
        if start in globally_done:
            continue
        path: list[str] = []
        index: Dict[str, int] = {}
        current: Optional[str] = start
        while current is not None and current in active_parents:
            if current in index:
                return path[index[current]:]
            if current in globally_done:
                break
            index[current] = len(path)
            path.append(current)
            current = active_parents[current]
        globally_done.update(path)
    return None


def resolve_acyclic_by_rejecting_lowest_cycle_revision(
    hierarchy: HierarchyLab,
) -> ResolvedHierarchy:
    """Resolve active parent cycles by rejecting one placement revision at a time.

    The opaque revision ID is used only as a deterministic tie-break among active
    edges participating in the same invalid cycle.  Its magnitude is not treated
    as time or causal order.

    When a revision is rejected, historical revisions of that atom may become
    materializable again.  This makes the hidden storage requirement explicit:
    safe cycle fallback needs more information than only the current parent value.
    """

    rejected: set[str] = set()

    while True:
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

        cycle = _find_cycle(parents)
        if cycle is None:
            return ResolvedHierarchy(
                active_parents=parents,
                active_revision_ids=active_ids,
                rejected_revision_ids=frozenset(rejected),
            )

        cycle_revisions = [active_ids[atom_id] for atom_id in cycle]
        loser = min(cycle_revisions, key=canonical_id)
        if loser in rejected:
            raise ModelError("cycle resolver made no progress")
        rejected.add(loser)
