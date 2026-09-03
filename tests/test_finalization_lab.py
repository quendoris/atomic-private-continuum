from __future__ import annotations

import unittest

from reference_model.apc_model import ModelError
from reference_model.causality_lab import FrontierCausalRegister
from reference_model.finalization_lab import (
    FinalizationLedger,
    conflict_value_with_identity,
)


def hid(value: int) -> str:
    return f"{value:064x}"


def observation_chain(rounds: int, *, finalize_early: bool) -> tuple[FinalizationLedger, str]:
    base = FrontierCausalRegister().assign(hid(1), "base")
    remote = base.copy()
    ledger = FinalizationLedger.from_causal(base)

    for index in range(rounds):
        local_id = hid(10_000 + index)
        ledger.append_local(
            revision_id=local_id,
            value=f"local-before-remote-{index}",
        )
        if finalize_early:
            ledger.finalize(local_id)

        remote = remote.assign(hid(100_000 + index), f"remote-{index}")
        ledger.merge_remote(remote)

    final_id = hid(900_000)
    ledger.append_local(revision_id=final_id, value="final-local")
    if finalize_early:
        ledger.finalize(final_id)
    return ledger, final_id


class ProvisionalIdentitySemanticsTests(unittest.TestCase):
    def test_unrelated_working_epoch_id_can_flip_winner_at_finalization(self) -> None:
        base = FrontierCausalRegister().assign(hid(1), "base")
        remote = base.assign(hid(100), "remote")

        visible_while_provisional = conflict_value_with_identity(
            base=base,
            remote=remote,
            local_identity=hid(200),
            local_value="local",
        )
        visible_after_fresh_final_id = conflict_value_with_identity(
            base=base,
            remote=remote,
            local_identity=hid(50),
            local_value="local",
        )

        self.assertEqual(visible_while_provisional, "local")
        self.assertEqual(visible_after_fresh_final_id, "remote")
        self.assertNotEqual(visible_while_provisional, visible_after_fresh_final_id)

    def test_preallocated_stable_revision_identity_preserves_conflict_result(self) -> None:
        base = FrontierCausalRegister().assign(hid(1), "base")
        remote = base.assign(hid(100), "remote")
        reserved_revision_id = hid(200)

        provisional = conflict_value_with_identity(
            base=base,
            remote=remote,
            local_identity=reserved_revision_id,
            local_value="local",
        )
        finalized = conflict_value_with_identity(
            base=base,
            remote=remote,
            local_identity=reserved_revision_id,
            local_value="local",
        )

        self.assertEqual(provisional, "local")
        self.assertEqual(finalized, provisional)


class FinalizationCostTests(unittest.TestCase):
    def test_provisional_private_observation_nodes_squash_before_finalization(self) -> None:
        rounds = 64
        ledger, final_id = observation_chain(rounds, finalize_early=False)

        before = ledger.metrics()
        self.assertEqual(before.local_revision_ids, rounds + 1)
        self.assertEqual(before.finalized_local_statements, 0)
        self.assertEqual(before.signing_transition_count, 0)

        result = ledger.squash_private()
        self.assertEqual(len(result.removed_ids), rounds)
        self.assertEqual(ledger.local_ids, {final_id})
        self.assertEqual(set(ledger.state.causal.frontier()), {final_id})
        self.assertEqual(ledger.state.causal.materialized_value(), "final-local")

        ledger.finalize(final_id)
        ledger.handoff([final_id])
        after = ledger.metrics()
        self.assertEqual(after.finalized_local_statements, 1)
        self.assertEqual(after.signing_transition_count, 1)
        self.assertEqual(after.handed_off_local_ids, 1)

    def test_early_finalization_blocks_same_private_squashing(self) -> None:
        rounds = 64
        ledger, _ = observation_chain(rounds, finalize_early=True)

        metrics = ledger.metrics()
        self.assertEqual(metrics.finalized_local_statements, rounds + 1)
        self.assertEqual(metrics.signing_transition_count, rounds + 1)

        with self.assertRaises(ModelError):
            ledger.squash_private()

    def test_publication_between_epochs_preserves_exposed_local_boundary(self) -> None:
        base = FrontierCausalRegister().assign(hid(1), "base")
        remote = base.copy()
        ledger = FinalizationLedger.from_causal(base)

        first_id = hid(10_000)
        ledger.append_local(revision_id=first_id, value="first-local")
        ledger.finalize(first_id)
        ledger.handoff([first_id])

        remote = remote.assign(hid(100_000), "remote-1")
        ledger.merge_remote(remote)

        private_mid = hid(10_001)
        ledger.append_local(revision_id=private_mid, value="private-mid")
        remote = remote.assign(hid(100_001), "remote-2")
        ledger.merge_remote(remote)

        final_id = hid(900_000)
        ledger.append_local(revision_id=final_id, value="final")

        result = ledger.squash_private()
        self.assertIn(private_mid, result.removed_ids)
        self.assertNotIn(first_id, result.removed_ids)
        self.assertIn(first_id, ledger.state.causal.revisions)

        ledger.finalize(final_id)
        ledger.handoff([final_id])
        self.assertEqual(ledger.metrics().signing_transition_count, 2)

    def test_duplicate_handoff_and_lost_ack_do_not_create_new_finalization(self) -> None:
        base = FrontierCausalRegister().assign(hid(1), "base")
        ledger = FinalizationLedger.from_causal(base)
        final_id = hid(10_000)
        ledger.append_local(revision_id=final_id, value="value")
        ledger.finalize(final_id)

        # First handoff may have succeeded remotely even if local ACK is lost.
        ledger.handoff([final_id])
        # Retrying finalization/handoff must be idempotent for the same statement.
        ledger.finalize(final_id)
        ledger.handoff([final_id])

        metrics = ledger.metrics()
        self.assertEqual(metrics.signing_transition_count, 1)
        self.assertEqual(metrics.handed_off_local_ids, 1)
        self.assertEqual(ledger.squash_private().removed_ids, frozenset())

    def test_transport_handoff_rejects_unfinalized_local_dependency(self) -> None:
        base = FrontierCausalRegister().assign(hid(1), "base")
        ledger = FinalizationLedger.from_causal(base)
        parent_id = hid(10_000)
        child_id = hid(10_001)
        ledger.append_local(revision_id=parent_id, value="parent")
        ledger.append_local(revision_id=child_id, value="child")
        ledger.finalize(child_id)

        with self.assertRaises(ModelError):
            ledger.handoff([child_id])


class FinalizationCrashTests(unittest.TestCase):
    def test_crash_restore_preserves_provisional_and_finalized_boundaries(self) -> None:
        base = FrontierCausalRegister().assign(hid(1), "base")
        ledger = FinalizationLedger.from_causal(base)
        provisional_id = hid(10_000)
        ledger.append_local(revision_id=provisional_id, value="pending")

        restored = FinalizationLedger.restore(ledger.snapshot())
        self.assertIn(provisional_id, restored.local_ids)
        self.assertNotIn(provisional_id, restored.finalized)
        self.assertEqual(restored.metrics().signing_transition_count, 0)

        restored.finalize(provisional_id)
        finalized_restore = FinalizationLedger.restore(restored.snapshot())
        self.assertIn(provisional_id, finalized_restore.finalized)
        self.assertEqual(finalized_restore.metrics().signing_transition_count, 1)

        # Re-finalizing after restart does not advance the modeled signing state.
        finalized_restore.finalize(provisional_id)
        self.assertEqual(finalized_restore.metrics().signing_transition_count, 1)


if __name__ == "__main__":
    unittest.main()
