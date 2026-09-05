# A.P.C. sync implementation

Status: **first protected scalar sync path implemented; transport API and portable sync encoding are not frozen**.

This document records the executable Rust synchronization boundary after the research model in `SYNC_EXPERIMENTS.md`. It does not replace `SYNC.md`, `SYNC_CAPSULES.md` or `GITHUB_TRANSPORT.md`.

## 1. Repository layer

The Rust workspace now contains a separate synchronization crate:

```text
crates/apc-core/          semantic state and merge
crates/apc-crypto/        authenticated symmetric protection
crates/apc-storage-fs/    development local durability backend
crates/apc-sync/          transport-independent protected sync projections
```

`apc-sync` depends on the semantic core and protection crate. It does not import GitHub concepts or filesystem durability semantics.

## 2. Semantic projection has no publication identity

The implemented `SyncProjection<K, S>` contains only merge-domain state:

```text
SyncProjection
└── domains
    ├── DomainKey -> mergeable state
    └── ...
```

It deliberately has no projection ID, publication ID, transport revision or timestamp.

The earlier Python research model used `max(projection_id)` while merging projections. That research-only ordering leak has been removed. Publication identity now exists only in multipart/protection bookkeeping and cannot influence semantic merge.

For the first scalar implementation, `DomainKey` contains:

```text
AtomId
+
pre-format domain identifier bytes
```

The final portable domain namespace/encoding remains open.

## 3. Dirty-domain state

`DirtyDomainState<K, S>` separates current semantic state from local publication dirtiness.

The implemented rules are:

- a local replacement of one domain marks exactly that domain dirty;
- importing validated remote state does not make a clean domain locally dirty;
- if a domain already contains unpublished local work, importing remote state preserves the dirty marker;
- export captures the exact current state of dirty domains;
- publication acknowledgement clears a dirty marker only if the current domain still equals the state that was exported.

Therefore this race is safe:

```text
export A
   |
local edit B
   |
ack A
```

The domain remains dirty because B differs from the acknowledged projection.

## 4. Pre-format scalar projection encoding

`apc-sync` currently has a deterministic development codec identified by `APCSYNC1`.

The codec serializes:

```text
projection
├── domain count
└── domains in canonical map order
    ├── AtomId
    ├── domain identifier bytes
    └── ScalarRegister
        └── revisions in canonical register order
            ├── RevisionId
            ├── value bytes
            └── direct causal parent IDs
```

Decoding validates scalar state through the ordinary core import boundary and rejects malformed magic/version, truncation, trailing bytes, duplicate domains and invalid revision structures.

This codec is **not** the native `.apc` format and is not a compatibility promise. It exists so real protected synchronization can execute before checkpoint/coverage encoding is frozen.

## 5. Protected sync parts

Transport-facing state is represented by `ProtectedSyncPart`:

```text
ProtectedSyncPart
├── publication_id     clear transport/assembly bookkeeping
├── part_index         clear transport/assembly bookkeeping
├── total_parts        clear transport/assembly bookkeeping
└── payload            authenticated ciphertext
```

The clear bookkeeping is not trusted merely because it is visible. `protect_scalar_part()` binds the following values into AEAD associated data:

```text
sync-part domain separator
ContinuumId
PublicationId
part_index
total_parts
```

The payload is the authenticated encryption of the deterministic clear scalar projection.

Consequently, changing the continuum, publication identity, part index or total part count without re-authentication causes the part to fail before semantic merge.

`PublicationId` is opaque. Its byte magnitude has no causal, temporal or merge meaning.

## 6. Multipart atomic visibility

`MultipartInbox` authenticates incoming parts and retains incomplete publications internally.

A semantic projection is returned only when every required authenticated part is present.

Duplicate delivery of an identical part is harmless. A conflicting authenticated state for the same publication/index is rejected. A publication whose declared total changes is rejected.

For a complete publication, part projections are merged using normal semantic merge. Arrival order does not determine the user-visible result.

## 7. Protected two-replica convergence

The Rust integration suite now executes two independent replica state machines from the same scalar baseline.

They make concurrent changes:

```text
base R1
├── left  R10
└── right R20
```

Each side exports its dirty state, protects it with real XChaCha20-Poly1305, acknowledges only its own successful publication and then receives the two protected publications in opposite orders.

Both replicas finish with the same complete domain state and the same concurrent frontier:

```text
{R10, R20}
```

Neither side becomes dirty merely because it imported remote state. Transport/publication order does not affect convergence.

## 8. Optimistic publication race

A second Rust integration test uses an intentionally test-local in-memory CAS transport to exercise the publication race required by `GITHUB_TRANSPORT.md`.

The sequence is:

```text
A reads head R
B reads head R

A publishes protected state against R
        -> success, head RA

B publishes protected state against R
        -> conflict, current head RA

B fetches protected state introduced after R
B authenticates + merges A
B keeps its own unpublished domain dirty
B exports current merged dirty state
B protects a new publication
B publishes against RA
        -> success, head RB

A fetches protected state after RA
A authenticates + merges B retry
```

The test verifies that:

- a stale publication conflict does not clear B's local dirty contribution;
- B can incorporate A while retaining its own pending publication responsibility;
- B's retry contains sufficient state for A to converge;
- the final domain state is equal on both replicas;
- transport revision identities are used only for fetch/CAS bookkeeping and never for scalar ordering.

The in-memory CAS object is deliberately test-local. It is not a frozen transport trait and is not GitHub code.

## 9. Independent process exchange

A development process worker now allows protected sync bytes to cross an actual operating-system process boundary during tests.

Two separate producer processes independently construct left and right causal states and emit only AEAD-protected payload bytes. Two further processes then start from their corresponding local states, consume the protected payloads in opposite orders, authenticate/decode/merge them and emit deterministic clear projection encodings for test comparison.

The parent test verifies that:

- the exchanged payload files do not contain the known clear edit strings;
- both independent merge processes produce byte-identical deterministic final projections;
- the recovered frontier is `{R10, R20}`;
- materialization follows the ordinary scalar causal/tie-break rule, not process or delivery order.

This is the first process-level protected synchronization convergence test. It is still a development harness, not a transport protocol.

## 10. What is not solved by this layer

The current Rust sync implementation does not yet freeze or solve:

- final compact causal/checkpoint representation;
- baseline membership proofs for omitted historical parent bodies;
- lifecycle/tombstone sync semantics;
- sequence/hierarchy sync semantics;
- attachment chunk reachability and protected chunk manifests;
- content-key epoch selection inside sync envelopes;
- replica signatures/key evolution;
- replay/rollback policy;
- a generic production transport trait;
- GitHub object/commit serialization;
- GitHub authentication or repository discovery;
- foreground scheduling/backoff;
- long-offline transport-generation compaction.

The current scalar capsule may still carry more causal metadata than the eventual compact representation. Correctness is being established before compression.

## 11. Immediate next implementation work

The next transport-facing slice should keep the same separation and proceed in this order:

1. define the minimum opaque transport bookkeeping required by a foreground sync session without freezing GitHub into `apc-sync`;
2. implement GitHub optimistic head read / immutable protected-object publication / fast-forward retry as an adapter;
3. keep all decrypt/merge/retry policy above the adapter so GitHub never sees plaintext semantics;
4. add overlapping same-replica in-flight publication tests;
5. add missing multipart retry/resume tests;
6. add a foreground session scheduler with cancellation on background and immediate catch-up on resume;
7. then measure real request counts, latency and AEAD overhead before selecting cadence constants.

A.P.C. transport code should become boring by construction: move opaque authenticated objects and expose enough CAS/change-detection information for the trusted sync layer to do the real work.
