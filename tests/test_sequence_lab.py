from __future__ import annotations

import random
import unittest

from reference_model.apc_model import ModelError, OrderedCollection, Placement
from reference_model.sequence_lab import (
    FuguePosition,
    FuguePositionTree,
    MovableSequenceLab,
)


def hid(value: int) -> str:
    return f"{value:064x}"


class ConstraintSequenceCounterexamples(unittest.TestCase):
    def test_constraint_graph_can_interleave_two_independent_runs(self) -> None:
        base = OrderedCollection()
        base = base.insert(Placement(hid(100), hid(1)))
        base = base.insert(Placement(hid(101), hid(2), left_atom_id=hid(1)))

        left = base.insert(
            Placement(
                hid(200),
                hid(3),
                left_atom_id=hid(1),
                right_atom_id=hid(2),
            )
        )
        left = left.insert(
            Placement(
                hid(220),
                hid(4),
                left_atom_id=hid(3),
                right_atom_id=hid(2),
            )
        )

        right = base.insert(
            Placement(
                hid(210),
                hid(5),
                left_atom_id=hid(1),
                right_atom_id=hid(2),
            )
        )
        right = right.insert(
            Placement(
                hid(230),
                hid(6),
                left_atom_id=hid(5),
                right_atom_id=hid(2),
            )
        )

        merged = left.merge(right).materialize()
        self.assertEqual(
            merged,
            [hid(1), hid(3), hid(5), hid(4), hid(6), hid(2)],
        )


class FuguePositionTreeTests(unittest.TestCase):
    def make_ab(self) -> tuple[FuguePositionTree, str, str]:
        tree = FuguePositionTree()
        tree = tree.create_between(hid(100), None, None)
        tree = tree.create_between(hid(101), hid(100), None)
        return tree, hid(100), hid(101)

    def test_created_position_is_strictly_between_neighbors(self) -> None:
        tree, a, b = self.make_ab()
        tree = tree.create_between(hid(200), a, b)

        self.assertLess(tree.compare(a, hid(200)), 0)
        self.assertLess(tree.compare(hid(200), b), 0)

    def test_concurrent_inserted_runs_remain_contiguous(self) -> None:
        base, a, b = self.make_ab()

        left = base.create_between(hid(200), a, b)
        left = left.create_between(hid(220), hid(200), b)

        right = base.create_between(hid(210), a, b)
        right = right.create_between(hid(230), hid(210), b)

        merged = left.merge(right)
        middle = merged.sorted_positions(
            [hid(200), hid(220), hid(210), hid(230)]
        )

        left_indices = sorted(
            middle.index(item) for item in (hid(200), hid(220))
        )
        right_indices = sorted(
            middle.index(item) for item in (hid(210), hid(230))
        )

        self.assertEqual(left_indices[1] - left_indices[0], 1)
        self.assertEqual(right_indices[1] - right_indices[0], 1)

    def test_random_insertions_preserve_local_order_with_opaque_ids(self) -> None:
        rng = random.Random(0xA0C5EED)
        for _ in range(25):
            tree = FuguePositionTree()
            visible: list[str] = []
            for _ in range(100):
                index = rng.randrange(len(visible) + 1)
                left = visible[index - 1] if index > 0 else None
                right = visible[index] if index < len(visible) else None
                position_id = f"{rng.getrandbits(256):064x}"
                while position_id in tree.positions:
                    position_id = f"{rng.getrandbits(256):064x}"

                tree = tree.create_between(position_id, left, right)
                visible.insert(index, position_id)
                self.assertEqual(tree.sorted_positions(visible), visible)

    def test_state_union_converges_in_random_merge_order(self) -> None:
        base, a, b = self.make_ab()
        replicas = [
            base.create_between(hid(1000 + i), a, b)
            for i in range(64)
        ]

        expected = None
        rng = random.Random(0xF067E)
        for _ in range(20):
            order = list(replicas)
            rng.shuffle(order)
            merged = base
            for replica in order:
                merged = merged.merge(replica)
            actual = merged.sorted_positions(
                [hid(1000 + i) for i in range(64)]
            )
            if expected is None:
                expected = actual
            self.assertEqual(actual, expected)


class MovableSequenceTests(unittest.TestCase):
    def make_abc(self) -> MovableSequenceLab:
        seq = MovableSequenceLab()
        seq = seq.place(
            atom_id=hid(1),
            position_id=hid(101),
            revision_id=hid(1001),
        )
        seq = seq.place(
            atom_id=hid(2),
            position_id=hid(102),
            revision_id=hid(1002),
            left_atom_id=hid(1),
        )
        seq = seq.place(
            atom_id=hid(3),
            position_id=hid(103),
            revision_id=hid(1003),
            left_atom_id=hid(2),
        )
        self.assertEqual(seq.materialize(), [hid(1), hid(2), hid(3)])
        return seq

    def test_concurrent_moves_of_same_atom_do_not_duplicate_it(self) -> None:
        base = self.make_abc()

        left = base.place(
            atom_id=hid(2),
            position_id=hid(201),
            revision_id=hid(2001),
            left_atom_id=hid(3),
        )
        right = base.place(
            atom_id=hid(2),
            position_id=hid(202),
            revision_id=hid(2002),
            right_atom_id=hid(1),
        )

        merged = left.merge(right)
        materialized = merged.materialize()

        self.assertEqual(materialized.count(hid(2)), 1)
        self.assertEqual(
            merged.active_location_revision(hid(2)).revision_id,
            hid(2002),
        )
        self.assertEqual(right.merge(left).materialize(), materialized)

    def test_move_does_not_change_atom_identity_or_other_locations(self) -> None:
        base = self.make_abc()
        a_before = base._active_position(hid(1))
        c_before = base._active_position(hid(3))

        moved = base.place(
            atom_id=hid(2),
            position_id=hid(201),
            revision_id=hid(2001),
            left_atom_id=hid(3),
        )

        self.assertEqual(moved._active_position(hid(1)), a_before)
        self.assertEqual(moved._active_position(hid(3)), c_before)
        self.assertEqual(moved.materialize(), [hid(1), hid(3), hid(2)])

    def test_causal_delete_after_move_hides_atom(self) -> None:
        base = self.make_abc()
        moved = base.place(
            atom_id=hid(2),
            position_id=hid(201),
            revision_id=hid(2001),
            left_atom_id=hid(3),
        )
        deleted = moved.delete(atom_id=hid(2), revision_id=hid(2002))

        self.assertNotIn(hid(2), deleted.materialize())

    def test_concurrent_delete_move_is_deterministic_but_policy_is_provisional(self) -> None:
        base = self.make_abc()

        moved = base.place(
            atom_id=hid(2),
            position_id=hid(201),
            revision_id=hid(3000),
            left_atom_id=hid(3),
        )
        deleted = base.delete(atom_id=hid(2), revision_id=hid(4000))

        merged = moved.merge(deleted)
        self.assertNotIn(hid(2), merged.materialize())
        self.assertEqual(
            deleted.merge(moved).materialize(),
            merged.materialize(),
        )

    def test_position_tree_rejects_parent_cycle(self) -> None:
        tree = FuguePositionTree(
            {
                hid(1): FuguePosition(hid(1), hid(2), False),
                hid(2): FuguePosition(hid(2), hid(1), False),
            }
        )
        with self.assertRaises(ModelError):
            tree.validate()


if __name__ == "__main__":
    unittest.main()
