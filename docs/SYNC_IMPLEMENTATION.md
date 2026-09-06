# A.P.C. sync implementation

Status: **protected scalar sync, opaque transport seam, GitHub adapter, crash-consistent durable outbox/session recovery, foreground transport gating and the first typed semantic-exposure runtime bridge are implemented; portable sync encoding and production GitHub HTTP binding are not frozen**.

This document records the executable Rust synchronization boundary after the research model in `SYNC_EXPERIMENTS.md`. It does not replace `SYNC.md`, `SYNC_CAPSULES.md`, `GITHUB_TRANSPORT.md` or `DURABLE_SYNC.md`.

## 1. Repository layer

The Rust workspace currently contains:

```text
crates/apc-core/              semantic state, working/finalization and merge rules
crates/apc-crypto/            authenticated symmetric protection
crates/apc-sync/              transport-independent sync/session/recovery logic
crates/apc-transport-github/  GitHub-specific opaque transport adapter
crates/apc-storage-fs/        development local durability backend
crates/apc-runtime/           platform-neutral composition/exposure boundary
```

The intended dependency direction remains strict:

```text
semantic state
      |
sync projection / publication preparation
      |
AEAD protection
      |
durable outbox + cursor/state recovery
      |
OpaqueTransport
      |
GitHub adapter or another transport
      |
platform-neutral runtime composition
      |
Android / desktop platform binding
```

The diagram is dependency/ownership guidance rather than a requirement that bytes physically traverse every box in that textual order. In particular, runtime code composes already-defined core/sync/transport contracts; it does not become a second semantic layer.

GitHub code does not import clear semantic merge state. Filesystem durability does not decide merge semantics. Transport revision identities do not participate in causal ordering.

## 2. Semantic projection has no publication identity

The implemented `SyncProjection<K, S>` contains only merge-domain state:

```text
SyncProjection
└── domains
    ├── DomainKey -> mergeable state
    └── ...
```

It deliberately has no projection ID, publication ID, transport revision or timestamp.

The earlier Python research model used `max(projection_id)` while merging projections. That research-only ordering leak has been removed. Publication identity now exists only in protection/assembly/transport bookkeeping and cannot influence semantic merge.

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

## 7. Protected convergence and optimistic publication races

The Rust integration suite executes independent replica state machines from the same scalar baseline, creates concurrent changes, protects them with real XChaCha20-Poly1305 and consumes publications in opposite orders. Both sides converge to the same causal state and concurrent frontier. Neither side becomes dirty merely because it imported remote state.

The optimistic publication race is also exercised explicitly:

```text
A reads head R
B reads head R

A publishes against R
        -> success, head RA

B publishes against R
        -> conflict, current head RA

B fetches after R
B authenticates + merges A
B retains unpublished local work
B exports/protects a retry
B publishes against RA
        -> success, head RB

A fetches after RA
A authenticates + merges B retry
```

Transport revision identities are used only for fetch/CAS bookkeeping. They never decide scalar order.

## 8. Independent process exchange

A development process worker allows protected sync bytes to cross an actual operating-system process boundary during tests.

Separate producer processes independently construct causal states and emit only AEAD-protected payload bytes. Separate merge processes consume those payloads in different orders and emit deterministic clear projection encodings for comparison.

The parent test verifies that exchanged payload files do not contain the known clear edit strings and that the independent merge processes produce byte-identical final projections. This remains a development harness, not a transport protocol.

## 9. Opaque transport seam and GitHub adapter

`OpaqueTransport` is an executable transport-independent boundary with three operations:

```text
head()
fetch_since(known_revision)
publish(expected_revision, protected_objects)
```

Its revision type is deliberately opaque. A transport revision may be retained as a crash-recovery cursor but has no semantic ordering meaning.

`apc-transport-github` implements this seam. Protected wire objects are stored under content-addressed transport paths derived from SHA-256 of the complete already-protected bytes. The adapter verifies that such paths are append-only/immutable while traversing incremental commits.

