# A.P.C. sync implementation

Status: **protected scalar sync, opaque transport seam, GitHub adapter, crash-consistent durable recovery, foreground transport gating, deterministic local scalar recovery encoding, typed outbound/inbound boundaries, durable-cursor catch-up, single- and multi-publication foreground resume, authenticated lost-ack reconciliation and bounded conflict follow-up are implemented; portable format, multipart runtime catch-up and production GitHub HTTP binding remain unfrozen**.

This document records the executable Rust synchronization boundary after the research model in `SYNC_EXPERIMENTS.md`. It does not replace `SYNC.md`, `SYNC_CAPSULES.md`, `GITHUB_TRANSPORT.md` or `DURABLE_SYNC.md`.

## 1. Repository layers

The Rust workspace currently contains:

```text
crates/apc-core/              semantic state, working/finalization and merge rules
crates/apc-crypto/            authenticated symmetric protection
crates/apc-sync/              transport-independent sync/session/recovery logic
crates/apc-transport-github/  GitHub-specific opaque transport adapter
crates/apc-storage-fs/        development opaque-byte durability backend
crates/apc-runtime/           platform-neutral composition boundary
```

The ownership rule is stricter than textual call order:

```text
semantic state decides meaning
crypto decides confidentiality/authenticity
sync decides projection/session/recovery protocol
storage decides durable opaque-byte commit
transport moves protected opaque objects
runtime composes those contracts
platform code drives lifecycle/UI/network bindings
```

`apc-storage-fs` stores opaque bytes only. Local semantic recovery encoding belongs in `apc-runtime`, above storage and below platform bindings.

GitHub code does not import clear semantic merge state. Transport revision identities do not participate in causal ordering.

## 2. Semantic projection remains free of transport identity

`SyncProjection<K, S>` contains merge-domain state only:

```text
SyncProjection
└── DomainKey -> mergeable state
```

It has no publication ID, transport revision, timestamp or arrival-order field. The earlier Python research-model `max(projection_id)` leak is absent from the Rust path.

For the first scalar path, `DomainKey` is currently:

```text
AtomId + pre-format domain identifier bytes
```

The final portable domain namespace remains open.

`APCSYNC1` is the deterministic development codec for `ScalarSyncProjection`. It serializes domain keys and scalar causal state with direct parent IDs. It is explicitly **not** the native `.apc` format or a compatibility promise.

## 3. Protection and multipart visibility

`ProtectedSyncPart` carries clear assembly bookkeeping plus authenticated ciphertext:

```text
PublicationId
part_index
total_parts
ciphertext
```

`PublicationId` is a 256-bit opaque transport/multipart identity. Its byte order has no temporal, causal or priority meaning.

`protect_scalar_part()` binds `ContinuumId`, `PublicationId`, part index and total part count into AEAD associated data. Tampering with clear bookkeeping therefore fails authentication before semantic merge.

`APCSPRT1` is the current development wire framing for protected parts.

`MultipartInbox` authenticates and accumulates parts per `PublicationId`. It exposes no `ScalarSyncProjection` before all declared parts exist. Duplicate identical parts are harmless; conflicting authenticated state or inconsistent totals fail closed.

The current runtime catch-up path still consumes complete single-part scalar objects. The next receive slice must place `MultipartInbox` above durable-cursor advancement so a fetched range containing an incomplete publication cannot be partially made visible and then skipped by committing a newer cursor.

## 4. Opaque transport and GitHub

`OpaqueTransport` exposes only:

```text
head()
fetch_since(known_revision)
publish(expected_revision, protected_objects)
```

The revision type is transport bookkeeping only.

`apc-transport-github` stores protected objects under content-addressed paths derived from SHA-256 of the complete already-protected wire bytes. Incremental traversal rejects mutation of an existing protected-object path. Publication uses expected-head CAS behavior; stale publication returns `Conflict`, and an unknown/too-old/nonlinear baseline returns `BaselineUnavailable` rather than pretending an incomplete fetch is complete.

`apc-runtime::GitHubCursorCodec` reversibly maps `GitHubCommitOid` to local opaque `TransportCursor` bytes. Lexical/numeric ordering of those bytes remains meaningless to A.P.C. semantics.

The concrete production GitHub HTTP/GraphQL client and credential flow are still open.

## 5. Durable session state

`DurableSyncRecord` crash-atomically couples:

