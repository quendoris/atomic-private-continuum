# A.P.C. sync implementation

Status: **protected scalar sync, opaque transport seam, GitHub adapter, crash-consistent durable recovery, foreground transport gating, deterministic local scalar recovery encoding, typed outbound publication preparation and typed inbound observation are implemented; portable format and production GitHub HTTP binding remain unfrozen**.

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

The ownership rule is stricter than the textual call order:

```text
semantic state decides meaning
crypto decides confidentiality/authenticity
sync decides projection/session/recovery protocol
storage decides durable opaque-byte commit
transport moves protected opaque objects
runtime composes those contracts
platform code drives lifecycle/UI/network bindings
```

`apc-storage-fs` no longer contains a duplicate semantic snapshot codec. It stores opaque bytes only. Local semantic recovery encoding belongs in `apc-runtime`, above storage and below platform bindings.

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

`protect_scalar_part()` binds `ContinuumId`, `PublicationId`, part index and total part count into AEAD associated data. Tampering with that clear bookkeeping therefore fails authentication before semantic merge.

`APCSPRT1` is the current development wire framing for protected parts.

`MultipartInbox` does not expose semantic state until all required authenticated parts exist. Duplicate identical delivery is harmless; conflicting part state or inconsistent totals fail closed. Multipart assembly remains independent of transport arrival order.

The first runtime receive helper is intentionally narrower: `decode_single_scalar_domain_object()` accepts only one complete `part_index=0,total_parts=1` object containing exactly the expected domain. It refuses multipart or unexpected-domain material instead of making partial state observable.

## 4. Opaque transport and GitHub

`OpaqueTransport` exposes only:

```text
head()
fetch_since(known_revision)
publish(expected_revision, protected_objects)
```

The revision type is transport bookkeeping only.

`apc-transport-github` stores protected objects under content-addressed paths derived from SHA-256 of the complete already-protected wire bytes. Incremental traversal rejects mutation of an existing protected-object path. Publication uses expected-head CAS behavior; stale publication returns `Conflict`, and an unknown/too-old/nonlinear baseline returns `BaselineUnavailable` rather than pretending an incomplete incremental fetch is complete.

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

Each outbox entry retains its exact protected wire bytes and the cursor against which those bytes were prepared.

The session coordinator currently provides:

- `stage_outbound()` — persist exposed trusted state plus exact retry bytes before network I/O;
- `publish_staged()` — send only exact durable bytes;
- `fetch_from_durable_cursor()` — fetch from the cursor paired with durable state;
- `commit_received()` — commit merged trusted state and new cursor together;
- `commit_reconciled_outbox()` — retire one named publication only with durable reconciliation;
- `commit_rebased_outbox()` — replace one stale outbox entry with a fresh `PublicationId`, fresh protected bytes and newer cursor in one durable transition.

Transport success is never itself a local durability boundary. A lost response after remote acceptance remains an unknown outcome: exact outbox bytes survive, retry may reveal a stale-head conflict, and refetch/reconciliation resolves the result from durable facts.

## 6. Deterministic local scalar recovery encoding

`apc-runtime::DevelopmentScalarTrustedStateCodec` now replaces the earlier in-memory test-vault assumption for the real scalar restart path.

Its current development framing is `APCLREC1`. It encodes the complete `LocalScalarSnapshot<Vec<u8>>` required by the current scalar runtime path:

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

Encoding and decoding both validate the snapshot through `LocalScalarDomain::restore()`. An invalid pending frontier or inconsistent finalization/exposure bookkeeping therefore cannot become valid merely because it was serialized.

`APCLREC1` is local development recovery framing only. It is not `.apc`, not `APCSYNC1`, and not frozen compatibility.

## 7. Typed outbound semantic-to-wire boundary

The first scalar outbound path no longer accepts an arbitrary pair of “revision IDs” and “already protected bytes” from application glue.

`prepare_scalar_handoff()` receives:

```text
LocalScalarDomain<Vec<u8>>
DomainKey
ContinuumId
PublicationId
ContentKey
selected RevisionIds
```

and performs these steps:

```text
clone semantic domain
        |
candidate.handoff(selected RevisionIds)
        |
prove every locally-owned dependency in the causal closure is finalized
        |
compute exact causal dependency closure
        |
build one-domain ScalarSyncProjection from that closure only
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

The dependency-closure step matters. If the local register also contains an unrelated concurrent remote revision, publishing local revision `L` does not automatically expose that unrelated branch merely because both happen to be stored in the same register.

`stage_prepared_scalar_handoff()` then consumes the coupled `PreparedScalarHandoff`, clones the live semantic domain, records handoff/exposure on the candidate, encodes the exposed trusted snapshot, persists that snapshot together with the exact protected outbox, and only after the durability barrier replaces the live in-memory domain.

Therefore the dangerous state cannot occur:

```text
RAM says RevisionId is exposed
persistent recovery state says it is private
network may already have received it
```

An unfinalized local dependency fails before encryption/publication preparation. A durability failure leaves both the live semantic domain and durable sync record unchanged.

## 8. Typed inbound authenticated-observation boundary

The inverse scalar path now exists as an executable runtime boundary.

First, `decode_single_scalar_domain_object()` performs wire decode, AEAD authentication, `ContinuumId`/publication context verification and exact expected-domain extraction.

Then `ReceivedScalarState` bundles three values that must not drift apart in application glue:

```text
authenticated remote ScalarRegister
pre-observation local RevisionId, if dirty local work must be sealed
new transport head
```

`commit_received_scalar_domain()` executes:

```text
clone live LocalScalarDomain
        |
