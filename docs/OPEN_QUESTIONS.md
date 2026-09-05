# A.P.C. open design questions

These questions are intentionally unresolved. They must be answered by analysis, prototypes or tests before becoming format commitments.

## Identifiers and causality

- Exact stable identifier construction for atoms, revisions, replicas and key states.
- Final required bit length and canonical binary/text encoding.
- Production representation of direct-frontier ID-linked causality.
- How causal membership can be compacted without making a long-offline valid replica appear concurrent with state it had already observed.
- Whether exact historical membership belongs in a cold index, is replaced by verifiable membership proofs, or is bounded by explicit causal/transport generations with safe rebootstrap.
- How wide concurrent frontiers are represented and compacted without introducing clocks or a hidden global sequence.
- Whether one compact causal summary can ever safely cover several independent merge domains without recreating global semantic causality.

The logical tie-break for genuinely concurrent scalar revisions is no longer open: causal precedence wins first; otherwise canonical unsigned lexicographic `RevisionId` order selects the materialized value.

The explicit all-ancestor representation is retained only as a correctness oracle; it is rejected as a production encoding because of quadratic reference growth.

## Working state and finalization

- Exact durable storage representation for `WorkingEpochId`, captured observation frontier, reserved `RevisionId`, finalized statement and exposure state.
- How late a stable `RevisionId` can be reserved without changing unresolved conflict semantics.
- Crash-atomic transition between private canonicalization, finalization, protected-capsule creation and transport handoff.
- Whether several finalized revisions can safely share one authentication/key-evolution transition under a studied cryptographic construction.
- Which private causal nodes can be eliminated before finalization without changing any valid future merge.

## Ordered collections and movement

- Exact sequence/CRDT structure for concurrent insertion, movement and reordering.
- Position identifier construction that avoids global renumbering as a correctness requirement.
- Final moved-anchor semantics when an offline insertion references an element that was concurrently moved.
- Production relationship between stable `AtomId`, immutable position identities and location/parent registers.
- Metadata growth and safe compaction of inactive/dead positions after long offline periods.

## Hierarchy and structural validity

- Final deterministic policy for resolving globally invalid parent graphs such as cycles.
- Whether the bounded `current -> causal witness -> safe root/orphan` candidate survives denser adversarial/statistical campaigns.
- Causal-purity metric: how often a fallback is the predecessor actually observed by the rejected move versus an unrelated historical/concurrent placement.
- Safe compaction of hierarchy-validity metadata without reintroducing unbounded placement history.
- Interaction between hierarchy validity, sequence position and deleted/hidden ancestors.

## Lifecycle and deletion

- Final concurrent delete-versus-edit policy.
- Whether explicit restore exists and, if so, what causal/lifecycle semantics it has.
- Retention policy for hidden concurrent content and dirty local content after a remote delete becomes visible.
- Tombstone stabilization and safe compaction for long-offline replicas.
- Interaction between deletion, attachment reachability and historical location anchors.

A simple physical delete and deletion encoded only as `location = None` are already rejected.

## Cross-domain atomic mutation

- Which operations, if any, genuinely require strong all-or-none semantics across several merge domains.
- Conflict semantics for concurrent or overlapping atomic mutation groups.
- Whether irreducible atomic conflict components can be bounded without turning ordinary independent domains into one large transaction domain.
- Whether a composite atomic domain can later split back into independent domains without changing semantics.

Transport/multipart all-or-none delivery alone is known to be insufficient for strong concurrent atomic merge semantics.

## Native container encoding

- Physical encoding of the single native `.apc` file.
- Internal indexing required for large continua without full-file scans.
- Incremental crash-safe durable update strategy for the single-file container.
- Integrity structure for partial reads, lazy verification and corruption detection.
- Internal layout that permits efficient access to very large attachments while preserving one native exportable object.
- Compaction/re-index rules that provably do not create semantic edits.

The complete `.apc` file is no longer assumed to be the unit of every synchronization update.

## Attachments

- Chunk size policy inside the native container and sync projection.
- Deduplication boundaries and privacy consequences.
- Random access, lazy verification and streaming decryption.
- Behavior for multi-gigabyte and multi-terabyte content.
- Reachability/garbage-collection rules across lifecycle deletion and retained sync/checkpoint generations.

## Cryptography

- Concrete AEAD and authenticated format framing.
- Nonce strategy and misuse resistance requirements.
- Content-encryption hierarchy and prospective content-key epochs.
- Concrete replica-authentication / forward-secure key-evolution construction satisfying `KEY_EVOLUTION.md`.
- Portable binding between stable `ReplicaId` and initial/current authenticated public key state.
- Trust-root or enrollment mechanism that authenticates a new replica without turning GitHub permissions into portable format semantics.
- Compact verification of long per-replica public key evolution.
- Same-replica fork detection/handling if active private state is accidentally cloned.
- Revocation model if A.P.C.-level authorization is later added.
- Replay/rollback treatment for protected synchronized material and local portable state.
- Key backup/transfer through explicit user-controlled mechanisms such as QR or offline transfer without creating a recovery authority.

## Synchronization capsules

- Frozen portable encoding for protected sync projections/capsules.
- Exact dependency declaration and buffering behavior for a capsule whose causal baseline is not yet locally available.
- Byte-based generic packing API without leaking provider-specific limits into core semantics.
- Multipart cryptographic binding, completion and resume protocol.
- Attachment-chunk reachability and retry semantics.
- Interaction between coalescing, private causal squashing and finalization.

## GitHub transport

- Concrete branch/ref layout and immutable protected-object layout.
- Optimistic compare-and-swap / fast-forward publication protocol and retry behavior under simultaneous writers.
- Efficient discovery of newly required opaque capsules from a small remote head marker.
- Practical polling/backoff/rate-limit policy for the foreground-only runtime.
- Transport generations/checkpoints and repository-history compaction.
- Exact stale-baseline policy when an old replica is outside the retained transport generation.
- Bootstrap policy for continua too large for GitHub, while preserving GitHub as an incremental transport afterward.

GitHub transport limits are not format limits, and GitHub is not required to store/retransmit the complete native `.apc` file for each edit.

## Durability

- Local transactional storage architecture.
- Exact content commit boundary.
- Working-epoch and finalization journal/WAL layout.
- View-state persistence cadence and crash tests.
- Recovery behavior after interrupted native-container writes.
- Recovery behavior across `seal -> merge remote -> persist -> finalize -> protect -> handoff` crash points.

## Compatibility

- Version negotiation rules.
- Critical versus optional extensions.
- Canonical test vectors for independent implementations.
- Rules for preserving unknown optional atom types and fields through read-modify-write cycles.
- Migration rules when an experimental merge primitive is replaced before format freeze.

## Implementation boundary

The core implementation may begin before these questions are closed, but unresolved mechanisms must sit behind explicit replaceable contracts. Convenience code must not silently freeze an answer that the specification still marks open.

## Validation rule

An open question should not be closed because one implementation technique is fashionable or convenient. A selected mechanism must be justified against A.P.C. requirements and, where practical, validated by an independent implementation, property test or adversarial counterexample campaign.