```text
trusted semantic/recovery state
+
applied transport cursor
+
pending outbound publications
```

Each outbox entry retains exact protected wire bytes and the cursor against which those bytes were prepared.

The session/recovery layer currently provides:

- `stage_outbound()` — persist exposed trusted state plus exact retry bytes before network I/O;
- `publish_staged()` — send one named publication using only exact durable bytes;
- `fetch_from_durable_cursor()` — fetch from the cursor paired with durable state;
- `commit_received()` — commit merged trusted state and new cursor together;
- `commit_reconciled_outbox()` — retire one named publication through a durability barrier;
- `commit_rebased_outbox()` — replace one stale publication with a fresh `PublicationId`, fresh protected bytes and a newer cursor in one durable transition;
- `publish_staged_batch()` — publish a non-empty set of exact durable publications only when every member has the same expected cursor;
- `commit_reconciled_outbox_batch()` — retire a selected publication set in one clone-persist-swap transition.

Transport success is never itself a local durability boundary.

### 5.1 Set batching does not create ID order

`publish_staged_batch()` treats supplied publication IDs as a mathematical set. Canonical ID byte order is used only to produce deterministic transport input. It is not retry priority or recency.

A mixed expected-cursor set is rejected before transport I/O:

```text
P1 @ R0
P2 @ R1
   |
   X  not one batch
```

No cursor byte comparison is used to decide which is older. The only batch relation is exact cursor equality.

## 6. Deterministic local scalar recovery encoding

`apc-runtime::DevelopmentScalarTrustedStateCodec` provides the current scalar restart representation.

Its development framing is `APCLREC1`:

```text
working state
├── causal register
└── optional WorkingEpoch
    ├── WorkingEpochId
    ├── current local value
    └── observed frontier

finalization state
├── locally-owned RevisionIds
├── FinalizedStatements
├── exposed local RevisionIds
└── directly handed-off local RevisionIds
```

Encoding and decoding validate through `LocalScalarDomain::restore()`. An invalid pending frontier or inconsistent finalization/exposure bookkeeping cannot become valid merely because it was serialized.

The current development encodings remain distinct:

```text
APCLREC1   local scalar trusted-state recovery
APCSREC1   durable sync record
APCSYNC1   clear scalar sync projection
APCSPRT1   protected sync-part wire framing
.apc       final portable native format — not frozen
```

## 7. Typed outbound semantic-to-wire boundary

`prepare_scalar_handoff()` receives:

```text
LocalScalarDomain<Vec<u8>>
DomainKey
ContinuumId
PublicationId
ContentKey
selected RevisionIds
```

and performs:

```text
clone semantic domain
        |
candidate.handoff(selected RevisionIds)
        |
prove every locally-owned dependency in closure is finalized
        |
compute exact causal dependency closure
        |
build one-domain ScalarSyncProjection
        |
protect_scalar_part()
        |
encode_protected_sync_part()
        |
PreparedScalarHandoff {
    selected RevisionIds,
    exact protected wire object
}
```

The dependency-closure step prevents an unrelated concurrent branch stored in the same register from being exposed merely because local revision `L` is selected for publication.

`stage_prepared_scalar_handoff()` then couples semantic exposure and exact retry material:

```text
clone live semantic domain
        |
record handoff/exposure on candidate
        |
encode candidate trusted state
        |
persist candidate trusted state + exact protected outbox
        |
LOCAL DURABILITY BARRIER
        |
replace live semantic domain
```

An unfinalized local dependency fails before protection. A durability failure leaves both the live domain and durable sync record unchanged.

## 8. Typed inbound authenticated-observation boundary

`decode_single_scalar_domain_object()` decodes the protected wire part, verifies one-part shape, performs AEAD authentication/context verification and extracts exactly the expected scalar domain.

`ReceivedScalarState` couples:

```text
authenticated remote ScalarRegister
pre-observation local RevisionId, if dirty work must be sealed
new transport head
```

`commit_received_scalar_domain()` executes:

```text
clone live LocalScalarDomain
        |
candidate.observe_remote(remote, pre-observation RevisionId)
        |
if dirty:
    seal local WorkingEpoch using frontier it actually observed
        |
merge authenticated remote state
        |
encode candidate APCLREC1 trusted state
        |
commit_received(candidate trusted state, new cursor)
        |
LOCAL DURABILITY BARRIER
        |
replace live semantic domain
```

