#!/usr/bin/env python3
"""Multi-seed workstation campaign for A.P.C. hierarchy research.

Normal CI should keep using semantic unit tests and the tiny smoke benchmark. This
runner is for local statistical work over many deterministic branch storms.

Wall-clock measurements are benchmark metadata only and never participate in
A.P.C. merge semantics.
"""

from __future__ import annotations

import argparse
import gc
import json
from pathlib import Path
import platform
import statistics
import sys
import time
from typing import Any, Optional

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from reference_model.apc_model import ModelError  # noqa: E402
from reference_model.hierarchy_bounded_lab import (  # noqa: E402
    resolve_with_one_witness_fallback,
    resolve_with_root_fallback,
)
from reference_model.hierarchy_torture_lab import resolve_acyclic_with_metrics  # noqa: E402
from tools.run_hierarchy_bench import build_branch_storm  # noqa: E402


PRESETS = {
    "smoke": dict(atoms=500, branches=5_000, forced_cycles=4, seeds=3, full_history=True),
    "medium": dict(atoms=20_000, branches=200_000, forced_cycles=32, seeds=16, full_history=False),
    "large": dict(atoms=100_000, branches=1_000_000, forced_cycles=64, seeds=16, full_history=False),
    "oracle": dict(atoms=5_000, branches=50_000, forced_cycles=32, seeds=32, full_history=True),
}


def parent_difference_count(
    left: dict[str, Optional[str]],
    right: dict[str, Optional[str]],
) -> int:
    if left.keys() != right.keys():
        raise ValueError("cannot compare parent graphs with different atom sets")
    return sum(1 for atom_id in left if left[atom_id] != right[atom_id])


def percentile95(values: list[float]) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, (95 * len(ordered) + 99) // 100 - 1))
    return ordered[index]


def summarize_numeric(values: list[float]) -> dict[str, float]:
    if not values:
        return {"min": 0.0, "median": 0.0, "p95": 0.0, "max": 0.0, "mean": 0.0}
    return {
        "min": min(values),
        "median": statistics.median(values),
        "p95": percentile95(values),
        "max": max(values),
        "mean": statistics.fmean(values),
    }


def run_seed(
    *,
    atom_count: int,
    branch_count: int,
    forced_cycles: int,
    seed: int,
    full_history: bool,
) -> dict[str, Any]:
    started = time.perf_counter()
    state, witnesses, counts = build_branch_storm(
        atom_count=atom_count,
        branch_count=branch_count,
        forced_cycles=forced_cycles,
        seed=seed,
    )
    build_seconds = time.perf_counter() - started

    started = time.perf_counter()
    root = resolve_with_root_fallback(state)
    root_seconds = time.perf_counter() - started

    started = time.perf_counter()
    one = resolve_with_one_witness_fallback(
        state,
        witness_by_revision=witnesses,
    )
    one_seconds = time.perf_counter() - started

    result: dict[str, Any] = {
        "seed": seed,
        "workload": counts,
        "build_seconds": build_seconds,
        "initial_cycle_count": root.metrics.initial_cycle_count,
        "spontaneous_cycle_count": max(
            0,
            root.metrics.initial_cycle_count - forced_cycles,
        ),
        "root": {
            "seconds": root_seconds,
            "metrics": root.metrics.__dict__,
        },
        "one_witness": {
            "seconds": one_seconds,
            "metrics": one.metrics.__dict__,
        },
        "one_vs_root_parent_difference_count": parent_difference_count(
            one.active_parents,
            root.active_parents,
        ),
    }

    if full_history:
        started = time.perf_counter()
        try:
            full = resolve_acyclic_with_metrics(state)
        except ModelError as exc:
            result["full_history"] = {
                "status": "model_error",
                "seconds": time.perf_counter() - started,
                "error": str(exc),
            }
        else:
            full_seconds = time.perf_counter() - started
            result["full_history"] = {
                "status": "ok",
                "seconds": full_seconds,
                "metrics": full.metrics.__dict__,
            }
            result["full_vs_one_parent_difference_count"] = parent_difference_count(
                full.resolved.active_parents,
                one.active_parents,
            )
            result["full_vs_root_parent_difference_count"] = parent_difference_count(
                full.resolved.active_parents,
                root.active_parents,
            )

    return result


