from __future__ import annotations

import random
import unittest

from reference_model.hierarchy_lab import HierarchyLab
from reference_model.hierarchy_validity_lab import (
    resolve_acyclic_by_rejecting_lowest_cycle_revision,
)


def hid(value: int) -> str:
    return f"{value:064x}"


class HierarchyValidityTests(unittest.TestCase):
    def test_two_node_cross_move_falls_back_deterministically(self) -> None:
        base = HierarchyLab()
        base = base.place(atom_id=hid(10), revision_id=hid(100), parent_atom_id=None)
        base = base.place(atom_id=hid(11), revision_id=hid(101), parent_atom_id=None)

        left = base.place(atom_id=hid(10), revision_id=hid(800), parent_atom_id=hid(11))
        right = base.place(atom_id=hid(11), revision_id=hid(900), parent_atom_id=hid(10))
        merged = left.merge(right)

        resolved = resolve_acyclic_by_rejecting_lowest_cycle_revision(merged)

        self.assertEqual(resolved.rejected_revision_ids, frozenset({hid(800)}))
        self.assertEqual(resolved.active_parents[hid(10)], None)
        self.assertEqual(resolved.active_parents[hid(11)], hid(10))
        self.assertEqual(resolved.active_revision_ids[hid(10)], hid(100))
        self.assertEqual(resolved.active_revision_ids[hid(11)], hid(900))

    def test_three_node_cycle_rejects_one_edge_and_keeps_rest(self) -> None:
        base = HierarchyLab()
        for atom, revision in ((20, 200), (21, 201), (22, 202)):
            base = base.place(atom_id=hid(atom), revision_id=hid(revision), parent_atom_id=None)

        a = base.place(atom_id=hid(20), revision_id=hid(900), parent_atom_id=hid(21))
        b = base.place(atom_id=hid(21), revision_id=hid(800), parent_atom_id=hid(22))
        c = base.place(atom_id=hid(22), revision_id=hid(700), parent_atom_id=hid(20))
        merged = a.merge(b).merge(c)

        resolved = resolve_acyclic_by_rejecting_lowest_cycle_revision(merged)

        self.assertEqual(resolved.rejected_revision_ids, frozenset({hid(700)}))
        self.assertEqual(resolved.active_parents[hid(20)], hid(21))
        self.assertEqual(resolved.active_parents[hid(21)], hid(22))
        self.assertEqual(resolved.active_parents[hid(22)], None)

    def test_resolution_is_independent_of_replica_merge_order(self) -> None:
        base = HierarchyLab()
        for atom, revision in ((30, 300), (31, 301), (32, 302), (33, 303)):
            base = base.place(atom_id=hid(atom), revision_id=hid(revision), parent_atom_id=None)

        replicas = [
            base.place(atom_id=hid(30), revision_id=hid(910), parent_atom_id=hid(31)),
            base.place(atom_id=hid(31), revision_id=hid(920), parent_atom_id=hid(32)),
            base.place(atom_id=hid(32), revision_id=hid(930), parent_atom_id=hid(30)),
            base.place(atom_id=hid(33), revision_id=hid(940), parent_atom_id=hid(30)),
        ]

        rng = random.Random(0xA11C1E)
        snapshots = []
        for _ in range(20):
            order = list(range(len(replicas)))
            rng.shuffle(order)
            merged = base
            for index in order:
                merged = merged.merge(replicas[index])
            resolved = resolve_acyclic_by_rejecting_lowest_cycle_revision(merged)
            snapshots.append(
                (
                    tuple(sorted(resolved.active_parents.items())),
                    tuple(sorted(resolved.rejected_revision_ids)),
                )
            )

        self.assertEqual(len(set(snapshots)), 1)

    def test_causal_successor_can_replace_a_previously_rejected_move(self) -> None:
        base = HierarchyLab()
        base = base.place(atom_id=hid(40), revision_id=hid(400), parent_atom_id=None)
        base = base.place(atom_id=hid(41), revision_id=hid(401), parent_atom_id=None)

        left = base.place(atom_id=hid(40), revision_id=hid(800), parent_atom_id=hid(41))
        right = base.place(atom_id=hid(41), revision_id=hid(900), parent_atom_id=hid(40))
        merged = left.merge(right)

        first = resolve_acyclic_by_rejecting_lowest_cycle_revision(merged)
        self.assertIn(hid(800), first.rejected_revision_ids)

        # A later same-atom placement causally supersedes revision 800. Its ID is
        # intentionally smaller than both cycle moves; causal successor semantics
        # inside the register still make it active before validity resolution.
        updated = merged.place(
            atom_id=hid(40),
            revision_id=hid(50),
            parent_atom_id=None,
        )
        second = resolve_acyclic_by_rejecting_lowest_cycle_revision(updated)

        self.assertEqual(second.rejected_revision_ids, frozenset())
        self.assertEqual(second.active_revision_ids[hid(40)], hid(50))
        self.assertEqual(second.active_parents[hid(40)], None)
        self.assertEqual(second.active_parents[hid(41)], hid(40))

    def test_fallback_requires_retained_historical_placement(self) -> None:
        base = HierarchyLab()
        base = base.place(atom_id=hid(50), revision_id=hid(500), parent_atom_id=None)
        base = base.place(atom_id=hid(51), revision_id=hid(501), parent_atom_id=None)
        left = base.place(atom_id=hid(50), revision_id=hid(800), parent_atom_id=hid(51))
        right = base.place(atom_id=hid(51), revision_id=hid(900), parent_atom_id=hid(50))
        merged = left.merge(right)

        resolved = resolve_acyclic_by_rejecting_lowest_cycle_revision(merged)
        self.assertEqual(resolved.active_revision_ids[hid(50)], hid(500))
        self.assertGreater(len(merged.parents[hid(50)].revisions), 1)


if __name__ == "__main__":
    unittest.main()
