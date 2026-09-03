from __future__ import annotations

import unittest

from reference_model.apc_model import ModelError
from reference_model.atom_interaction_lab import AtomInteractionLab


def hid(value: int) -> str:
    return f"{value:064x}"


class AtomInteractionTests(unittest.TestCase):
    def base(self) -> AtomInteractionLab:
        lab = AtomInteractionLab()
        lab.add_atom(
            atom_id=hid(100),
            position_id=hid(1000),
            location_revision_id=hid(2000),
            content_revision_id=hid(3000),
            content_value="A0",
        )
        lab.add_atom(
            atom_id=hid(101),
            position_id=hid(1001),
            location_revision_id=hid(2001),
            content_revision_id=hid(3001),
            content_value="B0",
            left_atom_id=hid(100),
        )
        return lab

    def test_remote_move_does_not_seal_dirty_content(self) -> None:
        local = self.base()
        remote = self.base()

        local.durable_edit_content(hid(100), "A-draft")
        remote.move_atom(
            atom_id=hid(100),
            position_id=hid(1100),
            location_revision_id=hid(2100),
            left_atom_id=hid(101),
        )

        local.observe_remote_structure(remote.structure)

        self.assertEqual(local.visible_atoms(), [hid(101), hid(100)])
        self.assertEqual(local.visible_content()[hid(100)], "A-draft")
        self.assertTrue(local.content[hid(100)].dirty)
        self.assertEqual(local.content[hid(100)].locally_created_causal_revisions, 0)

    def test_remote_delete_hides_atom_without_erasing_pending_content(self) -> None:
        local = self.base()
        remote = self.base()

        local.durable_edit_content(hid(100), "A-draft")
        remote.delete_atom(hid(100))
        local.observe_remote_structure(remote.structure)

        self.assertNotIn(hid(100), local.visible_atoms())
        self.assertNotIn(hid(100), local.visible_content())
        self.assertEqual(local.retained_content_value(hid(100)), "A-draft")
        self.assertTrue(local.content[hid(100)].dirty)
        self.assertEqual(local.content[hid(100)].locally_created_causal_revisions, 0)

        with self.assertRaises(ModelError):
            local.durable_edit_content(hid(100), "should-not-edit")

    def test_same_content_domain_remote_edit_still_requires_observation_boundary(self) -> None:
        local = self.base()
        remote = self.base()
        local.durable_edit_content(hid(100), "local-draft")

        remote.content[hid(100)].durable_edit("remote-edit")
        remote.seal_content(hid(100), hid(4000))

        with self.assertRaises(ModelError):
            local.observe_remote_content(hid(100), remote.content[hid(100)].causal)

        local.observe_remote_content(
            hid(100),
            remote.content[hid(100)].causal,
            pre_observation_revision_id=hid(5000),
        )
        self.assertFalse(local.content[hid(100)].dirty)
        self.assertEqual(local.content[hid(100)].locally_created_causal_revisions, 1)
        self.assertEqual(
            set(local.content[hid(100)].causal.frontier()),
            {hid(4000), hid(5000)},
        )

    def test_delete_wins_visibility_over_concurrent_content_edit(self) -> None:
        edited = self.base()
        deleted = self.base()

        edited.durable_edit_content(hid(100), "offline-edit")
        edited.seal_content(hid(100), hid(6000))
        deleted.delete_atom(hid(100))

        edited.observe_remote_structure(deleted.structure)

        self.assertNotIn(hid(100), edited.visible_atoms())
        self.assertEqual(edited.retained_content_value(hid(100)), "offline-edit")
        self.assertEqual(edited.content[hid(100)].causal.materialized_value(), "offline-edit")

    def test_stale_or_concurrent_move_cannot_resurrect_deleted_atom(self) -> None:
        deleted = self.base()
        moved = self.base()

        deleted.delete_atom(hid(100))
        moved.move_atom(
            atom_id=hid(100),
            position_id=hid(1200),
            location_revision_id=hid(2200),
            left_atom_id=hid(101),
        )

        deleted.observe_remote_structure(moved.structure)
        self.assertNotIn(hid(100), deleted.visible_atoms())

    def test_move_and_content_remain_attached_by_stable_atom_identity(self) -> None:
        lab = self.base()
        lab.durable_edit_content(hid(100), "edited")
        lab.move_atom(
            atom_id=hid(100),
            position_id=hid(1300),
            location_revision_id=hid(2300),
            left_atom_id=hid(101),
        )

        self.assertEqual(lab.visible_atoms(), [hid(101), hid(100)])
        self.assertEqual(lab.visible_content()[hid(100)], "edited")
        self.assertEqual(set(lab.content), {hid(100), hid(101)})


if __name__ == "__main__":
    unittest.main()
