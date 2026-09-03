from __future__ import annotations

import unittest

from reference_model.apc_model import ModelError
from reference_model.hierarchy_lab import HierarchyLab


def hid(value: int) -> str:
    return f"{value:064x}"


class HierarchyLabTests(unittest.TestCase):
    def base(self) -> HierarchyLab:
        h = HierarchyLab()
        h = h.place(atom_id=hid(10), revision_id=hid(100), parent_atom_id=None)  # P
        h = h.place(atom_id=hid(11), revision_id=hid(101), parent_atom_id=None)  # Q
        h = h.place(atom_id=hid(12), revision_id=hid(102), parent_atom_id=hid(10))  # C under P
        return h

    def test_move_between_containers_is_one_parent_location_change(self) -> None:
        h = self.base()
        moved = h.place(atom_id=hid(12), revision_id=hid(200), parent_atom_id=hid(11))

        self.assertEqual(moved.active_parent(hid(12)), hid(11))
        self.assertEqual(moved.visible_atoms(), {hid(10), hid(11), hid(12)})
        self.assertEqual(len(moved.parents), 3)

    def test_concurrent_moves_of_same_child_choose_one_parent_without_duplication(self) -> None:
        base = self.base()
        left = base.place(atom_id=hid(12), revision_id=hid(300), parent_atom_id=hid(10))
        right = base.place(atom_id=hid(12), revision_id=hid(400), parent_atom_id=hid(11))

        merged = left.merge(right)
        self.assertEqual(merged.active_parent(hid(12)), hid(11))
        self.assertEqual(merged.visible_atoms(), {hid(10), hid(11), hid(12)})
        self.assertEqual(list(merged.parents).count(hid(12)), 1)

    def test_child_insert_under_concurrently_deleted_parent_is_retained_but_hidden(self) -> None:
        base = self.base()
        deleted = base.delete(hid(10))

        inserted = base.place(
            atom_id=hid(13),
            revision_id=hid(500),
            parent_atom_id=hid(10),
        )

        merged = deleted.merge(inserted)
        self.assertIn(hid(13), merged.parents)
        self.assertNotIn(hid(13), merged.visible_atoms())
        self.assertIn(hid(13), merged.hidden_by_ancestor())

    def test_child_moved_out_concurrently_with_parent_delete_survives(self) -> None:
        base = self.base()
        deleted = base.delete(hid(10))
        escaped = base.place(
            atom_id=hid(12),
            revision_id=hid(600),
            parent_atom_id=None,
        )

        merged = deleted.merge(escaped)
        self.assertNotIn(hid(10), merged.visible_atoms())
        self.assertIn(hid(12), merged.visible_atoms())
        self.assertEqual(merged.active_parent(hid(12)), None)

    def test_delete_does_not_rewrite_descendant_parent_identity(self) -> None:
        h = self.base().delete(hid(10))
        self.assertEqual(h.active_parent(hid(12)), hid(10))
        self.assertIn(hid(12), h.hidden_by_ancestor())

    def test_concurrent_cross_moves_can_create_active_parent_cycle(self) -> None:
        base = HierarchyLab()
        base = base.place(atom_id=hid(20), revision_id=hid(700), parent_atom_id=None)
        base = base.place(atom_id=hid(21), revision_id=hid(701), parent_atom_id=None)

        left = base.place(atom_id=hid(20), revision_id=hid(800), parent_atom_id=hid(21))
        right = base.place(atom_id=hid(21), revision_id=hid(900), parent_atom_id=hid(20))
        merged = left.merge(right)

        self.assertEqual(merged.active_parent(hid(20)), hid(21))
        self.assertEqual(merged.active_parent(hid(21)), hid(20))
        with self.assertRaises(ModelError):
            merged.visible_atoms()


if __name__ == "__main__":
    unittest.main()
