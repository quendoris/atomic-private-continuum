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
            full_history=False,
        )

        self.assertEqual(run["workload"]["atoms"], 80)
        self.assertGreaterEqual(run["initial_cycle_count"], 4)
        self.assertGreaterEqual(run["one_vs_root_parent_difference_count"], 0)

        summary = campaign_summary([run], forced_cycles=4)
        self.assertEqual(summary["run_count"], 1)
        self.assertEqual(summary["forced_cycles_per_run"], 4)

    def test_campaign_records_full_history_exhaustion_instead_of_aborting(self) -> None:
        # This deterministic branch storm makes the full-history validity oracle
        # exhaust every retained placement for one atom. Bounded policies still
        # return a valid result, so a statistical campaign must record the oracle
        # failure instead of losing the rest of the seeds.
        run = run_seed(
            atom_count=80,
            branch_count=400,
            forced_cycles=4,
            seed=0xC0FFEE,
            full_history=True,
        )

        self.assertEqual(run["full_history"]["status"], "model_error")
        self.assertIn("exhausted every placement", run["full_history"]["error"])

        summary = campaign_summary([run], forced_cycles=4)
        self.assertEqual(summary["full_history_run_count"], 1)
        self.assertEqual(summary["full_history_success_count"], 0)
        self.assertEqual(summary["full_history_model_error_count"], 1)


if __name__ == "__main__":
    unittest.main()
