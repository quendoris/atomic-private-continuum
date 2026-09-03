"""Experimental sync-capsule model for A.P.C.

This module tests synchronization semantics only. It is not a production
transport implementation and does not define production cryptography.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, Optional
import copy
import secrets

from reference_model.apc_model import ModelError, ScalarRegister


@dataclass(frozen=True, order=True)
class DomainKey:
    atom_id: str
    domain: str


@dataclass
class SyncProjection:
    projection_id: str
    domains: Dict[DomainKey, ScalarRegister] = field(default_factory=dict)

    def copy(self) -> "SyncProjection":
        return SyncProjection(
            projection_id=self.projection_id,
            domains={key: value.copy() for key, value in self.domains.items()},
        )

    def merge(self, other: "SyncProjection") -> "SyncProjection":
        merged: Dict[DomainKey, ScalarRegister] = {}
        for key in set(self.domains) | set(other.domains):
            if key in self.domains and key in other.domains:
                merged[key] = self.domains[key].merge(other.domains[key])
            else:
                merged[key] = (self.domains.get(key) or other.domains[key]).copy()
        return SyncProjection(
            projection_id=max(self.projection_id, other.projection_id),
            domains=merged,
        )


@dataclass
class ReplicaSyncState:
    domains: Dict[DomainKey, ScalarRegister] = field(default_factory=dict)
    dirty: set[DomainKey] = field(default_factory=set)

    def assign(
        self,
        *,
        key: DomainKey,
        revision_id: str,
        value: str,
    ) -> None:
        register = self.domains.get(key, ScalarRegister())
        self.domains[key] = register.assign(revision_id, value)
        self.dirty.add(key)

    def import_projection(self, projection: SyncProjection) -> None:
        for key, incoming in projection.domains.items():
            current = self.domains.get(key)
            self.domains[key] = (
                incoming.copy() if current is None else current.merge(incoming)
            )

    def export_dirty(self, *, projection_id: str) -> Optional[SyncProjection]:
        if not self.dirty:
            return None
        return SyncProjection(
            projection_id=projection_id,
            domains={key: self.domains[key].copy() for key in self.dirty},
        )

    def acknowledge(self, projection: SyncProjection) -> None:
        """Clear only domains that did not change after this projection."""

        for key, exported in projection.domains.items():
            current = self.domains.get(key)
            if current is not None and current.revisions == exported.revisions:
                self.dirty.discard(key)

    def materialized(self, key: DomainKey):
        register = self.domains.get(key)
        return None if register is None else register.materialized_value()


@dataclass
class AdaptivePublicationGate:
    """Deterministic lab policy for idle + maximum-pending-age publication."""

    idle_ms: int = 1000
    max_pending_ms: int = 8000
    first_dirty_ms: Optional[int] = None
    last_edit_ms: Optional[int] = None

    def note_edit(self, now_ms: int) -> None:
        if self.first_dirty_ms is None:
            self.first_dirty_ms = now_ms
        self.last_edit_ms = now_ms

    def should_publish(self, now_ms: int) -> bool:
        if self.first_dirty_ms is None or self.last_edit_ms is None:
            return False
        return (
            now_ms - self.last_edit_ms >= self.idle_ms
            or now_ms - self.first_dirty_ms >= self.max_pending_ms
        )

    def published(self) -> None:
        self.first_dirty_ms = None
        self.last_edit_ms = None


@dataclass(frozen=True)
class ClearSyncPart:
    publication_id: str
    part_index: int
    total_parts: int
    projection: SyncProjection


@dataclass(frozen=True)
class ProtectedSyncPart:
    """Opaque object accepted by transport adapters.

    `payload` is intentionally uninterpreted by the sync transport. Production
    payload protection is not implemented by this reference module.
    """

    payload: bytes


def partition_projection(
    projection: SyncProjection,
    *,
    publication_id: str,
    max_domains_per_part: int,
) -> list[ClearSyncPart]:
    if max_domains_per_part <= 0:
        raise ModelError("max_domains_per_part must be positive")
    keys = sorted(projection.domains)
    if not keys:
        raise ModelError("cannot partition an empty projection")

    groups = [
        keys[index : index + max_domains_per_part]
        for index in range(0, len(keys), max_domains_per_part)
    ]
    total = len(groups)
    parts = []
    for index, group in enumerate(groups):
        fragment = SyncProjection(
            projection_id=projection.projection_id,
            domains={key: projection.domains[key].copy() for key in group},
        )
        parts.append(
            ClearSyncPart(
                publication_id=publication_id,
                part_index=index,
                total_parts=total,
                projection=fragment,
            )
        )
    return parts


class TestOnlyOpaqueProtector:
    """Non-cryptographic test double for the E2EE API boundary.

    It stores clear parts only in a private in-memory map and emits random opaque
    tokens to transport code. It deliberately makes no cryptographic claim.
    Production code must replace this with an audited authenticated-encryption
    construction while preserving the same clear/protected boundary.
    """

    def __init__(self) -> None:
        self._sealed: dict[bytes, ClearSyncPart] = {}

    def seal(self, part: ClearSyncPart) -> ProtectedSyncPart:
        token = secrets.token_bytes(32)
        while token in self._sealed:
            token = secrets.token_bytes(32)
        self._sealed[token] = copy.deepcopy(part)
        return ProtectedSyncPart(token)

    def open(self, part: ProtectedSyncPart) -> ClearSyncPart:
        try:
            return copy.deepcopy(self._sealed[part.payload])
        except KeyError as exc:
            raise ModelError("unknown or invalid protected test part") from exc


@dataclass
class MemoryOpaqueTransport:
    """Transport test double that refuses clear sync objects."""

    objects: list[ProtectedSyncPart] = field(default_factory=list)

    def publish(self, part: ProtectedSyncPart) -> None:
        if not isinstance(part, ProtectedSyncPart):
            raise TypeError("transport accepts protected sync parts only")
        self.objects.append(part)


@dataclass
class MultipartInbox:
    """Assemble protected multipart publications before semantic merge."""

    protector: TestOnlyOpaqueProtector
    _parts: dict[str, dict[int, ClearSyncPart]] = field(default_factory=dict)
    _totals: dict[str, int] = field(default_factory=dict)

    def ingest(self, protected: ProtectedSyncPart) -> Optional[SyncProjection]:
        clear = self.protector.open(protected)
        if clear.total_parts <= 0:
            raise ModelError("invalid multipart total")
        if not 0 <= clear.part_index < clear.total_parts:
            raise ModelError("invalid multipart part index")

        publication_id = clear.publication_id
        previous_total = self._totals.get(publication_id)
        if previous_total is not None and previous_total != clear.total_parts:
            raise ModelError("multipart total mismatch")
        self._totals[publication_id] = clear.total_parts

        bucket = self._parts.setdefault(publication_id, {})
        previous = bucket.get(clear.part_index)
        if previous is not None:
            if previous != clear:
                raise ModelError("multipart part collision")
        else:
            bucket[clear.part_index] = clear

        if len(bucket) != clear.total_parts:
            return None

        projection: Optional[SyncProjection] = None
        for index in range(clear.total_parts):
            part = bucket[index]
            projection = (
                part.projection.copy()
                if projection is None
                else projection.merge(part.projection)
            )

        del self._parts[publication_id]
        del self._totals[publication_id]
        return projection
