"""Bounded hierarchy fallback experiments for A.P.C.

The full-history hierarchy resolver can preserve old placement intent, but an
adversarial merged state can force fallback work linear in one atom's retained
move history.  This module compares two deliberately bounded alternatives:

* root fallback: reject an invalid current placement and place the atom at root;
* one-witness fallback: try one stored previous-known-valid parent, then root.

The input remains the explicit-history HierarchyLab so the bounded policies can be
compared against the current oracle.  Production causality compaction is a
separate problem; these experiments isolate hierarchy-validity semantics.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, Iterable, Mapping, Optional

from reference_model.apc_model import ModelError, ScalarRegister, canonical_id
from reference_model.hierarchy_lab import HierarchyLab


@dataclass(frozen=True)
class BoundedHierarchyMetrics:
    atom_count: int
    initial_cycle_count: int
    resolution_iterations: int
    rejected_current_revision_count: int
    witness_fallback_count: int
    root_fallback_count: int
    max_fallback_steps_per_atom: int


@dataclass(frozen=True)
class BoundedHierarchyResolution:
    active_parents: Dict[str, Optional[str]]
    source_revision_ids: Dict[str, str]
    fallback_modes: Dict[str, str]
    rejected_current_revision_ids: frozenset[str]
    metrics: BoundedHierarchyMetrics


def _current_state(
    hierarchy: HierarchyLab,
) -> tuple[Dict[str, Optional[str]], Dict[str, str]]:
    parents: Dict[str, Optional[str]] = {}
    revision_ids: Dict[str, str] = {}
    for atom_id, register in hierarchy.parents.items():
        revision = register.materialized_revision()
        if revision is None:
            raise ModelError("hierarchy atom has no current placement")
        parent = revision.value
        if parent is not None and parent not in hierarchy.parents:
            raise ModelError("current parent references an unknown atom")
        if parent == atom_id:
            raise ModelError("atom cannot be its own parent")
        parents[atom_id] = parent
        revision_ids[atom_id] = revision.revision_id
    return parents, revision_ids


def _all_cycles(active_parents: Mapping[str, Optional[str]]) -> list[list[str]]:
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


def _choose_loser(cycle: list[str], source_revision_ids: Mapping[str, str]) -> str:
    return min(
        cycle,
        key=lambda atom_id: (
            canonical_id(source_revision_ids[atom_id]),
            canonical_id(atom_id),
        ),
    )


def previous_parent_witnesses(hierarchy: HierarchyLab) -> Dict[str, Optional[str]]:
    """Derive test witnesses from the explicit-history oracle.

    For each placement revision, return the parent value that was materialized from
    that revision's same-atom causal context before the revision was created.

    A production one-witness design would store this bounded witness at placement
    creation time.  This helper deliberately derives it from retained history only
    so old tests can compare semantic policies without changing HierarchyLab.
    """

    witnesses: Dict[str, Optional[str]] = {}
    for register in hierarchy.parents.values():
        for revision_id, revision in register.revisions.items():
            prior = {
                candidate_id: candidate
                for candidate_id, candidate in register.revisions.items()
                if candidate_id in revision.context
            }
            if not prior:
                witnesses[revision_id] = None
                continue
            witnesses[revision_id] = ScalarRegister(prior).materialized_value()
    return witnesses


def _finish(
    *,
    parents: Dict[str, Optional[str]],
    source_revision_ids: Dict[str, str],
    modes: Dict[str, str],
    rejected: set[str],
    initial_cycle_count: int,
    iterations: int,
    steps_per_atom: Dict[str, int],
) -> BoundedHierarchyResolution:
    return BoundedHierarchyResolution(
        active_parents=dict(parents),
        source_revision_ids=dict(source_revision_ids),
        fallback_modes=dict(modes),
        rejected_current_revision_ids=frozenset(rejected),
        metrics=BoundedHierarchyMetrics(
            atom_count=len(parents),
            initial_cycle_count=initial_cycle_count,
            resolution_iterations=iterations,
            rejected_current_revision_count=len(rejected),
            witness_fallback_count=sum(1 for mode in modes.values() if mode == "witness"),
            root_fallback_count=sum(1 for mode in modes.values() if mode == "root"),
            max_fallback_steps_per_atom=max(steps_per_atom.values(), default=0),
        ),
    )


def resolve_with_root_fallback(hierarchy: HierarchyLab) -> BoundedHierarchyResolution:
    """Reject an invalid current placement directly to root.

    Each atom can be forced through this fallback at most once for one resolved
    input state, so the policy does not walk historical placement chains.
    """

    parents, source_ids = _current_state(hierarchy)
    initial_cycles = len(_all_cycles(parents))
    modes = {atom_id: "current" for atom_id in parents}
    rejected: set[str] = set()
    steps: Dict[str, int] = {}
    iterations = 0

    while True:
        cycles = _all_cycles(parents)
        if not cycles:
            return _finish(
                parents=parents,
                source_revision_ids=source_ids,
                modes=modes,
                rejected=rejected,
                initial_cycle_count=initial_cycles,
                iterations=iterations,
                steps_per_atom=steps,
            )

        cycle = min(cycles, key=_cycle_key)
        loser = _choose_loser(cycle, source_ids)
        if modes[loser] == "root":
            raise ModelError("root fallback failed to remove an atom from a cycle")

        rejected.add(source_ids[loser])
        parents[loser] = None
        modes[loser] = "root"
        steps[loser] = steps.get(loser, 0) + 1
        iterations += 1


def resolve_with_one_witness_fallback(
    hierarchy: HierarchyLab,
    *,
    witness_by_revision: Mapping[str, Optional[str]] | None = None,
) -> BoundedHierarchyResolution:
    """Try one previous-parent witness, then root if another cycle remains.

    The witness is intentionally not a historical placement revision that becomes
    active again.  It is bounded validity metadata attached to the current move.
    If that witness is also invalid in the merged graph, the atom falls to root
    rather than walking arbitrary move history.
    """

    parents, source_ids = _current_state(hierarchy)
    witnesses = dict(witness_by_revision or previous_parent_witnesses(hierarchy))
    initial_cycles = len(_all_cycles(parents))
    modes = {atom_id: "current" for atom_id in parents}
    rejected: set[str] = set()
    steps: Dict[str, int] = {}
    iterations = 0

    while True:
        cycles = _all_cycles(parents)
        if not cycles:
            return _finish(
                parents=parents,
                source_revision_ids=source_ids,
                modes=modes,
                rejected=rejected,
                initial_cycle_count=initial_cycles,
                iterations=iterations,
                steps_per_atom=steps,
            )

        cycle = min(cycles, key=_cycle_key)
        loser = _choose_loser(cycle, source_ids)
        source_revision_id = source_ids[loser]
        rejected.add(source_revision_id)

        if modes[loser] == "current":
            witness = witnesses.get(source_revision_id)
            if witness is not None and witness not in parents:
                raise ModelError("fallback witness references an unknown atom")
            if witness == loser:
                raise ModelError("fallback witness cannot self-parent")
            parents[loser] = witness
            modes[loser] = "witness"
        elif modes[loser] == "witness":
            parents[loser] = None
            modes[loser] = "root"
        else:
            raise ModelError("root fallback failed to remove an atom from a cycle")

        steps[loser] = steps.get(loser, 0) + 1
        if steps[loser] > 2:
            raise ModelError("one-witness policy exceeded its per-atom fallback bound")
        iterations += 1