Network receipt therefore does not retroactively become a causal parent of local work that began before the remote state was semantically observable.

## 9. Durable-cursor catch-up

`catch_up_single_scalar_domain()` always fetches from the cursor paired with durable trusted state.

For a changed range it authenticates the returned scalar objects, merges decoded remote registers, then crosses one semantic observation boundary. Dirty local work is sealed once against the frontier it actually observed, not once per transport object.

A transport-head advance containing no A.P.C. semantic objects advances the durable cursor together with the unchanged trusted snapshot without manufacturing a local causal revision.

For an authenticated semantic change, `ScalarCatchUpOutcome::Applied` also reports the exact `RevisionId`s present in the authenticated remote register assembled during that pass. Those IDs are transport-observation evidence for lost-ack reconciliation; they are not causal order and are not inferred from local state.

## 10. Foreground resume: narrow path

`resume_single_scalar_domain()` composes catch-up with one pending scalar publication:

```text
catch up from durable cursor
        |
authenticate + merge + durably advance cursor
        |
inspect pending exact publication
        |
├─ expected cursor still current -> retry exact durable bytes
├─ stale + full authenticated rediscovery proof -> retire, no republish
├─ stale without proof -> NeedsRebase
└─ baseline unavailable -> stop safely
```

`resume_single_scalar_domain_bounded()` allows exactly one follow-up pass after a transport CAS conflict. There is no unbounded `while conflict` loop.

This narrow path remains useful as a small executable oracle even though the runtime now also has a set-based path.

## 11. Foreground resume: publication sets

`resume_scalar_outbox_set()` handles any number of pending scalar publications without selecting one by `PublicationId` order.

It snapshots the pending ID set, performs catch-up, then classifies entries using only exact equality with the resulting durable cursor:

```text
entry.expected_cursor == applied_cursor -> current set
entry.expected_cursor != applied_cursor -> stale set
```

For stale entries, the runtime may decode the exact pending single-part scalar wire object and compare its complete carried causal revision set against `observed_revision_ids` from that exact authenticated catch-up.

Every stale publication proven observed is retired in one durable reconciliation set. If any stale publication remains unresolved, the runtime returns:

```text
NeedsRebase { unresolved stale PublicationIds }
```

and does not publish the current set. This conservative stop avoids inventing cross-generation scheduling semantics.

If all stale entries are resolved, the current equal-cursor set is published through one `publish_staged_batch()` call. Successful publication is followed by one `commit_reconciled_outbox_batch()` durability barrier; conflict leaves the whole set pending.

`resume_scalar_outbox_set_bounded()` adds exactly one follow-up set-based pass after a batch conflict. One foreground callback can therefore perform at most one batch publication attempt plus one catch-up/reclassification pass.

## 12. Lost acknowledgement and durability evidence

### 12.1 Single publication

The filesystem restart test stages an exposed scalar publication into an AEAD-protected durable record, simulates remote acceptance followed by process death before acknowledgement handling, reopens the Unix backend, runs resume, rediscovers the accepted publication through authenticated catch-up, retires the outbox without a second publish, then reopens again and verifies the reconciled cursor/state/outbox.

A separate failure-injection test forces the post-catch-up outbox-retirement durability barrier to fail. The caught-up `{trusted state, new cursor, pending outbox}` record remains the complete durable state.

### 12.2 Publication batch

The batch recovery test stages two typed scalar handoffs at the same cursor and then executes:

```text
P1 @ R1 ─┐
P2 @ R1 ─┴─> one batch publish
              |
              remote accepts -> R2
              |
              local batch-retirement persist fails
```

The local `DurableSyncRecord` remains at `R1` with both exact pending publications even though the transport has accepted them at `R2`.

The next resume fetches `R1 -> R2`, authenticates the accepted protected objects, proves both pending causal closures, durably advances the semantic state/cursor and retires the two publications as one observed set. The transport publish count remains one.

This verifies that batch acknowledgement uncertainty has the same safe asymmetry as the narrow single-publication path.

## 13. Conflict evidence and bounded work

The single-publication conflict test creates local revision `200` and concurrent remote revision `900`. One publish loses the CAS race; one follow-up catch-up authenticates `900`, advances the cursor and ends at explicit `NeedsRebase` for the still-needed local publication.

The set-based conflict test stages two entries in reverse ID order. They are treated as one equal-cursor set, the batch loses one CAS race, one follow-up catch-up advances to the competing head, and the cycle stops with both IDs in a `NeedsRebase` set. There are two fetch calls and one publish call. No opaque ID becomes hidden retry priority.

