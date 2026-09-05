from __future__ import annotations

import unittest

from reference_model.apc_model import ModelError
from reference_model.sync_lab import (
    AdaptivePublicationGate,
    DomainKey,
    MemoryOpaqueTransport,
    MultipartInbox,
    ReplicaSyncState,
    SyncProjection,
    TestOnlyOpaqueProtector,
    partition_projection,
)


def hid(value: int) -> str:
    return f"{value:064x}"


class AdaptivePublicationTests(unittest.TestCase):
    def test_continuous_typing_is_bounded_by_max_pending_age(self) -> None:
        gate = AdaptivePublicationGate(idle_ms=1000, max_pending_ms=5000)

        for now_ms in range(0, 5000, 200):
            gate.note_edit(now_ms)
            self.assertFalse(gate.should_publish(now_ms))

        self.assertTrue(gate.should_publish(5000))

    def test_quiet_boundary_publishes_earlier_than_max_age(self) -> None:
        gate = AdaptivePublicationGate(idle_ms=1000, max_pending_ms=8000)
        gate.note_edit(100)
        gate.note_edit(400)

        self.assertFalse(gate.should_publish(1399))
        self.assertTrue(gate.should_publish(1400))


class DirtyProjectionTests(unittest.TestCase):
    def test_many_local_edits_coalesce_to_one_dirty_domain(self) -> None:
        key = DomainKey(hid(1), "body")
        replica = ReplicaSyncState()

        for index in range(50):
            replica.assign(
                key=key,
                revision_id=hid(1000 + index),
                value=f"draft-{index}",
            )

        projection = replica.export_dirty()
        self.assertIsNotNone(projection)
        assert projection is not None
        self.assertEqual(set(projection.domains), {key})
        self.assertEqual(
            projection.domains[key].materialized_value(),
            "draft-49",
        )

    def test_ack_does_not_clear_domain_changed_during_publication(self) -> None:
        key = DomainKey(hid(1), "body")
        replica = ReplicaSyncState()
        replica.assign(key=key, revision_id=hid(1), value="A")

        projection = replica.export_dirty()
        assert projection is not None

        replica.assign(key=key, revision_id=hid(2), value="B")
        replica.acknowledge(projection)
        self.assertIn(key, replica.dirty)

        newest = replica.export_dirty()
        assert newest is not None
        replica.acknowledge(newest)
        self.assertNotIn(key, replica.dirty)


class CapsuleMergeTests(unittest.TestCase):
    def test_duplicate_and_out_of_order_capsules_converge(self) -> None:
        key = DomainKey(hid(1), "title")

        base = ReplicaSyncState()
        base.assign(key=key, revision_id=hid(1), value="base")
        base_projection = base.export_dirty()
        assert base_projection is not None

        left = ReplicaSyncState()
        right = ReplicaSyncState()
        receiver = ReplicaSyncState()
        for replica in (left, right, receiver):
            replica.import_projection(base_projection)

        left.assign(key=key, revision_id=hid(10), value="left")
        right.assign(key=key, revision_id=hid(20), value="right")
        left_projection = left.export_dirty()
        right_projection = right.export_dirty()
        assert left_projection is not None and right_projection is not None

        receiver.import_projection(right_projection)
        receiver.import_projection(right_projection)
        receiver.import_projection(left_projection)
        receiver.import_projection(left_projection)

        self.assertEqual(receiver.materialized(key), "right")
        self.assertEqual(
            set(receiver.domains[key].frontier()),
            {hid(10), hid(20)},
        )

    def test_independent_domains_merge_without_interference(self) -> None:
        body = DomainKey(hid(1), "body")
        title = DomainKey(hid(1), "title")

        left = ReplicaSyncState()
        right = ReplicaSyncState()
        left.assign(key=body, revision_id=hid(10), value="body")
        right.assign(key=title, revision_id=hid(20), value="title")

        left_projection = left.export_dirty()
        right_projection = right.export_dirty()
        assert left_projection is not None and right_projection is not None

        receiver = ReplicaSyncState()
        receiver.import_projection(left_projection)
        receiver.import_projection(right_projection)

        self.assertEqual(receiver.materialized(body), "body")
        self.assertEqual(receiver.materialized(title), "title")

    def test_projection_merge_has_no_identifier_precedence(self) -> None:
        left_key = DomainKey(hid(1), "body")
        right_key = DomainKey(hid(2), "body")
        left = ReplicaSyncState()
        right = ReplicaSyncState()
        left.assign(key=left_key, revision_id=hid(900), value="left")
        right.assign(key=right_key, revision_id=hid(1), value="right")

        left_projection = left.export_dirty()
        right_projection = right.export_dirty()
        assert left_projection is not None and right_projection is not None

        merged = left_projection.merge(right_projection)
        self.assertEqual(set(merged.domains), {left_key, right_key})
        self.assertFalse(hasattr(merged, "projection_id"))


