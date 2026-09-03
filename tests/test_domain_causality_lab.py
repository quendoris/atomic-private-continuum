from __future__ import annotations

import unittest

from reference_model.apc_model import ModelError
from reference_model.causality_lab import FrontierCausalRegister
from reference_model.domain_causality_lab import (
    DomainWorkingSet,
    naive_global_observation_boundary,
)


def hid(value: int) -> str:
    return f"{value:064x}"


class DomainLocalObservationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.base_a = FrontierCausalRegister().assign(hid(1), "A0")
        self.base_b = FrontierCausalRegister().assign(hid(2), "B0")

    def test_remote_change_in_other_domain_does_not_seal_dirty_domain(self) -> None:
        working = DomainWorkingSet()
        working.ensure_domain("A", self.base_a)
        working.ensure_domain("B", self.base_b)
        working.durable_edit("A", "A-local")

        remote_b = self.base_b.assign(hid(100), "B-remote")
        working.observe_remote_domain("B", remote_b)

        self.assertTrue(working.domains["A"].dirty)
        self.assertEqual(working.domains["A"].locally_created_causal_revisions, 0)
        self.assertEqual(working.domains["A"].working_value, "A-local")
        self.assertEqual(working.domains["B"].working_value, "B-remote")

    def test_same_domain_remote_change_requires_real_observation_boundary(self) -> None:
        working = DomainWorkingSet()
        working.ensure_domain("A", self.base_a)
        working.durable_edit("A", "A-local")
        remote_a = self.base_a.assign(hid(100), "A-remote")

        with self.assertRaises(ModelError):
            working.observe_remote_domain("A", remote_a)

        local_pre = hid(200)
        working.observe_remote_domain(
            "A",
            remote_a,
            pre_observation_revision_id=local_pre,
        )

        self.assertFalse(working.domains["A"].dirty)
        self.assertEqual(
            set(working.domains["A"].causal.frontier()),
            {hid(100), local_pre},
        )
        self.assertEqual(working.domains["A"].locally_created_causal_revisions, 1)

    def test_projection_only_requires_ids_for_dirty_touched_domains(self) -> None:
        working = DomainWorkingSet()
        working.ensure_domain("A", self.base_a)
        working.ensure_domain("B", self.base_b)
        working.durable_edit("A", "A-local")

        remote_b = self.base_b.assign(hid(100), "B-remote")
        working.observe_remote_projection({"B": remote_b})
        self.assertTrue(working.domains["A"].dirty)

        remote_a = self.base_a.assign(hid(101), "A-remote")
        with self.assertRaises(ModelError):
            working.observe_remote_projection({"A": remote_a})

        working.observe_remote_projection(
            {"A": remote_a},
            pre_observation_revision_ids={"A": hid(201)},
        )
        self.assertFalse(working.domains["A"].dirty)

    def test_domain_local_policy_avoids_unrelated_observation_revision_explosion(self) -> None:
        rounds = 100

        local = DomainWorkingSet()
        local.ensure_domain("A", self.base_a)
        local.ensure_domain("B", self.base_b)

        global_policy = DomainWorkingSet()
        global_policy.ensure_domain("A", self.base_a)
        global_policy.ensure_domain("B", self.base_b)

        remote_b = self.base_b.copy()
        local.durable_edit("A", "A-0")
        global_policy.durable_edit("A", "A-0")

        for index in range(rounds):
            remote_b = remote_b.assign(hid(10_000 + index), f"B-{index}")

            local.observe_remote_domain("B", remote_b)

            naive_global_observation_boundary(
                global_policy,
                touched_domain="B",
                remote=remote_b,
                revision_ids_for_all_dirty={"A": hid(100_000 + index)},
            )

            # The user keeps editing A after seeing each unrelated B update.
            local.durable_edit("A", f"A-{index + 1}")
            global_policy.durable_edit("A", f"A-{index + 1}")

        local.seal_domain("A", hid(900_000))
        global_policy.seal_domain("A", hid(900_001))

        self.assertEqual(
            local.domains["A"].locally_created_causal_revisions,
            1,
        )
        self.assertEqual(
            global_policy.domains["A"].locally_created_causal_revisions,
            rounds + 1,
        )
        self.assertEqual(local.domains["A"].working_value, "A-100")
        self.assertEqual(global_policy.domains["A"].working_value, "A-100")

    def test_unrelated_remote_domain_does_not_change_observed_frontier_of_dirty_domain(self) -> None:
        working = DomainWorkingSet()
        working.ensure_domain("A", self.base_a)
        working.ensure_domain("B", self.base_b)
        working.durable_edit("A", "A-local")
        observed_before = working.domains["A"].observed_frontier

        remote_b = self.base_b.assign(hid(100), "B-remote")
        working.observe_remote_domain("B", remote_b)

        self.assertEqual(working.domains["A"].observed_frontier, observed_before)
        sealed = working.seal_domain("A", hid(200))
        assert sealed is not None
        self.assertEqual(sealed.parents, observed_before)


if __name__ == "__main__":
    unittest.main()
