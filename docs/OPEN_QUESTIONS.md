# A.P.C. open design questions

These questions are intentionally unresolved. They must be answered by analysis, prototypes or tests before becoming format commitments.

## Identifiers and causality

- Exact stable identifier construction for atoms, revisions, replicas and key states.
- Final required bit length and canonical binary/text encoding.
- Compact representation of causal context using ID-linked state rather than wall-clock ordering.
- How causal metadata can be compacted without making a long-offline valid replica appear concurrent with state it had already observed.
- Whether an explicit ancestry DAG is acceptable only for the prototype or can be replaced by a more compact production construction without introducing trusted counters/clocks into semantic ordering.

The logical tie-break for genuinely concurrent scalar revisions is no longer open: causal precedence wins first; otherwise canonical unsigned lexicographic `RevisionId` order selects the materialized value.

## Ordered collections

- Exact sequence/CRDT structure for concurrent insertion, movement and reordering.
- Position identifier construction that avoids global renumbering as a correctness requirement.
- Deletion and tombstone semantics.
- Concurrent delete-versus-edit behavior.
- Metadata growth and safe compaction after long offline periods.

## Native container encoding

- Physical encoding of the single native `.apc` file.
- Internal indexing required for large continua without full-file scans.
- Incremental durable update strategy for the single-file container.
- Integrity structure for partial reads and corruption detection.
- Internal layout that permits efficient access to very large attachments while preserving one native file.

## Attachments

- Chunk size policy inside the native container.
- Deduplication boundaries and privacy consequences.
- Random access, lazy verification and streaming decryption.
- Behavior for multi-gigabyte and larger attachments.

## Cryptography

- Content-encryption hierarchy.
- Authenticated format framing and nonce management.
- Concrete replica-authentication / forward-secure key-evolution construction satisfying `KEY_EVOLUTION.md`.
- Portable binding between a stable `ReplicaId` and its initial/current authenticated public key state.
- Trust-root or enrollment mechanism that authenticates a new replica without turning GitHub permissions into portable format semantics.
- Compact verification of long per-replica public key evolution.
- Same-replica fork detection/handling if active private state is accidentally cloned.
- Revocation model if A.P.C.-level authorization is later added.
- Key backup/transfer through explicit user-controlled mechanisms such as QR or offline transfer without creating a recovery authority.

## GitHub synchronization

- Efficient transfer/update strategy for one encrypted `.apc` repository file at large scales.
- Publication retry and remote revision protocol.
- Whether Git or an auxiliary transfer mechanism can avoid unnecessary full-file network transfer while the repository still exposes one synchronized native file.
- Practical GitHub size constraints and the point at which a different transport is required without changing A.P.C. format semantics.

## Durability

- Local transactional storage architecture.
- Exact content commit boundary.
- View-state persistence cadence and crash tests.
- Recovery behavior after interrupted writes.

## Compatibility

- Version negotiation rules.
- Critical versus optional extensions.
- Canonical test vectors for independent implementations.
- Rules for preserving unknown optional atom types and fields through read-modify-write cycles.

## Validation rule

An open question should not be closed because one implementation technique is fashionable or convenient. A selected mechanism must be justified against A.P.C. requirements and, where practical, validated by an independent implementation or adversarial test.
