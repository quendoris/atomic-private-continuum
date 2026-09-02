# A.P.C. open design questions

These questions are intentionally unresolved. They must be answered by analysis, prototypes or tests before becoming format commitments.

## Identifiers

- Exact stable identifier construction for atoms, revisions and replicas.
- Required bit length and canonical encoding.
- Deterministic tie-break rules for genuinely concurrent scalar revisions.

## Ordered collections

- Sequence/CRDT structure for concurrent insertion and reordering.
- Deletion and tombstone semantics.
- Metadata growth and safe compaction after long offline periods.

## Native container encoding

- Physical encoding of a native `.apc` object.
- Indexing required for large continua without full-file scans.
- Whether native export is a single physical file with an internal indexed container, and how that container supports incremental durable writes.
- Integrity structure for partial reads and corruption detection.

## Attachments

- Chunk size policy and content-addressing strategy.
- Deduplication boundaries and privacy consequences.
- Random access, lazy verification and streaming decryption.
- Behavior for multi-gigabyte and larger attachments.

## Cryptography

- Content-encryption hierarchy.
- Authenticated format framing and nonce management.
- Replica authentication and forward-secure signing/key evolution.
- Portable introduction of a new replica.
- Revocation model if A.P.C.-level authorization is later added.
- Key backup/transfer through explicit user-controlled mechanisms such as QR or offline transfer without creating a recovery authority.

## Synchronization transport

- Efficient Git/GitHub representation for very large continua.
- Whether one repository file is practical at target scales or whether the Git adapter requires an opaque segmented transport representation.
- Publication retry and remote revision protocol.
- Garbage collection of superseded transport objects if segmented transport is used.

## Durability

- Local transactional storage architecture.
- Exact content commit boundary.
- View-state persistence cadence and crash tests.
- Recovery behavior after interrupted writes.

## Compatibility

- Version negotiation rules.
- Critical versus optional extensions.
- Canonical test vectors for independent implementations.

## Validation rule

An open question should not be closed because one implementation technique is fashionable or convenient. A selected mechanism must be justified against A.P.C. requirements and, where practical, validated by an independent implementation or adversarial test.