Publication uses an expected-head CAS contract. A stale head returns `Conflict`; it never overwrites the winner. A missing/too-old/nonlinear baseline returns `BaselineUnavailable` rather than guessing that an incremental result is complete.

`apc-runtime::GitHubCursorCodec` now provides the concrete reversible conversion between `GitHubCommitOid` and the local opaque `TransportCursor` recovery representation. The representation is exact UTF-8 bytes of the opaque commit identity; lexical or numeric comparison remains meaningless to A.P.C. semantics. Invalid UTF-8 or invalid adapter identities fail closed.

The current `GitHubApi` remains an injectable API boundary. A concrete production HTTP/GraphQL binding and authentication flow are still open.

## 10. Durable session recovery

Transport success is not a local durability boundary. `DurableSyncRecord` therefore couples:

```text
trusted_state
+
applied transport cursor
+
pending outbound publications
```

into one crash-recovery unit.

Each `DurableOutboxEntry` stores the exact already-protected wire bytes plus the transport cursor against which they were prepared. Exact bytes survive restart so a retry never re-encrypts an allegedly equivalent publication into a new transport object accidentally.

The session coordinator provides:

- `stage_outbound()` — persist exposure/retry material before network I/O;
- `publish_staged()` — send only exact durable outbox bytes;
- `fetch_from_durable_cursor()` — fetch only from the cursor paired with durable local state;
- `commit_received()` — durably pair merged trusted state with the newly applied cursor;
- `commit_reconciled_outbox()` — retire one named pending publication only together with durable reconciliation;
- `commit_rebased_outbox()` — atomically replace one stale pending publication with a fresh identity/protected object set prepared against a newer cursor.

A response can be lost after remote acceptance. The implementation treats that as an unknown outcome: retain outbox, retry exact bytes, use stale-head conflict as evidence to refetch, then reconcile from durable facts.

`ProtectedSyncRecordStore` serializes the complete recovery record, authenticates/encrypts it, and sends only protected bytes through `commit_durable()` to the local durability backend.

## 11. Typed semantic exposure boundary

The first platform-neutral runtime bridge now closes the most dangerous gap between `FinalizationLedger::handoff()` and durable sync staging for the scalar path.

The implemented transition is:

```text
current LocalScalarDomain
        |
clone candidate
        |
candidate.handoff(revision_ids)
        |
validate all local causal dependencies are finalized
        |
encode candidate snapshot through TrustedStateCodec
        |
stage_outbound(
    encoded exposed trusted state,
    PublicationId,
    exact protected objects
)
        |
DURABILITY BARRIER
        |
replace caller's in-memory semantic domain
        |
network I/O may begin later
```

`stage_scalar_handoff()` does not mark the live in-memory semantic domain exposed before the durable record succeeds. If handoff validation, trusted-state encoding or durability fails, the caller's domain and sync record remain unchanged.

If a crash occurs after persistence but before the final in-memory assignment, restart uses the exposed semantic snapshot already paired with the durable outbox, so an externally observable causal identity cannot become private again merely because the process died.

`recover_scalar_domain()` restores the scalar domain from the exact trusted-state bytes paired with the recovery record. The `TrustedStateCodec` itself remains an explicit runtime seam: no final local recovery encoding is frozen by this bridge.

The bridge also refuses an unfinalized local dependency before any persistence occurs, preserving the finalization rule that every local identity in the handed-off transitive dependency closure must be frozen before transport exposure.

This is intentionally the first scalar-domain implementation path, not yet the final complete-continuum recovery codec or a general multi-domain publication transaction.

## 12. Overlapping pending publications

The outbox may contain more than one pending publication. Reconciling one does not rewrite the others.

This creates an important stale-outbox case:

```text
P1 expected R0
P2 expected R0

P1 reconciles -> durable cursor R1
P2 remains exact bytes expected R0
```

`P2` cannot be mutated in place and its `PublicationId` cannot be reused for different protected bytes. If semantic reconciliation shows that the contribution still needs publication, the higher layer must re-export/re-protect it under a fresh `PublicationId` against `R1`. `commit_rebased_outbox()` makes replacement of the stale durable entry crash-atomic.

