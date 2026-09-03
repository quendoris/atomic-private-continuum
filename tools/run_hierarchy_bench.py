#!/usr/bin/env python3
"""Local statistical stress runner for A.P.C. hierarchy research.

This tool is intentionally outside normal CI scale.  It builds a merged synthetic
branch storm directly, without copying an entire HierarchyLab for every offline
replica, so hundreds of thousands or millions of independent placement revisions
can be explored on a workstation.

Wall-clock time is used only to measure implementation cost.  It is never part of
A.P.C. merge semantics.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import platform
import random
import sys
import time
from typing import Optional

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from reference_model.apc_model import ScalarRegister, ScalarRevision  # noqa: E402
from reference_model.hierarchy_bounded_lab import (  # noqa: E402
    resolve_with_one_witness_fallback,
    resolve_with_root_fallback,
)
from reference_model.hierarchy_lab import HierarchyLab  # noqa: E402
from reference_model.hierarchy_torture_lab import resolve_acyclic_with_metrics  # noqa: E402


PRESETS = {
    "smoke": dict(atoms=100, branches=500, forced_cycles=4),
    "medium": dict(atoms=20_000, branches=200_000, forced_cycles=32),
    "large": dict(atoms=100_000, branches=1_000_000, forced_cycles=64),
}


def hid(value: int) -> str:
    return f"{value:064x}"


def build_branch_storm(
    *,
    atom_count: int,
    branch_count: int,
    forced_cycles: int,
    seed: int,
) -> tuple[HierarchyLab, dict[str, Optional[str]], dict[str, int]]:
    if atom_count < 2:
        raise ValueError("atom_count must be >= 2")
    if branch_count < 0 or forced_cycles < 0:
        raise ValueError("branch_count and forced_cycles must be non-negative")
    if forced_cycles * 2 > atom_count:
        raise ValueError("forced_cycles requires two reserved atoms per cycle")

    rng = random.Random(seed)

    base_parent: dict[str, Optional[str]] = {}
    base_revision: dict[str, str] = {}
    registers: dict[str, ScalarRegister] = {}

    # Deterministic random acyclic base tree.  Parent indices are always smaller
    # than the child index, so the initial graph cannot contain a cycle.
    for atom_number in range(1, atom_count + 1):
        atom_id = hid(atom_number)
        if atom_number == 1 or rng.random() < 0.12:
            parent = None
        else:
            parent = hid(rng.randrange(1, atom_number))
        revision_id = hid(10_000_000_000 + atom_number)
        base_parent[atom_id] = parent
        base_revision[atom_id] = revision_id
        registers[atom_id] = ScalarRegister(
            {
                revision_id: ScalarRevision(
                    revision_id=revision_id,
                    value=parent,
                    context=frozenset(),
                )
            }
        )

    witnesses: dict[str, Optional[str]] = dict()
    random_revision_base = 20_000_000_000

    # Every generated branch is independent from the others for that atom: it
    # observes only the atom's base placement.  This creates a large concurrent
    # branch storm without pretending branch index is causal time.
    for index in range(branch_count):
        atom_number = rng.randrange(1, atom_count + 1)
        atom_id = hid(atom_number)
        if rng.random() < 0.10:
            parent = None
        else:
            parent_number = rng.randrange(1, atom_count + 1)
            while parent_number == atom_number:
                parent_number = rng.randrange(1, atom_count + 1)
            parent = hid(parent_number)

        revision_id = hid(random_revision_base + index)
        registers[atom_id].revisions[revision_id] = ScalarRevision(
            revision_id=revision_id,
            value=parent,
            context=frozenset({base_revision[atom_id]}),
        )
        witnesses[revision_id] = base_parent[atom_id]

    # Force disjoint active 2-cycles with IDs above all random branch IDs.  These
    # guarantee validity work even when random active winners happen to be acyclic.
    forced_revision_base = random_revision_base + branch_count + 10_000
    for cycle_index in range(forced_cycles):
        a_number = cycle_index * 2 + 1
        b_number = a_number + 1
        a = hid(a_number)
        b = hid(b_number)

        a_revision = hid(forced_revision_base + cycle_index * 2)
        b_revision = hid(forced_revision_base + cycle_index * 2 + 1)
        registers[a].revisions[a_revision] = ScalarRevision(
            revision_id=a_revision,
            value=b,
            context=frozenset({base_revision[a]}),
        )
        registers[b].revisions[b_revision] = ScalarRevision(
            revision_id=b_revision,
            value=a,
            context=frozenset({base_revision[b]}),
        )
        witnesses[a_revision] = base_parent[a]
        witnesses[b_revision] = base_parent[b]

    state = HierarchyLab(parents=registers)
    counts = {
        "atoms": atom_count,
        "independent_branch_revisions": branch_count,
        "forced_cycles": forced_cycles,
        "total_placement_revisions": atom_count + branch_count + forced_cycles * 2,
    }
    return state, witnesses, counts


def parent_digest(parents: dict[str, Optional[str]]) -> str:
    digest = hashlib.sha256()
    for atom_id, parent in sorted(parents.items()):
        digest.update(bytes.fromhex(atom_id))
        digest.update(b"\x00" if parent is None else bytes.fromhex(parent))
    return digest.hexdigest()


def timed(label: str, fn):
    started = time.perf_counter()
    value = fn()
    elapsed = time.perf_counter() - started
    return label, value, elapsed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--preset", choices=sorted(PRESETS), default=None)
    parser.add_argument("--atoms", type=int, default=2_000)
    parser.add_argument("--branches", type=int, default=20_000)
    parser.add_argument("--forced-cycles", type=int, default=16)
    parser.add_argument("--seed", type=lambda text: int(text, 0), default=0xA9C0FFEE)
    parser.add_argument(
        "--full-history",
        action="store_true",
        help="also run the expensive full-history oracle (keep workloads moderate)",
    )
    parser.add_argument("--output", type=Path, default=None)
    args = parser.parse_args()

    if args.preset:
        preset = PRESETS[args.preset]
        args.atoms = preset["atoms"]
        args.branches = preset["branches"]
        args.forced_cycles = preset["forced_cycles"]

    build_started = time.perf_counter()
    state, witnesses, counts = build_branch_storm(
        atom_count=args.atoms,
        branch_count=args.branches,
        forced_cycles=args.forced_cycles,
        seed=args.seed,
    )
    build_seconds = time.perf_counter() - build_started

    results: dict[str, object] = {
        "schema": "apc-hierarchy-bench-v1",
        "python": sys.version.split()[0],
        "platform": platform.platform(),
        "seed": args.seed,
        "workload": counts,
        "build_seconds": build_seconds,
        "policies": {},
    }

    label, root, elapsed = timed("root", lambda: resolve_with_root_fallback(state))
    results["policies"][label] = {
        "seconds": elapsed,
        "parent_digest": parent_digest(root.active_parents),
        "metrics": root.metrics.__dict__,
    }

    label, one, elapsed = timed(
        "one_witness",
        lambda: resolve_with_one_witness_fallback(
            state,
            witness_by_revision=witnesses,
        ),
    )
    results["policies"][label] = {
        "seconds": elapsed,
        "parent_digest": parent_digest(one.active_parents),
        "metrics": one.metrics.__dict__,
        "same_parent_graph_as_root": one.active_parents == root.active_parents,
    }

    if args.full_history:
        label, full, elapsed = timed("full_history", lambda: resolve_acyclic_with_metrics(state))
        results["policies"][label] = {
            "seconds": elapsed,
            "parent_digest": parent_digest(full.resolved.active_parents),
            "metrics": full.metrics.__dict__,
            "same_parent_graph_as_one_witness": (
                full.resolved.active_parents == one.active_parents
            ),
            "same_parent_graph_as_root": full.resolved.active_parents == root.active_parents,
        }

    encoded = json.dumps(results, indent=2, sort_keys=True)
    print(encoded)
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
