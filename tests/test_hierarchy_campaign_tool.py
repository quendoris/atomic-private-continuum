from __future__ import annotations

import unittest

from tools.run_hierarchy_campaign import campaign_summary, run_seed


class HierarchyCampaignToolTests(unittest.TestCase):
    def test_smoke_seed_and_summary(self) -> None:
        run = run_seed(
            atom_count=80,
            branch_count=400,
            forced_cycles=4,
            seed=0xC0FFEE,
            full_history=True,
        )

        self.assertEqual(run["workload"]["atoms"], 80)
        self.assertGreaterEqual(run["initial_cycle_count"], 4)
        self.assertGreaterEqual(run["one_vs_root_parent_difference_count"], 0)
        self.assertIn("full_history", run)
        self.assertIn("full_vs_one_parent_difference_count", run)

        summary = campaign_summary([run], forced_cycles=4)
        self.assertEqual(summary["run_count"], 1)
        self.assertEqual(summary["forced_cycles_per_run"], 4)
        self.assertIn("full_history_seconds", summary)


if __name__ == "__main__":
    unittest.main()