## 14. Foreground-only lifecycle boundary

`ForegroundSyncLifecycle` and `ForegroundTransport<T>` start transport closed. Platform code must explicitly enter foreground before `head`, `fetch_since` or `publish` can reach transport. Entering background blocks future transport calls without changing semantic state, cursor or outbox.

An already-running mutation may already have succeeded remotely when the application backgrounds. Lifecycle state does not guess the result; durable outbox plus authenticated foreground recovery determines what happened.

No worker, daemon, foreground service, alarm or background scheduler is required for correctness.

The Android lifecycle binding that invokes these runtime paths on actual foreground entry remains open.

## 15. Current failure coverage

The Rust suite covers, among other cases:

- outbound outbox persistence failure;
- network failure after durable staging;
- remote acceptance followed by lost response;
- inbound merged-state persistence failure;
- reconciliation persistence failure;
- multiple simultaneous outbox entries;
- stale-entry replacement with a fresh publication identity;
- failed stale-entry replacement;
- foreground/background transport gating;
- backgrounding during an accepted in-flight mutation;
- background catch-up blocked before transport I/O;
- foreground catch-up applying authenticated remote state once;
- transport-head advance with no semantic objects and no false local causal seal;
- unfinalized local dependency rejected before outbound protection;
- outbound semantic exposure + exact protected outbox committed together;
- deterministic scalar recovery snapshot validation;
- real encrypted filesystem restart for outbound exposure/outbox;
- authenticated inbound dirty observation + cursor advance committed together;
- real encrypted filesystem restart preserving true local/remote concurrency;
- current exact outbox retry followed by durable retirement;
- stale outbox detection without mutating/reusing its bytes;
- authenticated lost-ack reconciliation without second publish;
- failure of the post-catch-up retirement barrier preserving pending outbox;
- same-cursor batch publication with deterministic transport input but no ID priority;
- mixed-cursor batch rejection before network I/O;
- atomic batch reconciliation preserving unselected entries;
- failed batch reconciliation preserving every selected entry;
- set-based runtime publication of several pending entries in one transport mutation;
- unresolved stale set blocking a current set without implicit ordering;
- authenticated reconciliation of an entire stale lost-ACK set;
- bounded batch conflict followed by exactly one catch-up/reclassification pass;
- remote batch acceptance followed by failed local reconciliation and later authenticated set recovery with no second publish.

## 16. Still intentionally unresolved

The executable path does **not** freeze or fully solve:

- final compact causal/checkpoint representation and old-baseline membership proofs;
- native `.apc` binary layout;
- complete-continuum local trusted-state encoding;
- production lifecycle/tombstone semantics;
- production sequence/hierarchy semantics;
- attachment chunk manifests/reachability;
- content-key epoch selection;
- replica signatures and key evolution;
- replay/rollback policy;
- truly irreducible multi-domain atomic publication semantics;
- multipart runtime catch-up above `MultipartInbox`;
- automatic semantic re-export/rebase of unresolved stale publication sets across cursor generations;
- durable preservation of authenticated lost-ack proof across an additional crash between catch-up commit and later outbox retirement;
- production GitHub HTTP/GraphQL client and credentials;
- cancellation bridge for an already-running platform request;
- Android filesystem/lifecycle validation;
- long-offline transport generation/checkpoint retention.

The current scalar capsules may retain more causal material than the eventual compact format. Correctness remains ahead of compression.

## 17. Immediate next implementation work

The next slices should move outward from the now-tested set-based scalar resume path:

1. wire complete authenticated multipart assembly into durable-cursor catch-up while refusing to advance the cursor past any incomplete publication;
2. add repeated multi-publication conflict/rebase-chain and fetch-failure adversarial tests;
3. introduce the first complete local trusted-state container abstraction for several independent merge domains without accidental cross-domain transaction semantics;
4. keep semantic stale-set re-export above transport/session code and require a fresh `PublicationId` for every replacement;
5. keep the GitHub API injectable until runtime/session invariants stabilize, then add the concrete production GitHub binding;
6. carry the same restart/failure oracle onto Android through ADB before claiming handset power-loss guarantees.

A.P.C. transport remains deliberately boring: move opaque authenticated objects and expose enough CAS/change-detection information for trusted semantic/runtime code to decide meaning.