candidate.observe_remote(remote, pre-observation RevisionId)
        |
if dirty:
    seal local WorkingEpoch using the frontier it actually observed
        |
merge authenticated remote state
        |
encode candidate APCLREC1 trusted state
        |
commit_received(candidate trusted state, new transport cursor)
        |
LOCAL DURABILITY BARRIER
        |
replace live semantic domain
```

This preserves the earlier working-state research result: network receipt does not retroactively become a causal parent of local work that began before the remote state was semantically observed.

If durable receive persistence fails, the live domain remains dirty, the pre-observation local revision is not created in live state, and the old durable cursor remains authoritative.

## 9. Real vertical restart tests

`crates/apc-runtime/tests/vertical_recovery.rs` now exercises both directions across real crate boundaries rather than only in-memory mocks.

### 9.1 Outbound vertical slice

The test executes:

```text
LocalScalarDomain
→ exact selected causal dependency closure
→ ScalarSyncProjection
→ XChaCha20-Poly1305 protected sync part
→ APCSPRT1 wire
→ PreparedScalarHandoff
→ exposed APCLREC1 trusted state + exact outbox
→ ProtectedSyncRecordStore
→ XChaCha20-Poly1305 local record protection
→ UnixFsDurabilityBackend
→ close/reopen
→ decrypt recovery record
→ decode/restore LocalScalarDomain
→ decode/unprotect exact outbox wire
```

It verifies that finalized/exposed/handoff bookkeeping, pending local working state, transport cursor and exact protected retry bytes survive restart. Raw committed filesystem bytes are checked not to contain selected known plaintext strings or the clear `APCLREC1` marker.

### 9.2 Inbound vertical slice

A second test creates one remote finalized scalar revision and one dirty local working epoch from the same base, protects the remote publication through the real outbound builder, authenticates/decodes it on the receiver, then commits semantic observation with a newer GitHub transport cursor through the real protected filesystem record store.

The dirty local epoch is sealed with the old observed frontier before remote merge. After close/reopen the recovered causal frontier contains both local and remote revisions as genuinely concurrent branches; neither is an ancestor of the other. The new transport cursor and the exact semantic state survive together. Raw filesystem bytes again do not contain the known local/remote plaintexts.

These are process/restart and Unix durability-contract proofs, not yet physical Android power-loss proofs.

## 10. Overlapping outbox and stale publication replacement

Multiple pending publications may coexist. Reconciling one does not mutate another.

If `P1` and `P2` were prepared at `R0` and reconciling `P1` advances the durable cursor to `R1`, `P2` remains the exact historical bytes it was at `R0`. It cannot be mutated or reused under the same `PublicationId`.

If semantic reconciliation says its contribution is still needed, a higher layer produces new protected bytes under a fresh `PublicationId`, and `commit_rebased_outbox()` commits stale retirement plus fresh replacement atomically. If the old publication became redundant, it can instead be retired by the appropriate reconciliation path.

## 11. Foreground-only lifecycle boundary

`ForegroundSyncLifecycle` and `ForegroundTransport<T>` start transport closed. A platform binding must explicitly enter foreground before `head`, `fetch_since` or `publish` can reach the wrapped transport. Entering background blocks future transport calls without changing semantic state, cursor or outbox.

An already-running mutation may already have succeeded remotely when the application backgrounds. The lifecycle gate therefore does not invent an outcome. The failure suite exercises remote acceptance + lost response + background blocking + foreground retry + stale-head conflict + refetch.

No worker, daemon, foreground service, alarm or background scheduler is required for correctness.

## 12. Current failure coverage

The Rust suite currently covers, among other cases:

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
- unfinalized local dependency rejected before outbound protection;
- outbound semantic exposure + exact protected outbox committed together;
- deterministic scalar recovery snapshot validation;
- real encrypted filesystem restart for outbound exposure/outbox;
- authenticated inbound scalar observation + cursor advance committed together;
- real encrypted filesystem restart preserving true local/remote concurrency after dirty observation;
- GitHub cursor round-trip without semantic ordering.

## 13. Still intentionally unresolved

The executable scalar path does **not** freeze or fully solve:

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
- multipart runtime receive orchestration above `MultipartInbox`;
- production GitHub HTTP/GraphQL client and credentials;
- cancellation bridge for an already-running platform request;
- foreground-resume orchestration;
- Android filesystem/lifecycle validation;
- long-offline transport generation/checkpoint retention.

The current scalar capsules may retain more causal material than the eventual compact format. Correctness remains ahead of compression.

## 14. Immediate next implementation work

The next slices should now move outward from the proven scalar vertical path rather than rebuilding it:

1. build a small runtime sync-session orchestrator that combines foreground resume, durable-cursor fetch, authenticated decode/merge and pending-outbox retry through the existing transitions;
2. add deterministic failure tests for fetch failure and repeated conflict/rebase cycles with several pending publications;
3. extend receive orchestration to complete authenticated multipart publications without permitting partial semantic visibility;
4. introduce the first complete local trusted-state container abstraction for several independent merge domains while explicitly avoiding accidental cross-domain transaction semantics;
5. keep the GitHub API injectable until the runtime/session invariants are stable, then add the concrete production GitHub binding;
6. carry the same restart/failure oracle onto Android through ADB before claiming handset power-loss guarantees.

A.P.C. transport remains deliberately boring: move opaque authenticated objects and expose enough CAS/change-detection information for trusted semantic/runtime code to decide meaning.
