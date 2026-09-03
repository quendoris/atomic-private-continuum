from __future__ import annotations

import random
import unittest

from reference_model.apc_model import ModelError, ScalarRegister
from reference_model.causality_lab import FrontierCausalRegister


def hid(value: int) -> str:
    return f"{value:064x}"


class FrontierCausalityOracleTests(unittest.TestCase):
    def test_linear_history_matches_oracle_and_reduces_references(self) -> None:
        oracle = ScalarRegister()
        compact = FrontierCausalRegister()
        count = 256

        for index in range(1, count + 1):
            revision_id = hid(index)
            oracle = oracle.assign(revision_id, revision_id)
            compact = compact.assign(revision_id, revision_id)

        self.assertEqual(compact.materialized_value(), oracle.materialized_value())
        self.assertEqual(set(compact.frontier()), set(oracle.frontier()))

        explicit_references = sum(
            len(revision.context) for revision in oracle.revisions.values()
        )
        compact_references = compact.metrics().direct_reference_count

        self.assertEqual(explicit_references, count * (count - 1) // 2)
        self.assertEqual(compact_references, count - 1)

    def test_causal_successor_with_smaller_id_still_wins(self) -> None:
        high = hid(2**255)
        low = hid(7)

        oracle = ScalarRegister().assign(high, "old").assign(low, "new")
        compact = FrontierCausalRegister().assign(high, "old").assign(low, "new")

        self.assertEqual(oracle.materialized_value(), "new")
        self.assertEqual(compact.materialized_value(), "new")
        self.assertTrue(compact.is_ancestor(high, low))

    def test_concurrent_frontier_tie_break_matches_oracle(self) -> None:
        base_o = ScalarRegister().assign(hid(1), "base")
        base_c = FrontierCausalRegister().assign(hid(1), "base")

        left_o = base_o.assign(hid(10), "left")
        right_o = base_o.assign(hid(20), "right")
        left_c = base_c.assign(hid(10), "left")
        right_c = base_c.assign(hid(20), "right")

        merged_o = left_o.merge(right_o)
        merged_c = left_c.merge(right_c)

        self.assertEqual(set(merged_c.frontier()), set(merged_o.frontier()))
        self.assertEqual(merged_c.materialized_value(), merged_o.materialized_value())
        self.assertFalse(merged_c.is_ancestor(hid(10), hid(20)))
        self.assertFalse(merged_c.is_ancestor(hid(20), hid(10)))

    def test_random_replica_history_matches_explicit_oracle(self) -> None:
        rng = random.Random(0xA0C5EED)
        replica_count = 12

        seed_o = ScalarRegister().assign(hid(1), "seed")
        seed_c = FrontierCausalRegister().assign(hid(1), "seed")
        oracles = [seed_o.copy() for _ in range(replica_count)]
        compacts = [seed_c.copy() for _ in range(replica_count)]

        next_id = 1000
        for _ in range(1200):
            target = rng.randrange(replica_count)
            if rng.random() < 0.68:
                revision_id = hid(next_id)
                next_id += 1
                oracles[target] = oracles[target].assign(revision_id, revision_id)
                compacts[target] = compacts[target].assign(revision_id, revision_id)
            else:
                source = rng.randrange(replica_count)
                if source == target:
                    continue
                oracles[target] = oracles[target].merge(oracles[source])
                compacts[target] = compacts[target].merge(compacts[source])

            self.assertEqual(
                set(compacts[target].frontier()),
                set(oracles[target].frontier()),
            )
            self.assertEqual(
                compacts[target].materialized_value(),
                oracles[target].materialized_value(),
            )

        merged_o = seed_o
        merged_c = seed_c
        order = list(range(replica_count))
        rng.shuffle(order)
        for index in order:
            merged_o = merged_o.merge(oracles[index])
            merged_c = merged_c.merge(compacts[index])

        self.assertEqual(set(merged_c.frontier()), set(merged_o.frontier()))
        self.assertEqual(merged_c.materialized_value(), merged_o.materialized_value())

    def test_stale_state_cannot_roll_back_current_state(self) -> None:
        current_o = ScalarRegister()
        current_c = FrontierCausalRegister()
        stale_o = None
        stale_c = None

        for index in range(1, 80):
            revision_id = hid(index)
            current_o = current_o.assign(revision_id, revision_id)
            current_c = current_c.assign(revision_id, revision_id)
            if index == 20:
                stale_o = current_o.copy()
                stale_c = current_c.copy()

        assert stale_o is not None and stale_c is not None
        self.assertEqual(current_o.merge(stale_o).materialized_value(), hid(79))
        self.assertEqual(current_c.merge(stale_c).materialized_value(), hid(79))
        self.assertEqual(stale_c.merge(current_c), current_c)

    def test_many_concurrent_replicas_cost_scales_with_frontier_not_history(self) -> None:
        base_o = ScalarRegister().assign(hid(1), "base")
        base_c = FrontierCausalRegister().assign(hid(1), "base")

        branches_o = []
        branches_c = []
        replicas = 512
        for index in range(replicas):
            revision_id = hid(10_000 + index)
            branches_o.append(base_o.assign(revision_id, revision_id))
            branches_c.append(base_c.assign(revision_id, revision_id))

        merged_o = base_o
        merged_c = base_c
        for oracle, compact in zip(branches_o, branches_c):
            merged_o = merged_o.merge(oracle)
            merged_c = merged_c.merge(compact)

        self.assertEqual(len(merged_c.frontier()), replicas)
        self.assertEqual(set(merged_c.frontier()), set(merged_o.frontier()))

        join_id = hid(50_000)
        joined_o = merged_o.assign(join_id, "joined")
        joined_c = merged_c.assign(join_id, "joined")

        self.assertEqual(set(joined_c.frontier()), {join_id})
        self.assertEqual(joined_c.metrics().max_parent_count, replicas)
        self.assertEqual(joined_c.materialized_value(), joined_o.materialized_value())

        after_id = hid(50_001)
        after_c = joined_c.assign(after_id, "after")
        self.assertEqual(len(after_c.revisions[after_id].parents), 1)

    def test_baseline_delta_can_ship_only_missing_nodes(self) -> None:
        sender = FrontierCausalRegister()
        for index in range(1, 101):
            sender = sender.assign(hid(index), f"v{index}")

        receiver = FrontierCausalRegister()
        for index in range(1, 91):
            receiver = receiver.assign(hid(index), f"v{index}")

        delta = sender.missing_from(receiver.revisions)
        self.assertEqual(len(delta.revisions), 10)
        self.assertEqual(delta.external_parents, frozenset({hid(90)}))

        caught_up = receiver.apply_delta(delta)
        self.assertEqual(caught_up, sender)

    def test_delta_rejects_missing_baseline_dependency(self) -> None:
        sender = FrontierCausalRegister()
        for index in range(1, 6):
            sender = sender.assign(hid(index), str(index))

        delta = sender.missing_from({hid(1), hid(2), hid(3)})
        empty = FrontierCausalRegister()

        with self.assertRaises(ModelError):
            empty.apply_delta(delta)


if __name__ == "__main__":
    unittest.main()
