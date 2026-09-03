from __future__ import annotations

import random
import unittest

from reference_model.apc_model import ModelError
from reference_model.causality_lab import FrontierCausalRegister
from reference_model.checkpoint_lab import ExactCoverageRegister


def hid(value: int) -> str:
    return f"{value:064x}"


class ExactCoverageCheckpointTests(unittest.TestCase):
    def linear(self, count: int) -> FrontierCausalRegister:
        register = FrontierCausalRegister()
        for index in range(1, count + 1):
            register = register.assign(hid(index), f"v{index}")
        return register

    def test_compaction_preserves_frontier_identity_and_materialization(self) -> None:
        full = self.linear(100)
        compact = ExactCoverageRegister.from_full(full)

        self.assertEqual(set(compact.frontier()), set(full.frontier()))
        self.assertEqual(compact.materialized_value(), full.materialized_value())
        self.assertEqual(set(compact.retained), {hid(100)})
        self.assertEqual(len(compact.covered_ids), 99)

    def test_fresh_checkpoint_id_substitution_would_be_semantically_unsafe(self) -> None:
        """The retained logical frontier ID must not be replaced by a checkpoint ID.

        A stale concurrent branch can be chosen so that its deterministic scalar
        tie-break differs depending on whether the real frontier ID or a fresh
        summary ID participates.  The executable assertion documents why
        checkpoint identity and logical revision identity are separate concepts.
        """

        current = FrontierCausalRegister().assign(hid(100), "current")
        stale = FrontierCausalRegister().assign(hid(50), "base").assign(hid(75), "stale")

        # In the real scalar rule 100 beats concurrent 75 by canonical ID.
        real_winner = current.merge(stale).materialized_value()
        self.assertEqual(real_winner, "current")

        # A made-up checkpoint ID 60 would incorrectly let stale 75 win.
        fake_checkpoint = FrontierCausalRegister().assign(hid(60), "current")
        fake_winner = fake_checkpoint.merge(stale).materialized_value()
        self.assertEqual(fake_winner, "stale")
        self.assertNotEqual(fake_winner, real_winner)

    def test_long_offline_branch_reconnects_through_covered_parent(self) -> None:
        current = self.linear(100)
        stale = self.linear(20).assign(hid(10_000), "offline-edit")

        full_merged = current.merge(stale)
        compact = ExactCoverageRegister.from_full(current)

        # Only the genuinely new offline revision is transported.  Its direct
        # parent R20 has been discarded from the active DAG but remains covered.
        offline_id = hid(10_000)
        compact_merged = compact.import_revisions(
            {offline_id: stale.revisions[offline_id]}
        )

        self.assertEqual(set(compact_merged.frontier()), set(full_merged.frontier()))
        self.assertEqual(
            compact_merged.materialized_value(), full_merged.materialized_value()
        )
        self.assertEqual(set(compact_merged.frontier()), {hid(100), offline_id})

    def test_many_stale_branches_match_full_oracle_in_random_delivery_order(self) -> None:
        rng = random.Random(0xC0A5A1)
        current = self.linear(200)
        full = current
        compact = ExactCoverageRegister.from_full(current)

        branches = []
        for offset in range(64):
            base_index = rng.randint(1, 199)
            stale = self.linear(base_index)
            branch_id = hid(100_000 + offset)
            stale = stale.assign(branch_id, f"branch-{offset}")
            full = full.merge(stale)
            branches.append(stale.revisions[branch_id])

        rng.shuffle(branches)
        for revision in branches:
            compact = compact.import_revisions([revision])

        self.assertEqual(set(compact.frontier()), set(full.frontier()))
        self.assertEqual(compact.materialized_value(), full.materialized_value())

        join_id = hid(900_000)
        full_joined = full.assign(join_id, "joined")
        compact_joined = compact.assign(join_id, "joined")
        self.assertEqual(set(compact_joined.frontier()), set(full_joined.frontier()))
        self.assertEqual(compact_joined.materialized_value(), "joined")

    def test_recompaction_keeps_current_frontier_but_coverage_still_grows(self) -> None:
        current = self.linear(256)
        compact = ExactCoverageRegister.from_full(current)

        first = compact.metrics()
        self.assertEqual(first.retained_revision_count, 1)
        self.assertEqual(first.covered_id_count, 255)
        self.assertEqual(first.exact_coverage_payload_bytes_at_256_bits, 255 * 32)

        for index in range(256, 512):
            compact = compact.assign(hid(10_000 + index), f"v{index}")

        compact = compact.compact()
        second = compact.metrics()
        self.assertEqual(second.retained_revision_count, 1)
        # 255 already covered + the previously retained frontier + 255 newly
        # dominated revisions = 511 covered opaque IDs.
        self.assertEqual(second.covered_id_count, 511)
        self.assertEqual(second.exact_coverage_payload_bytes_at_256_bits, 511 * 32)

    def test_dropping_coverage_makes_stale_branch_parent_unverifiable(self) -> None:
        current = self.linear(100)
        stale = self.linear(20).assign(hid(20_000), "offline")
        branch = stale.revisions[hid(20_000)]

        exact = ExactCoverageRegister.from_full(current)
        self.assertIn(hid(20), exact.covered_ids)
        exact.import_revisions([branch])  # valid

        blind = ExactCoverageRegister(
            retained=dict(exact.retained),
            covered_ids=frozenset(),
        )
        with self.assertRaises(ModelError):
            blind.import_revisions([branch])

    def test_unknown_historical_parent_is_never_guessed(self) -> None:
        compact = ExactCoverageRegister.from_full(self.linear(10))
        branch = self.linear(50).assign(hid(70_000), "not-from-this-baseline")

        with self.assertRaises(ModelError):
            compact.import_revisions([branch.revisions[hid(70_000)]])


if __name__ == "__main__":
    unittest.main()
