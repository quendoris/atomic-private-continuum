from __future__ import annotations

import random
import unittest

from reference_model.hierarchy_bounded_lab import (
    previous_parent_witnesses,
    resolve_with_one_witness_fallback,
    resolve_with_root_fallback,
)
from reference_model.hierarchy_lab import HierarchyLab
from reference_model.hierarchy_torture_lab import resolve_acyclic_with_metrics


def hid(value: int) -> str:
    return f"{value:064x}"


def root_hierarchy(atom_count: int) -> HierarchyLab:
    state = HierarchyLab()
    for atom in range(1, atom_count + 1):
        state = state.place(
            atom_id=hid(atom),
            revision_id=hid(100_000 + atom),
            parent_atom_id=None,
        )
    return state


class HierarchyBoundedFallbackTests(unittest.TestCase):
    def test_deep_history_attack_is_bounded(self) -> None:
        depth = 64
        base = root_hierarchy(depth + 1)
        b = hid(1)

        chain = base
        for index in range(1, depth + 1):
            chain = chain.place(
                atom_id=b,
                revision_id=hid(1_000 + index),
                parent_atom_id=hid(index + 1),
            )

        merged = chain
        for index in range(1, depth + 1):
            merged = merged.merge(
                base.place(
                    atom_id=hid(index + 1),
                    revision_id=hid(200_000 + index),
                    parent_atom_id=b,
                )
            )

        full = resolve_acyclic_with_metrics(merged)
        one = resolve_with_one_witness_fallback(merged)
        root = resolve_with_root_fallback(merged)

        self.assertEqual(full.metrics.resolution_iterations, depth)
        self.assertEqual(full.metrics.max_fallback_depth, depth)

        self.assertLessEqual(one.metrics.resolution_iterations, 2)
        self.assertLessEqual(one.metrics.max_fallback_steps_per_atom, 2)
        self.assertLessEqual(root.metrics.resolution_iterations, 1)
        self.assertLessEqual(root.metrics.max_fallback_steps_per_atom, 1)

        self.assertIsNone(full.resolved.active_parents[b])
        self.assertIsNone(one.active_parents[b])
        self.assertIsNone(root.active_parents[b])

    def test_one_witness_preserves_immediately_previous_parent_when_safe(self) -> None:
        # B moves P -> Q -> R.  A concurrent R -> B makes only the newest move
        # invalid.  Full history and one-witness both preserve Q; root fallback
        # intentionally loses that immediately previous placement intent.
        base = root_hierarchy(4)
        b, p, q, r = hid(1), hid(2), hid(3), hid(4)

        chain = base.place(atom_id=b, revision_id=hid(500), parent_atom_id=p)
        chain = chain.place(atom_id=b, revision_id=hid(600), parent_atom_id=q)
        chain = chain.place(atom_id=b, revision_id=hid(700), parent_atom_id=r)
        merged = chain.merge(
            base.place(atom_id=r, revision_id=hid(9_000), parent_atom_id=b)
        )

        full = resolve_acyclic_with_metrics(merged)
        one = resolve_with_one_witness_fallback(merged)
        root = resolve_with_root_fallback(merged)

        self.assertEqual(full.resolved.active_parents[b], q)
        self.assertEqual(one.active_parents[b], q)
        self.assertIsNone(root.active_parents[b])
        self.assertEqual(one.fallback_modes[b], "witness")

    def test_one_witness_falls_to_root_when_witness_is_also_invalid(self) -> None:
        # B moves P -> Q -> R, while independent branches make both Q and R
        # children of B.  Full history can walk back to P.  One-witness tries Q
        # once, then takes the bounded root fallback instead of walking history.
        base = root_hierarchy(4)
        b, p, q, r = hid(1), hid(2), hid(3), hid(4)

        chain = base.place(atom_id=b, revision_id=hid(500), parent_atom_id=p)
        chain = chain.place(atom_id=b, revision_id=hid(600), parent_atom_id=q)
        chain = chain.place(atom_id=b, revision_id=hid(700), parent_atom_id=r)
        merged = chain
        merged = merged.merge(
            base.place(atom_id=q, revision_id=hid(8_000), parent_atom_id=b)
        )
        merged = merged.merge(
            base.place(atom_id=r, revision_id=hid(9_000), parent_atom_id=b)
        )

        full = resolve_acyclic_with_metrics(merged)
        one = resolve_with_one_witness_fallback(merged)

        self.assertEqual(full.resolved.active_parents[b], p)
        self.assertIsNone(one.active_parents[b])
        self.assertEqual(one.fallback_modes[b], "root")
        self.assertEqual(one.metrics.max_fallback_steps_per_atom, 2)

    def test_witness_is_the_value_observed_before_the_move(self) -> None:
        base = root_hierarchy(3)
        b, p, q = hid(1), hid(2), hid(3)
        state = base.place(atom_id=b, revision_id=hid(500), parent_atom_id=p)
        state = state.place(atom_id=b, revision_id=hid(600), parent_atom_id=q)

        witnesses = previous_parent_witnesses(state)
        self.assertEqual(witnesses[hid(600)], p)

    def test_bounded_policies_are_merge_order_independent(self) -> None:
        base = root_hierarchy(80)
        rng = random.Random(0xB0A1DED)
        replicas: list[HierarchyLab] = []

        # Sparse random branches.
        for replica_index in range(6):
            state = base
            for step in range(12):
                atom = rng.randrange(17, 81)
                parent_number = rng.randrange(1, 81)
                while parent_number == atom:
                    parent_number = rng.randrange(1, 81)
                state = state.place(
                    atom_id=hid(atom),
                    revision_id=hid(1_000_000 + replica_index * 100 + step),
                    parent_atom_id=hid(parent_number),
                )
            replicas.append(state)

        # Eight guaranteed cross-move cycles.
        for atom in range(1, 17, 2):
            partner = atom + 1
            replicas.append(
                base.place(
                    atom_id=hid(atom),
                    revision_id=hid(2_000_000 + atom),
                    parent_atom_id=hid(partner),
                )
            )
            replicas.append(
                base.place(
                    atom_id=hid(partner),
                    revision_id=hid(2_000_000 + partner),
                    parent_atom_id=hid(atom),
                )
            )

        one_snapshots = []
        root_snapshots = []
        for _ in range(12):
            order = list(range(len(replicas)))
            rng.shuffle(order)
            merged = base
            for index in order:
                merged = merged.merge(replicas[index])

            one = resolve_with_one_witness_fallback(merged)
            root = resolve_with_root_fallback(merged)
            one_snapshots.append(
                (
                    tuple(sorted(one.active_parents.items())),
                    tuple(sorted(one.rejected_current_revision_ids)),
                    one.metrics,
                )
            )
            root_snapshots.append(
                (
                    tuple(sorted(root.active_parents.items())),
                    tuple(sorted(root.rejected_current_revision_ids)),
                    root.metrics,
                )
            )

        self.assertEqual(len(set(one_snapshots)), 1)
        self.assertEqual(len(set(root_snapshots)), 1)


if __name__ == "__main__":
    unittest.main()