class ProtectedMultipartTests(unittest.TestCase):
    def make_projection(self, count: int = 5) -> SyncProjection:
        replica = ReplicaSyncState()
        for index in range(count):
            replica.assign(
                key=DomainKey(hid(100 + index), "body"),
                revision_id=hid(1000 + index),
                value=f"secret-{index}",
            )
        projection = replica.export_dirty()
        assert projection is not None
        return projection

    def test_transport_rejects_clear_parts(self) -> None:
        projection = self.make_projection(1)
        clear = partition_projection(
            projection,
            publication_id=hid(6000),
            max_domains_per_part=1,
        )[0]
        transport = MemoryOpaqueTransport()

        with self.assertRaises(TypeError):
            transport.publish(clear)  # type: ignore[arg-type]

    def test_transport_only_receives_opaque_protected_parts(self) -> None:
        projection = self.make_projection(1)
        clear = partition_projection(
            projection,
            publication_id=hid(6001),
            max_domains_per_part=1,
        )[0]
        protector = TestOnlyOpaqueProtector()
        protected = protector.seal(clear)
        transport = MemoryOpaqueTransport()
        transport.publish(protected)

        self.assertEqual(len(transport.objects), 1)
        self.assertNotIn(b"secret-0", transport.objects[0].payload)

    def test_multipart_publication_is_not_visible_until_complete(self) -> None:
        projection = self.make_projection(5)
        clear_parts = partition_projection(
            projection,
            publication_id=hid(7000),
            max_domains_per_part=2,
        )
        self.assertEqual(len(clear_parts), 3)

        protector = TestOnlyOpaqueProtector()
        protected = [protector.seal(part) for part in clear_parts]
        inbox = MultipartInbox(protector)

        self.assertIsNone(inbox.ingest(protected[2]))
        self.assertIsNone(inbox.ingest(protected[0]))
        self.assertIsNone(inbox.ingest(protected[0]))

        assembled = inbox.ingest(protected[1])
        self.assertIsNotNone(assembled)
        assert assembled is not None
        self.assertEqual(set(assembled.domains), set(projection.domains))

    def test_publication_id_is_only_multipart_assembly_bookkeeping(self) -> None:
        projection = self.make_projection(2)
        first_parts = partition_projection(
            projection,
            publication_id=hid(10),
            max_domains_per_part=1,
        )
        second_parts = partition_projection(
            projection,
            publication_id=hid(9999),
            max_domains_per_part=1,
        )

        first_merged = first_parts[0].projection.merge(first_parts[1].projection)
        second_merged = second_parts[0].projection.merge(second_parts[1].projection)
        self.assertEqual(first_merged.domains, second_merged.domains)

    def test_unknown_protected_payload_is_rejected(self) -> None:
        from reference_model.sync_lab import ProtectedSyncPart

        inbox = MultipartInbox(TestOnlyOpaqueProtector())
        with self.assertRaises(ModelError):
            inbox.ingest(ProtectedSyncPart(b"not-a-known-token"))


if __name__ == "__main__":
    unittest.main()