Whether a stale entry is semantically redundant or must be rebased remains a semantic decision above transport bookkeeping.

## 13. Foreground-only lifecycle boundary

`ForegroundSyncLifecycle` and `ForegroundTransport<T>` encode the no-background-sync rule directly.

The gate starts closed. A platform binding must explicitly enter foreground before `head`, `fetch_since` or `publish` can reach the wrapped transport. Entering background blocks all future transport calls without modifying the durable outbox or cursor.

The gate intentionally does not claim that an already-running HTTP mutation can be made nonexistent. If the platform cancels a request after remote acceptance but before the response survives, the outcome is unknown and is recovered through the same durable outbox protocol.

The suite exercises exactly that sequence: remote acceptance, foreground→background transition, lost response, blocked retry while backgrounded, foreground resume, stale-head conflict and refetch of the accepted bytes.

No worker, daemon, alarm or background scheduler is required for correctness.

## 14. Current deterministic failure coverage

The executable matrix now checks at least these boundaries:

- failure to persist an outbound outbox does not expose a new in-memory/network-eligible state;
- ordinary network failure after durable staging preserves exact retry material;
- remote acceptance followed by response loss is recovered as unknown outcome;
- inbound merge persistence failure cannot advance only the process-local cursor;
- reconciliation persistence failure cannot retire outbox or advance cursor;
- reconciling one outbox entry preserves other pending entries verbatim;
- stale-entry rebase uses a fresh `PublicationId` and one durable replacement transition;
- failed rebase persistence restores the complete old state;
- publication identity cannot be reused for changed protected bytes;
- foreground/background transitions gate transport without touching semantic state;
- background during an accepted in-flight publication remains recoverable on resume;
- scalar semantic handoff and durable outbox staging advance together;
- failed scalar handoff persistence cannot expose only the process-local finalization ledger;
- an unfinalized local causal dependency cannot cross the runtime publication boundary;
- real development filesystem restart tests preserve the state/cursor/outbox pairing;
- the GitHub adapter participates in a lost-ACK restart/reconciliation integration test.

These tests establish the current Rust contracts, not real handset power-loss behavior.

## 15. Still intentionally unresolved

The current Rust sync implementation does not freeze or fully solve:

- final compact causal/checkpoint representation;
- baseline membership proofs for omitted historical parent bodies;
- final portable `.apc` encoding;
- final local trusted-state recovery encoding for the complete continuum;
- lifecycle/tombstone production sync semantics;
- sequence/hierarchy production sync semantics;
- attachment chunk reachability and protected chunk manifests;
- content-key epoch selection inside sync envelopes;
- replica signatures/key evolution;
- replay/rollback policy;
- general multi-domain finalization/exposure-to-publication preparation;
- production GitHub HTTP/GraphQL client, credentials and repository discovery;
- cancellation of already-running platform network requests;
- Android storage/lifecycle integration;
- long-offline transport-generation compaction.

The current scalar capsule may still carry more causal metadata than the eventual compact representation. Correctness is being established before compression.

## 16. Immediate next implementation work

The next implementation slices should keep the same separation:

1. replace the test-vault trusted-state codec with a deterministic versioned development codec for the scalar `LocalScalarSnapshot`, still explicitly pre-format;
2. extend the typed runtime bridge from one scalar domain toward a complete trusted local recovery image without inventing cross-domain atomic semantics;
3. extend failure injection across fetch failure, multi-entry stale rebase chains and repeated conflict/rebase cycles;
4. add a cancellable platform-network boundary while treating a cancelled mutation as unknown outcome unless reconciliation proves otherwise;
5. add foreground-resume orchestration that immediately executes durable-cursor catch-up/reconciliation;
6. carry the same test oracle onto Android through ADB before claiming handset power-loss guarantees.

A.P.C. transport code should remain boring by construction: move opaque authenticated objects and expose enough CAS/change-detection information for trusted semantic/sync code to do the real work.
