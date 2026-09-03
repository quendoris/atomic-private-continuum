from __future__ import annotations

import unittest

from reference_model.apc_model import ModelError
from reference_model.hierarchy_bounded_lab import (
    previous_parent_witnesses,
    resolve_with_one_witness_fallback,
    resolve_with_root_fallback,
)
from reference_model.hierarchy_lab import HierarchyLab
from reference_model.hierarchy_torture_lab import resolve_acyclic_with_metrics


def hid(value: int) -> str:
    return f"{value:064x}"


class HierarchyCausalWitnessTests(unittest.TestCase):
    def test_full_history_can_activate_an_unobserved_concurrent_alternative(self) -> None:
        """Historical fallback is not always causal intent preservation.

        B starts under P. Two replicas concurrently move B to Q and R from the
        same causal base. The Q move has the larger opaque RevisionId and is the
        current materialized placement. A concurrent Q -> B move makes only that
        current placement invalid.

        Full-history fallback rejects B -> Q and then materializes B -> R merely
        because R is the next concurrent register winner. But the Q move never
        observed the R move. The one-witness policy instead falls back to P, the
        parent that the rejected Q move actually observed before it was created.
        """

        b, p, q, r = hid(1), hid(2), hid(3), hid(4)

        base = HierarchyLab()
        for atom, revision in ((p, 100), (q, 101), (r, 102)):
            base = base.place(
                atom_id=atom,
                revision_id=hid(revision),
                parent_atom_id=None,
            )
        base = base.place(atom_id=b, revision_id=hid(500), parent_atom_id=p)

        move_to_q = base.place(atom_id=b, revision_id=hid(900), parent_atom_id=q)
        move_to_r = base.place(atom_id=b, revision_id=hid(800), parent_atom_id=r)
        q_back_to_b = base.place(atom_id=q, revision_id=hid(9_000), parent_atom_id=b)

        merged = move_to_q.merge(move_to_r).merge(q_back_to_b)
        full = resolve_acyclic_with_metrics(merged)
        witnesses = previous_parent_witnesses(merged)
        one = resolve_with_one_witness_fallback(
            merged,
            witness_by_revision=witnesses,
        )

        q_revision = merged.parents[b].revisions[hid(900)]
        self.assertNotIn(hid(800), q_revision.context)
        self.assertEqual(witnesses[hid(900)], p)

        self.assertEqual(full.resolved.active_parents[b], r)
        self.assertEqual(one.active_parents[b], p)
        self.assertEqual(one.fallback_modes[b], "witness")

    def test_full_history_can_exhaust_all_placements_while_bounded_policy_is_total(self) -> None:
        """Every historical parent of B can become invalid after merge.

        The current B -> Q placement is invalidated by Q -> B. Full-history then
        falls back to B -> P, but P -> B invalidates that foundational placement
        too. Rejecting it leaves no placement revision for B and the historical
        resolver has no defined result. One-witness instead tries P once and then
        takes its explicit safe root fallback.
        """

        b, p, q = hid(20), hid(21), hid(22)

        base = HierarchyLab()
        base = base.place(atom_id=p, revision_id=hid(800), parent_atom_id=None)
        base = base.place(atom_id=q, revision_id=hid(900), parent_atom_id=None)
        base = base.place(atom_id=b, revision_id=hid(100), parent_atom_id=p)

        current = base.place(atom_id=b, revision_id=hid(200), parent_atom_id=q)
        p_back = base.place(atom_id=p, revision_id=hid(8_000), parent_atom_id=b)
        q_back = base.place(atom_id=q, revision_id=hid(9_000), parent_atom_id=b)
        merged = current.merge(p_back).merge(q_back)

        with self.assertRaisesRegex(ModelError, "exhausted every placement"):
            resolve_acyclic_with_metrics(merged)

        one = resolve_with_one_witness_fallback(merged)
        root = resolve_with_root_fallback(merged)

        self.assertIsNone(one.active_parents[b])
        self.assertEqual(one.fallback_modes[b], "root")
        self.assertEqual(one.metrics.max_fallback_steps_per_atom, 2)
        self.assertIsNone(root.active_parents[b])

    def test_causal_witness_is_stable_under_merge_order(self) -> None:
        b, p, q, r = hid(10), hid(11), hid(12), hid(13)

        base = HierarchyLab()
        for atom, revision in ((p, 200), (q, 201), (r, 202)):
            base = base.place(atom_id=atom, revision_id=hid(revision), parent_atom_id=None)
        base = base.place(atom_id=b, revision_id=hid(600), parent_atom_id=p)

        left = base.place(atom_id=b, revision_id=hid(950), parent_atom_id=q)
        right = base.place(atom_id=b, revision_id=hid(850), parent_atom_id=r)
        invalidator = base.place(atom_id=q, revision_id=hid(9_500), parent_atom_id=b)

        first = left.merge(right).merge(invalidator)
        second = invalidator.merge(right).merge(left)

        first_witnesses = previous_parent_witnesses(first)
        second_witnesses = previous_parent_witnesses(second)
        first_resolved = resolve_with_one_witness_fallback(
            first,
            witness_by_revision=first_witnesses,
        )
        second_resolved = resolve_with_one_witness_fallback(
            second,
            witness_by_revision=second_witnesses,
        )

        self.assertEqual(first_witnesses[hid(950)], p)
        self.assertEqual(second_witnesses[hid(950)], p)
        self.assertEqual(first_resolved, second_resolved)


if __name__ == "__main__":
    unittest.main()
