from __future__ import annotations

import unittest

from reference_model.apc_model import ModelError
from reference_model.causality_lab import FrontierCausalRegister
from reference_model.working_state_lab import (
    WorkingScalar,
    naive_publish_using_latest_frontier,
)


def hid(value: int) -> str:
    return f"{value:064x}"


class WorkingStateCausalityTests(unittest.TestCase):
    def base(self) -> FrontierCausalRegister:
        return FrontierCausalRegister().assign(hid(1), "base")

    def test_many_durable_edits_create_one_portable_causal_revision(self) -> None:
        working = WorkingScalar.from_causal(self.base())

        for index in range(10_000):
            working.durable_edit(f"text-{index}")

        before = working.metrics()
        self.assertEqual(before.durable_write_count, 10_000)
        self.assertEqual(before.locally_created_causal_revisions, 0)
        self.assertEqual(before.retained_causal_nodes, 1)
        self.assertTrue(before.pending_dirty)

        sealed = working.seal(hid(10_001))
        self.assertIsNotNone(sealed)
        assert sealed is not None
        self.assertEqual(sealed.parents, frozenset({hid(1)}))
        self.assertEqual(working.working_value, "text-9999")

        after = working.metrics()
        self.assertEqual(after.durable_write_count, 10_000)
        self.assertEqual(after.locally_created_causal_revisions, 1)
        self.assertEqual(after.retained_causal_nodes, 2)
        self.assertFalse(after.pending_dirty)

    def test_crash_restore_preserves_pending_value_and_observation_frontier(self) -> None:
        working = WorkingScalar.from_causal(self.base())
        for index in range(250):
            working.durable_edit(f"draft-{index}")

        restored = WorkingScalar.restore(working.snapshot())
        self.assertTrue(restored.dirty)
        self.assertEqual(restored.working_value, "draft-249")
        self.assertEqual(restored.observed_frontier, frozenset({hid(1)}))
        self.assertEqual(restored.metrics().durable_write_count, 250)

        restored.durable_edit("draft-after-restart")
        revision = restored.seal(hid(500))
        assert revision is not None
        self.assertEqual(revision.parents, frozenset({hid(1)}))
        self.assertEqual(restored.materialized if hasattr(restored, "materialized") else restored.working_value, "draft-after-restart")

    def test_naive_latest_frontier_publish_falsely_claims_remote_observation(self) -> None:
        base = self.base()
        local_pending_value = "local-before-remote"

        remote = base.assign(hid(900), "remote")
        causal_after_remote = base.merge(remote)

        naive = naive_publish_using_latest_frontier(
            causal_after_remote=causal_after_remote,
            revision_id=hid(100),
            pending_value=local_pending_value,
        )

        # The local revision ID is deliberately smaller than the remote ID.  It
        # still wins because the naive model incorrectly makes it a descendant
        # of the remote revision.
        self.assertEqual(naive.materialized_value(), local_pending_value)
        self.assertEqual(set(naive.frontier()), {hid(100)})
        self.assertTrue(naive.is_ancestor(hid(900), hid(100)))

    def test_remote_observation_seals_pre_remote_work_before_merge(self) -> None:
        base = self.base()
        working = WorkingScalar.from_causal(base)
        working.durable_edit("local-before-remote")

        remote = base.assign(hid(900), "remote")
        working.observe_remote(remote, pre_observation_revision_id=hid(100))

        # Pre-remote local work and the remote branch are genuinely concurrent.
        self.assertEqual(set(working.causal.frontier()), {hid(100), hid(900)})
        self.assertFalse(working.causal.is_ancestor(hid(900), hid(100)))
        self.assertFalse(working.causal.is_ancestor(hid(100), hid(900)))
        self.assertEqual(working.working_value, "remote")

        # Work performed after the remote merge really has observed both sides.
        working.durable_edit("local-after-seeing-remote")
        post = working.seal(hid(50))  # smaller than both concurrent IDs
        assert post is not None
        self.assertEqual(post.parents, frozenset({hid(100), hid(900)}))
        self.assertEqual(set(working.causal.frontier()), {hid(50)})
        self.assertEqual(working.working_value, "local-after-seeing-remote")
        self.assertTrue(working.causal.is_ancestor(hid(900), hid(50)))
        self.assertTrue(working.causal.is_ancestor(hid(100), hid(50)))

    def test_dirty_remote_observation_requires_explicit_seal_id(self) -> None:
        base = self.base()
        working = WorkingScalar.from_causal(base)
        working.durable_edit("pending")
        remote = base.assign(hid(20), "remote")

        with self.assertRaises(ModelError):
            working.observe_remote(remote)

        self.assertTrue(working.dirty)
        self.assertEqual(working.working_value, "pending")

    def test_remote_observation_without_pending_work_creates_no_local_revision(self) -> None:
        base = self.base()
        working = WorkingScalar.from_causal(base)
        remote = base.assign(hid(20), "remote")

        working.observe_remote(remote)

        self.assertEqual(working.metrics().locally_created_causal_revisions, 0)
        self.assertEqual(working.working_value, "remote")
        self.assertFalse(working.dirty)

    def test_causal_revision_count_tracks_observation_boundaries_not_keystrokes(self) -> None:
        base = self.base()
        working = WorkingScalar.from_causal(base)
        remote_base = base

        edit_count = 0
        next_local_id = 100_000
        next_remote_id = 200_000

        for epoch in range(4):
            for index in range(2_000):
                working.durable_edit(f"epoch-{epoch}-edit-{index}")
                edit_count += 1

            if epoch < 3:
                remote_base = remote_base.assign(
                    hid(next_remote_id), f"remote-{epoch}"
                )
                next_remote_id += 1
                working.observe_remote(
                    remote_base,
                    pre_observation_revision_id=hid(next_local_id),
                )
                next_local_id += 1

        working.seal(hid(next_local_id))

        metrics = working.metrics()
        self.assertEqual(edit_count, 8_000)
        self.assertEqual(metrics.durable_write_count, 8_000)
        self.assertEqual(metrics.locally_created_causal_revisions, 4)
        self.assertFalse(metrics.pending_dirty)

        # The final post-observation local revision causally dominates every
        # prior frontier, despite causal IDs carrying no recency semantics.
        self.assertEqual(len(working.causal.frontier()), 1)


if __name__ == "__main__":
    unittest.main()