def campaign_summary(runs: list[dict[str, Any]], *, forced_cycles: int) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "run_count": len(runs),
        "initial_cycles": summarize_numeric(
            [float(run["initial_cycle_count"]) for run in runs]
        ),
        "spontaneous_cycles": summarize_numeric(
            [float(run["spontaneous_cycle_count"]) for run in runs]
        ),
        "build_seconds": summarize_numeric(
            [float(run["build_seconds"]) for run in runs]
        ),
        "root_seconds": summarize_numeric(
            [float(run["root"]["seconds"]) for run in runs]
        ),
        "one_witness_seconds": summarize_numeric(
            [float(run["one_witness"]["seconds"]) for run in runs]
        ),
        "one_vs_root_parent_differences": summarize_numeric(
            [float(run["one_vs_root_parent_difference_count"]) for run in runs]
        ),
        "one_witness_root_fallbacks": summarize_numeric(
            [
                float(run["one_witness"]["metrics"]["root_fallback_count"])
                for run in runs
            ]
        ),
        "forced_cycles_per_run": forced_cycles,
    }

    full_runs = [run for run in runs if "full_history" in run]
    if full_runs:
        successful = [
            run for run in full_runs if run["full_history"].get("status") == "ok"
        ]
        failures = [
            run for run in full_runs if run["full_history"].get("status") != "ok"
        ]
        summary["full_history_run_count"] = len(full_runs)
        summary["full_history_success_count"] = len(successful)
        summary["full_history_model_error_count"] = len(failures)
        summary["full_history_seconds"] = summarize_numeric(
            [float(run["full_history"]["seconds"]) for run in full_runs]
        )
        summary["full_vs_one_parent_differences"] = summarize_numeric(
            [float(run["full_vs_one_parent_difference_count"]) for run in successful]
        )
        summary["full_vs_root_parent_differences"] = summarize_numeric(
            [float(run["full_vs_root_parent_difference_count"]) for run in successful]
        )

    return summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--preset", choices=sorted(PRESETS), default="medium")
    parser.add_argument("--atoms", type=int, default=None)
    parser.add_argument("--branches", type=int, default=None)
    parser.add_argument("--forced-cycles", type=int, default=None)
    parser.add_argument("--seeds", type=int, default=None)
    parser.add_argument("--seed-base", type=lambda text: int(text, 0), default=0xA9C0FFEE)
    parser.add_argument("--full-history", action="store_true")
    parser.add_argument("--no-full-history", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    preset = dict(PRESETS[args.preset])
    atom_count = args.atoms if args.atoms is not None else preset["atoms"]
    branch_count = args.branches if args.branches is not None else preset["branches"]
    forced_cycles = (
        args.forced_cycles if args.forced_cycles is not None else preset["forced_cycles"]
    )
    seed_count = args.seeds if args.seeds is not None else preset["seeds"]
    full_history = bool(preset["full_history"])
    if args.full_history:
        full_history = True
    if args.no_full_history:
        full_history = False
    if args.full_history and args.no_full_history:
        parser.error("--full-history and --no-full-history are mutually exclusive")
    if seed_count <= 0:
        parser.error("--seeds must be positive")

    runs: list[dict[str, Any]] = []
    campaign_started = time.perf_counter()

    for index in range(seed_count):
        seed = args.seed_base + index
        print(
            f"[{index + 1}/{seed_count}] seed={seed:#x} "
            f"atoms={atom_count} branches={branch_count}",
            flush=True,
        )
        run = run_seed(
            atom_count=atom_count,
            branch_count=branch_count,
            forced_cycles=forced_cycles,
            seed=seed,
            full_history=full_history,
        )
        runs.append(run)
        full_status = ""
        if "full_history" in run:
            full_status = f" full={run['full_history'].get('status', 'unknown')}"
        print(
            "  cycles={cycles} spontaneous={spontaneous} "
            "one-vs-root={diff} one-root-fallbacks={root_fallbacks}{full_status}".format(
                cycles=run["initial_cycle_count"],
                spontaneous=run["spontaneous_cycle_count"],
                diff=run["one_vs_root_parent_difference_count"],
                root_fallbacks=run["one_witness"]["metrics"]["root_fallback_count"],
                full_status=full_status,
            ),
            flush=True,
        )
        gc.collect()

    result = {
        "schema": "apc-hierarchy-campaign-v1",
        "python": sys.version.split()[0],
        "platform": platform.platform(),
        "preset": args.preset,
        "configuration": {
            "atoms": atom_count,
            "branches": branch_count,
            "forced_cycles": forced_cycles,
            "seed_count": seed_count,
            "seed_base": args.seed_base,
            "full_history": full_history,
        },
        "campaign_seconds": time.perf_counter() - campaign_started,
        "summary": campaign_summary(runs, forced_cycles=forced_cycles),
        "runs": runs,
    }

    encoded = json.dumps(result, indent=2, sort_keys=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
