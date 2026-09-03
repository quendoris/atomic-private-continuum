from __future__ import annotations

import unittest

from reference_model.atomic_mutation_lab import (
    AtomicDomainState,
    AtomicMutationInbox,
    mutation_consistent,
)


def hid(value: int) -> str:
    return f"{value:064x}"


class AtomicMutationVisibilityTests(unittest.TestCase):
    def seeded(self) -> AtomicDomainState:
        state = AtomicDomainState()
        state.seed("left", hid(1), 60)
        state.seed("right", hid(2), 40)
        return state

    def test_partial_delivery_is_not_visible(self) -> None:
        base = self.seeded()
        mutation = base.build_mutation(
            hid(1000),
            {
                "left": (hid(1001), 50),
                "right": (hid(1002), 50),
            },
        )
        inbox = AtomicMutationInbox()
        inbox.announce(mutation.mutation_id, mutation.domains)

        inbox.receive(mutation.mutation_id, "left", mutation.members["left"])
        self.assertIsNone(inbox.complete(mutation.mutation_id))
        self.assertEqual(base.materialized_values(), {"left": 60, "right": 40})

        inbox.receive(mutation.mutation_id, "right", mutation.members["right"])
        complete = inbox.complete(mutation.mutation_id)
        assert complete is not None
        base.apply_complete(complete)
        self.assertEqual(base.materialized_values(), {"left": 50, "right": 50})

    def test_failed_member_validation_leaves_all_domains_unchanged(self) -> None:
        base = self.seeded()
        mutation = base.build_mutation(
            hid(2000),
            {
                "left": (hid(2001), 55),
                "right": (hid(2002), 45),
            },
        )

        # Remove one dependency from the receiver to force right-member failure.
        receiver = base.copy()
        receiver.domains["right"].revisions.clear()
        before = receiver.materialized_values()

        with self.assertRaises(Exception):
            receiver.apply_complete(mutation)

        self.assertEqual(receiver.materialized_values(), before)


class AtomicMutationConcurrencyTests(unittest.TestCase):
    def seeded(self) -> AtomicDomainState:
        state = AtomicDomainState()
        state.seed("A", hid(1), 0)
        state.seed("B", hid(2), 0)
        return state

    def test_independent_domain_merge_can_tear_concurrent_atomic_mutations(self) -> None:
        base = self.seeded()
        left = base.copy()
        right = base.copy()

        # Mutation X is intended to mean the tuple (1, 1).  Its A member has the
        # larger revision ID, while mutation Y has the larger B member ID.
        tx = left.build_mutation(
            hid(10_000),
            {
                "A": (hid(900), 1),
                "B": (hid(100), 1),
            },
        )
        ty = right.build_mutation(
            hid(20_000),
            {
                "A": (hid(200), 2),
                "B": (hid(800), 2),
            },
        )
        left.apply_complete(tx)
        right.apply_complete(ty)

        merged = left.merge_domains_independently(right)
        values = merged.materialized_values()

        self.assertEqual(values, {"A": 1, "B": 2})
        self.assertFalse(mutation_consistent(values, {"A": 1, "B": 1}))
        self.assertFalse(mutation_consistent(values, {"A": 2, "B": 2}))

    def test_overlapping_atomic_groups_expose_conflict_scope_problem(self) -> None:
        base = AtomicDomainState()
        for index, name in enumerate(("A", "B", "C"), start=1):
            base.seed(name, hid(index), 0)

        left = base.copy()
        right = base.copy()
        tx = left.build_mutation(
            hid(30_000),
            {
                "A": (hid(900), "X"),
                "B": (hid(100), "X"),
            },
        )
        ty = right.build_mutation(
            hid(40_000),
            {
                "B": (hid(800), "Y"),
                "C": (hid(700), "Y"),
            },
        )
        left.apply_complete(tx)
        right.apply_complete(ty)

        merged = left.merge_domains_independently(right).materialized_values()
        self.assertEqual(merged, {"A": "X", "B": "Y", "C": "Y"})

        # Y won the overlapping B conflict, yet X's A effect remains visible.
        # If X promised all-or-none semantics this is a torn transaction even
        # though each individual merge domain converged correctly.
        self.assertFalse(mutation_consistent(merged, {"A": "X", "B": "X"}))

    def test_atomicity_does_not_require_cross_domain_causal_parents_at_creation(self) -> None:
        base = self.seeded()
        tx = base.build_mutation(
            hid(50_000),
            {
                "A": (hid(501), 10),
                "B": (hid(502), 20),
            },
        )

        self.assertEqual(tx.members["A"].parents, frozenset({hid(1)}))
        self.assertEqual(tx.members["B"].parents, frozenset({hid(2)}))
        self.assertNotIn(hid(2), tx.members["A"].parents)
        self.assertNotIn(hid(1), tx.members["B"].parents)


if __name__ == "__main__":
    unittest.main()
