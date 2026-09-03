from __future__ import annotations

import random
import unittest

from reference_model.apc_model import ModelError
from reference_model.sequence_adversarial import (
    DeleteWinsSequenceLab,
    sequence_metrics,
)
from reference_model.sequence_lab import FuguePositionTree, MovableSequenceLab


def hid(value: int) -> str:
    return f"{value:064x}"


def make_linear(count: int = 3) -> MovableSequenceLab:
    seq = MovableSequenceLab()
    for index in range(count):
        atom_id = hid(index + 1)
        visible = seq.materialize()
        left = visible[-1] if visible else None
        seq = seq.place(
            atom_id=atom_id,
            position_id=hid(100 + index),
            revision_id=hid(1000 + index),
            left_atom_id=left,
        )
    return seq


def move_to_index(
    seq: MovableSequenceLab,
    *,
    atom_id: str,
    index: int,
    position_id: str,
    revision_id: str,
) -> MovableSequenceLab:
    visible = seq.materialize()
    if atom_id not in visible:
        raise ModelError("test helper can only move a visible atom")

    remaining = [item for item in visible if item != atom_id]
    if not 0 <= index <= len(remaining):
        raise ModelError("test move index out of range")

    left = remaining[index - 1] if index > 0 else None
    right = remaining[index] if index < len(remaining) else None
    return seq.place(
        atom_id=atom_id,
        position_id=position_id,
        revision_id=revision_id,
        left_atom_id=left,
        right_atom_id=right,
    )


class AnchorConcurrencyResearch(unittest.TestCase):
    def test_insert_next_to_concurrently_moved_anchor_stays_at_old_position(self) -> None:
        """Executable counterexample: position anchors do not follow moved atoms."""

        base = make_linear(3)
        a, b, c, d = hid(1), hid(2), hid(3), hid(4)

        moved = base.place(
            atom_id=b,
            position_id=hid(5001),
            revision_id=hid(6001),
            left_atom_id=c,
        )
        self.assertEqual(moved.materialize(), [a, c, b])

        inserted = base.place(
            atom_id=d,
            position_id=hid(5002),
            revision_id=hid(6002),
            left_atom_id=b,
            right_atom_id=c,
        )
        self.assertEqual(inserted.materialize(), [a, b, d, c])

        merged = moved.merge(inserted)

        # D remains next to B's historical position rather than following B's
        # winning moved location. This converges, but its user semantics are an
        # open design question.
        self.assertEqual(merged.materialize(), [a, d, c, b])
        self.assertEqual(inserted.merge(moved).materialize(), merged.materialize())

    def test_insert_next_to_concurrently_deleted_anchor_survives_in_gap(self) -> None:
        base = DeleteWinsSequenceLab(sequence=make_linear(3))
        a, b, c, d = hid(1), hid(2), hid(3), hid(4)

        deleted = base.delete(atom_id=b)
        inserted = base.place(
            atom_id=d,
            position_id=hid(5100),
            revision_id=hid(6100),
            left_atom_id=b,
            right_atom_id=c,
        )

        merged = deleted.merge(inserted)
        self.assertEqual(merged.materialize(), [a, d, c])
        self.assertEqual(inserted.merge(deleted).materialize(), [a, d, c])

    def test_crossing_moves_converge_without_cycle_or_duplication(self) -> None:
        base = make_linear(4)
        a, b, c, d = hid(1), hid(2), hid(3), hid(4)

        left = base.place(
            atom_id=a,
            position_id=hid(5200),
            revision_id=hid(6200),
            left_atom_id=c,
            right_atom_id=d,
        )
        right = base.place(
            atom_id=c,
            position_id=hid(5201),
            revision_id=hid(6201),
            right_atom_id=a,
        )

        merged_lr = left.merge(right)
        merged_rl = right.merge(left)

        self.assertEqual(merged_lr.materialize(), merged_rl.materialize())
        self.assertEqual(len(merged_lr.materialize()), 4)
        self.assertEqual(set(merged_lr.materialize()), {a, b, c, d})


