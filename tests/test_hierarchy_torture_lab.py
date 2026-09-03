from __future__ import annotations

import random
import unittest

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


class HierarchyTortureTests(unittest.TestCase):
    def test_thousand_atom_multi_replica_state_converges_across_merge_orders(self) -> None:
        """Mix sparse random moves with guaranteed concurrent cross-move cycles."""

        atom_count = 1000
        base = root_hierarchy(atom_count)
        rng = random.Random(0xA9C0FFEE)
        replicas: list[HierarchyLab] = []

        # Eight offline replicas make unrelated/sometimes repeated moves.  The
        # first 64 atoms are reserved so the forced cycle farm below cannot be
        # overwritten by these random branches.
        for replica_index in range(8):
            state = base
            for step in range(25):
                atom = rng.randrange(65, atom_count + 1)
                if rng.random() < 0.20:
                    parent = None
                else:
                    parent_number = rng.randrange(1, atom_count + 1)
                    while parent_number == atom:
                        parent_number = rng.randrange(1, atom_count + 1)
                    parent = hid(parent_number)
                state = state.place(
                    atom_id=hid(atom),
                    revision_id=hid(1_000_000 + replica_index * 1000 + step),
                    parent_atom_id=parent,
                )
            replicas.append(state)

        # Sixteen guaranteed 2-cycles, each created by two independent replicas.
        for atom in range(1, 33, 2):
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

        snapshots = []
        for _ in range(10):
            order = list(range(len(replicas)))
            rng.shuffle(order)
            merged = base
            for index in order:
                merged = merged.merge(replicas[index])

            traced = resolve_acyclic_with_metrics(merged)
            snapshots.append(
                (
                    tuple(sorted(traced.resolved.active_parents.items())),
                    tuple(sorted(traced.resolved.rejected_revision_ids)),
                    traced.metrics,
                )
            )

        self.assertEqual(len(set(snapshots)), 1)
        metrics = snapshots[0][2]
        self.assertEqual(metrics.atom_count, 1000)
        self.assertGreaterEqual(metrics.initial_cycle_count, 16)
        self.assertGreaterEqual(metrics.rejected_revision_count, 16)
        self.assertEqual(metrics.resolution_iterations, metrics.rejected_revision_count)
        self.assertGreaterEqual(metrics.max_fallback_depth, 1)

    def test_deep_repeated_fallback_can_be_linear_in_one_atom_history(self) -> None:
        """One rejected placement can reveal another cycle, repeatedly.

        B has 64 causal placements B->C1 ... B->C64.  Independent replicas make
        every Ci point back to B.  Only the newest B placement is initially in a
        cycle, but rejecting it reveals the previous cycle, and so on.
        """

        depth = 64
        base = root_hierarchy(depth + 1)
        b = hid(1)

        chain = base
        expected_rejected: set[str] = set()
        for index in range(1, depth + 1):
            revision_id = hid(1_000 + index)
            expected_rejected.add(revision_id)
            chain = chain.place(
                atom_id=b,
                revision_id=revision_id,
                parent_atom_id=hid(index + 1),
            )

        merged = chain
        for index in range(1, depth + 1):
            back_edge = base.place(
                atom_id=hid(index + 1),
                revision_id=hid(200_000 + index),
                parent_atom_id=b,
            )
            merged = merged.merge(back_edge)

        traced = resolve_acyclic_with_metrics(merged)

        self.assertEqual(traced.metrics.initial_cycle_count, 1)
        self.assertEqual(traced.metrics.resolution_iterations, depth)
        self.assertEqual(traced.metrics.rejected_revision_count, depth)
        self.assertEqual(traced.metrics.max_fallback_depth, depth)
        self.assertEqual(traced.metrics.total_fallback_steps, depth)
        self.assertEqual(traced.resolved.rejected_revision_ids, frozenset(expected_rejected))
        self.assertIsNone(traced.resolved.active_parents[b])

        # 64 superseded B placements plus 64 concurrent back-edge placements are
        # retained beyond the one base placement per atom in this oracle state.
        self.assertEqual(traced.metrics.historical_placement_revisions, depth * 2)

    def test_trace_is_independent_of_mapping_insertion_order(self) -> None:
        base = root_hierarchy(6)
        states = [
            base.place(atom_id=hid(1), revision_id=hid(900), parent_atom_id=hid(2)),
            base.place(atom_id=hid(2), revision_id=hid(800), parent_atom_id=hid(1)),
            base.place(atom_id=hid(3), revision_id=hid(700), parent_atom_id=hid(4)),
            base.place(atom_id=hid(4), revision_id=hid(600), parent_atom_id=hid(3)),
            base.place(atom_id=hid(5), revision_id=hid(500), parent_atom_id=hid(6)),
        ]

        merged = base
        for state in states:
            merged = merged.merge(state)

        reversed_mapping = HierarchyLab(
            parents=dict(reversed(list(merged.parents.items()))),
            deleted_atoms=merged.deleted_atoms,
        )

        left = resolve_acyclic_with_metrics(merged)
        right = resolve_acyclic_with_metrics(reversed_mapping)

        self.assertEqual(left.resolved, right.resolved)
        self.assertEqual(left.metrics, right.metrics)


if __name__ == "__main__":
    unittest.main()
