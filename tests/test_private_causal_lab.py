from __future__ import annotations

import unittest

from reference_model.causality_lab import FrontierCausalRegister
from reference_model.private_causal_lab import ExposureAwareCausalState
from reference_model.working_state_lab import WorkingScalar


def hid(value: int) -> str:
    return f"{value:064x}"


class PrivateCausalSquashingTests(unittest.TestCase):
    def base(self) -> FrontierCausalRegister:
        return FrontierCausalRegister().assign(hid(1), "base")

    def resolved_local_remote_history(self):
        base = self.base()
        working = WorkingScalar.from_causal(base)
        working.durable_edit("local-before-remote")

        remote = base.assign(hid(900), "remote")
        working.observe_remote(remote, pre_observation_revision_id=hid(100))
        working.durable_edit("resolved-after-observation")
        working.seal(hid(50))
        return working.causal

    def test_unexposed_dominated_local_revision_can_be_bypassed(self) -> None:
        original = self.resolved_local_remote_history()
        state = ExposureAwareCausalState(
            causal=original,
            private_local_ids={hid(100), hid(50)},
            exposed_ids={hid(1), hid(900)},
        )

        result = state.squash_unexposed_dominated()
        compact = result.state.causal

        self.assertEqual(result.removed_ids, frozenset({hid(100)}))
        self.assertNotIn(hid(100), compact.revisions)
        self.assertIn(hid(50), compact.revisions)
        self.assertEqual(set(compact.frontier()), set(original.frontier()))
        self.assertEqual(compact.materialized_value(), original.materialized_value())

        # Remote 900 already implies base 1, so transitive reduction leaves the
        # final private frontier revision depending only on remote 900.
        self.assertEqual(compact.revisions[hid(50)].parents, frozenset({hid(900)}))
        self.assertTrue(compact.is_ancestor(hid(900), hid(50)))
        self.assertTrue(compact.is_ancestor(hid(1), hid(50)))

    def test_exposed_local_revision_is_never_removed(self) -> None:
        original = self.resolved_local_remote_history()
        state = ExposureAwareCausalState(
            causal=original,
            private_local_ids={hid(100), hid(50)},
            exposed_ids={hid(1), hid(900)},
        )

        state.mark_exposed([hid(100)])
        result = state.squash_unexposed_dominated()

        self.assertNotIn(hid(100), result.removed_ids)
        self.assertIn(hid(100), result.state.causal.revisions)
        self.assertIn(hid(100), result.state.exposed_ids)

    def test_exposing_descendant_closes_over_private_parent_before_handoff(self) -> None:
        original = self.resolved_local_remote_history()
        state = ExposureAwareCausalState(
            causal=original,
            private_local_ids={hid(100), hid(50)},
            exposed_ids={hid(1), hid(900)},
        )

        # If final revision 50 is handed to transport before squashing, its
        # named private parent 100 becomes externally relevant as well.
        state.mark_exposed([hid(50)])
        self.assertIn(hid(100), state.exposed_ids)
        self.assertIn(hid(50), state.exposed_ids)

        result = state.squash_unexposed_dominated()
        self.assertEqual(result.removed_ids, frozenset())

    def test_squash_must_happen_before_transport_handoff(self) -> None:
        original = self.resolved_local_remote_history()

        before_handoff = ExposureAwareCausalState(
            causal=original,
            private_local_ids={hid(100), hid(50)},
            exposed_ids={hid(1), hid(900)},
        ).squash_unexposed_dominated().state
        before_handoff.mark_exposed([hid(50)])

        after_handoff = ExposureAwareCausalState(
            causal=original,
            private_local_ids={hid(100), hid(50)},
            exposed_ids={hid(1), hid(900)},
        )
        after_handoff.mark_exposed([hid(50)])
        after_handoff = after_handoff.squash_unexposed_dominated().state

        self.assertNotIn(hid(100), before_handoff.causal.revisions)
        self.assertIn(hid(100), after_handoff.causal.revisions)
        self.assertEqual(
            before_handoff.causal.materialized_value(),
            after_handoff.causal.materialized_value(),
        )

    def test_many_private_observation_epochs_reduce_to_external_history_plus_frontier(self) -> None:
        base = self.base()
        working = WorkingScalar.from_causal(base)
        remote = base
        private_ids: set[str] = set()
        exposed_ids: set[str] = {hid(1)}

        epochs = 64
        for epoch in range(epochs):
            working.durable_edit(f"local-pre-{epoch}")
            local_id = hid(100_000 + epoch)

            remote_id = hid(200_000 + epoch)
            remote = remote.assign(remote_id, f"remote-{epoch}")
            exposed_ids.add(remote_id)

            working.observe_remote(
                remote,
                pre_observation_revision_id=local_id,
            )
            private_ids.add(local_id)

        working.durable_edit("final-local")
        final_id = hid(900_000)
        working.seal(final_id)
        private_ids.add(final_id)

        original = working.causal
        state = ExposureAwareCausalState(
            causal=original,
            private_local_ids=private_ids,
            exposed_ids=exposed_ids,
        )
        result = state.squash_unexposed_dominated()
        compact = result.state.causal

        self.assertEqual(len(result.removed_ids), epochs)
        self.assertEqual(set(compact.frontier()), {final_id})
        self.assertEqual(compact.materialized_value(), "final-local")

        # The externally sourced remote chain must remain available to this lab,
        # while all dominated never-exposed local observation markers disappear.
        self.assertEqual(len(original.revisions) - len(compact.revisions), epochs)
        self.assertEqual(compact.revisions[final_id].parents, frozenset({hid(200_000 + epochs - 1)}))


if __name__ == "__main__":
    unittest.main()