class LifecycleResearch(unittest.TestCase):
    def test_location_none_model_can_causally_resurrect_deleted_atom(self) -> None:
        """Counterexample showing why location and lifecycle are different domains."""

        base = make_linear(3)
        b, c = hid(2), hid(3)

        deleted = base.delete(atom_id=b, revision_id=hid(7000))
        self.assertNotIn(b, deleted.materialize())

        resurrected = deleted.place(
            atom_id=b,
            position_id=hid(7001),
            revision_id=hid(7002),
            left_atom_id=c,
        )

        # The provisional location=None encoding treats a later placement as a
        # normal causal successor, so the atom becomes visible again.
        self.assertIn(b, resurrected.materialize())

    def test_delete_wins_candidate_does_not_let_move_change_lifecycle(self) -> None:
        base = DeleteWinsSequenceLab(sequence=make_linear(3))
        b, c = hid(2), hid(3)
        deleted = base.delete(atom_id=b)

        with self.assertRaises(ModelError):
            deleted.place(
                atom_id=b,
                position_id=hid(7100),
                revision_id=hid(7101),
                left_atom_id=c,
            )

    def test_delete_wins_over_concurrent_move_independent_of_revision_order(self) -> None:
        base = DeleteWinsSequenceLab(sequence=make_linear(3))
        b, c = hid(2), hid(3)

        moved = base.place(
            atom_id=b,
            position_id=hid(7200),
            revision_id=hid(2**255),
            left_atom_id=c,
        )
        deleted = base.delete(atom_id=b)

        self.assertNotIn(b, moved.merge(deleted).materialize())
        self.assertNotIn(b, deleted.merge(moved).materialize())


class RandomizedMoveDeleteResearch(unittest.TestCase):
    def test_random_offline_move_delete_replicas_converge(self) -> None:
        rng = random.Random(0xA0C0FF1CE)
        base = make_linear(10)
        replicas: list[MovableSequenceLab] = []

        for replica_index in range(8):
            seq = base
            for step in range(30):
                visible = seq.materialize()
                if not visible:
                    break

                unique = replica_index * 1000 + step
                revision_id = hid(1_000_000 + unique)

                if len(visible) > 2 and rng.random() < 0.22:
                    atom_id = rng.choice(visible)
                    seq = seq.delete(atom_id=atom_id, revision_id=revision_id)
                    continue

                atom_id = rng.choice(visible)
                remaining_count = len(visible) - 1
                target_index = rng.randrange(remaining_count + 1)
                seq = move_to_index(
                    seq,
                    atom_id=atom_id,
                    index=target_index,
                    position_id=hid(2_000_000 + unique),
                    revision_id=revision_id,
                )

            replicas.append(seq)

        expected_state = None
        expected_visible = None
        for _ in range(20):
            order = list(replicas)
            rng.shuffle(order)
            merged = base
            for replica in order:
                merged = merged.merge(replica)

            if expected_state is None:
                expected_state = merged
                expected_visible = merged.materialize()
            else:
                self.assertEqual(merged, expected_state)
                self.assertEqual(merged.materialize(), expected_visible)


class MetadataGrowthResearch(unittest.TestCase):
    def test_repeated_moves_expose_linear_dead_positions_and_quadratic_context(self) -> None:
        seq = make_linear(3)
        b = hid(2)
        moves = 128

        for index in range(1, moves + 1):
            # Alternate front/end so the same logical atom is repeatedly moved
            # through different stable positions.
            target = 0 if index % 2 else 2
            seq = move_to_index(
                seq,
                atom_id=b,
                index=target,
                position_id=hid(3_000_000 + index),
                revision_id=hid(4_000_000 + index),
            )

        metrics = sequence_metrics(seq)
        self.assertEqual(metrics.position_count, 3 + moves)
        self.assertEqual(metrics.visible_atom_count, 3)
        self.assertEqual(metrics.location_revision_count, 3 + moves)
        self.assertEqual(metrics.dead_position_count, moves)

        # B begins with one revision. Move i observes all i prior revisions,
        # therefore the explicit reference model stores 1+2+...+moves links.
        self.assertEqual(metrics.causal_reference_count, moves * (moves + 1) // 2)

    def test_naive_fugue_tree_depth_can_grow_linearly_under_append(self) -> None:
        tree = FuguePositionTree()
        last = None
        count = 256
        for index in range(count):
            current = hid(5_000_000 + index)
            tree = tree.create_between(current, last, None)
            last = current

        self.assertEqual(tree.max_depth(), count)


if __name__ == "__main__":
    unittest.main()
