from __future__ import annotations

import copy
import itertools
import random
import unittest

from reference_model.apc_model import (
    Atom,
    ContinuumState,
    ModelError,
    OrderedCollection,
    Placement,
    ScalarRegister,
    logical_snapshot,
)


def hid(value: int) -> str:
    return f"{value:064x}"


class ScalarRegisterTests(unittest.TestCase):
    def test_causal_precedence_beats_id_order(self) -> None:
        base = ScalarRegister().assign(hid(100), "draft")
        later = base.assign(hid(1), "final")
        self.assertEqual(later.materialized_value(), "final")

    def test_concurrent_scalar_tie_break_is_id_based(self) -> None:
        base = ScalarRegister().assign(hid(1), "draft")
        left = base.assign(hid(10), "alpha")
        right = base.assign(hid(20), "beta")

        merged_lr = left.merge(right)
        merged_rl = right.merge(left)

        self.assertEqual(merged_lr.materialized_value(), "beta")
        self.assertEqual(merged_rl.materialized_value(), "beta")
        self.assertEqual(set(merged_lr.frontier()), {hid(10), hid(20)})

    def test_later_causal_edit_with_smaller_id_still_wins(self) -> None:
        base = ScalarRegister().assign(hid(1), "draft")
        left = base.assign(hid(100), "alpha")
        right = base.assign(hid(200), "beta")
        merged = left.merge(right)

        later = merged.assign(hid(2), "gamma")

        self.assertEqual(later.materialized_value(), "gamma")
        self.assertEqual(set(later.frontier()), {hid(2)})

    def test_duplicate_merge_is_idempotent(self) -> None:
        state = ScalarRegister().assign(hid(1), "a").assign(hid(2), "b")
        merged = state.merge(state)
        self.assertEqual(merged.revisions, state.revisions)


class OrderedCollectionTests(unittest.TestCase):
    def make_ab(self) -> OrderedCollection:
        sequence = OrderedCollection()
        sequence = sequence.insert(Placement(hid(100), hid(1)))
        sequence = sequence.insert(
            Placement(hid(101), hid(2), left_atom_id=hid(1))
        )
        return sequence

    def test_concurrent_insertions_in_same_gap_are_preserved(self) -> None:
        base = self.make_ab()
        left = base.insert(
            Placement(
                hid(200),
                hid(3),
                left_atom_id=hid(1),
                right_atom_id=hid(2),
            )
        )
        right = base.insert(
            Placement(
                hid(201),
                hid(4),
                left_atom_id=hid(1),
                right_atom_id=hid(2),
            )
        )

        merged = left.merge(right)
        self.assertEqual(merged.materialize(), [hid(1), hid(3), hid(4), hid(2)])
        self.assertEqual(
            right.merge(left).materialize(),
            merged.materialize(),
        )

    def test_many_concurrent_insertions_converge_under_random_merge_order(self) -> None:
        base = self.make_ab()
        replicas = []
        for index in range(128):
            replicas.append(
                base.insert(
                    Placement(
                        hid(1000 + index),
                        hid(10000 + index),
                        left_atom_id=hid(1),
                        right_atom_id=hid(2),
                    )
                )
            )

        expected = None
        rng = random.Random(0xA0C)
        for _ in range(20):
            order = list(replicas)
            rng.shuffle(order)
            merged = base
            for replica in order:
                merged = merged.merge(replica)
            materialized = merged.materialize()
            if expected is None:
                expected = materialized
            self.assertEqual(materialized, expected)

        self.assertEqual(len(expected or []), 130)
        self.assertEqual((expected or [None])[0], hid(1))
        self.assertEqual((expected or [None])[-1], hid(2))

    def test_cycle_is_rejected(self) -> None:
        cyclic = OrderedCollection(
            {
                hid(10): Placement(hid(10), hid(1), left_atom_id=hid(2)),
                hid(20): Placement(hid(20), hid(2), left_atom_id=hid(1)),
            }
        )
        with self.assertRaises(ModelError):
            cyclic.materialize()


class StateMergeTests(unittest.TestCase):
    def make_states(self) -> tuple[ContinuumState, ContinuumState]:
        continuum_id = hid(9000)
        sticker_id = hid(9001)

        base_atom = Atom(
            atom_id=sticker_id,
            type_id="sticker",
            scalars={"title": ScalarRegister().assign(hid(1), "Old")},
            collections={
                "children": OrderedCollection().insert(
                    Placement(hid(100), hid(10))
                )
            },
        )

        left = ContinuumState(continuum_id, {sticker_id: copy.deepcopy(base_atom)})
        right = ContinuumState(continuum_id, {sticker_id: copy.deepcopy(base_atom)})

        left.atoms[sticker_id].scalars["title"] = (
            left.atoms[sticker_id].scalars["title"].assign(hid(2), "New")
        )
        right.atoms[sticker_id].collections["children"] = (
            right.atoms[sticker_id].collections["children"].insert(
                Placement(hid(101), hid(11), left_atom_id=hid(10))
            )
        )
        return left, right

    def test_field_independence(self) -> None:
        left, right = self.make_states()
        merged = left.merge(right)
        sticker = next(iter(merged.atoms.values()))

        self.assertEqual(sticker.scalars["title"].materialized_value(), "New")
        self.assertEqual(sticker.collections["children"].materialize(), [hid(10), hid(11)])

    def test_merge_algebra_on_reference_states(self) -> None:
        left, right = self.make_states()
        combined = left.merge(right)
        states = [left, right, combined]

        for state in states:
            self.assertEqual(
                logical_snapshot(state.merge(state)),
                logical_snapshot(state),
            )

        for a, b in itertools.product(states, repeat=2):
            self.assertEqual(
                logical_snapshot(a.merge(b)),
                logical_snapshot(b.merge(a)),
            )

        for a, b, c in itertools.product(states, repeat=3):
            self.assertEqual(
                logical_snapshot(a.merge(b).merge(c)),
                logical_snapshot(a.merge(b.merge(c))),
            )

    def test_continuum_mismatch_is_rejected(self) -> None:
        left = ContinuumState(hid(1))
        right = ContinuumState(hid(2))
        with self.assertRaises(ModelError):
            left.merge(right)


if __name__ == "__main__":
    unittest.main()
