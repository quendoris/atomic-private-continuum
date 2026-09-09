# A.P.C. sync implementation

Status: **protected scalar sync, opaque transport seam, GitHub adapter, crash-consistent durable recovery, foreground transport gating, deterministic local scalar recovery encoding, typed outbound/inbound boundaries, complete authenticated multipart durable-cursor catch-up, single- and multi-publication foreground resume, authenticated lost-ack reconciliation, bounded conflict follow-up and a JNI/Android process-death recovery harness are implemented; portable format, production GitHub HTTP binding and real-device power-loss validation remain unfrozen**.

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
crates/apc-android-bridge/    narrow JNI/lifecycle/device-harness boundary
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

`apc-android-bridge` does not redefine semantic or sync state. Its current purpose is to translate Android lifecycle/process test calls into already-defined Rust boundaries and expose development recovery probes to the minimal APK harness under `android/harness/`.

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

`MultipartInbox` authenticates and accumulates parts per `PublicationId`. It exposes no `ScalarSyncProjection` before all declared parts exist. Duplicate identical parts while a publication is pending are harmless; conflicting authenticated state or inconsistent totals fail closed.

`apc-runtime::decode_complete_scalar_domain_objects()` now places that assembly boundary above one fetched transport range. It consumes every protected object in the range, authenticates and assembles every publication, and returns scalar state only if no publication remains incomplete after the entire range has been consumed.

The consequence is deliberate:

```text
complete publication A
+
incomplete publication B
inside one fetched range
        |
        v
no semantic visibility from either publication
no durable cursor advance
```

The ephemeral inbox is discarded on failure. The old durable cursor remains authoritative so the same range can be refetched instead of persisting partial semantic visibility.

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

The complete multi-domain recovery path uses `DevelopmentMultiScalarTrustedStateCodec` and its current `APCLSET1` framing to retain several independent scalar merge domains in one physical recovery image. That co-persistence is crash-atomic bookkeeping; it does not create a multi-domain semantic transaction or global causality.

The current development encodings remain distinct:

```text
APCLREC1   local scalar trusted-state recovery
APCLSET1   multi-domain local recovery container
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

The multi-domain path applies the same rule through `prepare_recovery_handoff()` and `stage_prepared_recovery_handoff()`: selected causal closures from independent domains are protected and their exposure/outbox state is persisted together without turning physical co-persistence into cross-domain causal semantics.

## 8. Typed inbound authenticated-observation boundary

`decode_single_scalar_domain_object()` remains the narrow inverse for a code path that explicitly requires one complete single-part publication.

The durable range path uses `decode_complete_scalar_domain_objects()`, which wire-decodes, authenticates and completely assembles every multipart publication in the fetched range before returning any scalar register.

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

The multi-domain recovery catch-up keys pre-observation revision identities by `DomainKey`. A remote change in one merge domain cannot seal dirty work or manufacture causality in an unrelated domain.

## 9. Durable-cursor catch-up

`catch_up_single_scalar_domain()` always fetches from the cursor paired with durable trusted state.

For a changed range it now performs:

```text
fetch from durable cursor
        |
decode every protected object
        |
authenticate + assemble every publication
        |
any incomplete multipart publication?
      /              \
    yes              no
     |                |
fail before        merge all fully
semantic mutation authenticated scalar state
or cursor advance      |
                    one semantic observation
                        |
                    one durability barrier
```

All complete scalar registers are merged before the single semantic observation boundary. A dirty local working epoch is therefore sealed once against the frontier it actually observed, not once per wire object, part, publication or transport completion order.

A transport-head advance containing no A.P.C. semantic objects advances the durable cursor together with the unchanged trusted snapshot without manufacturing a local causal revision.

For an authenticated semantic change, catch-up reports the exact remote `RevisionId`s present in fully assembled authenticated state, grouped by merge domain in the multi-domain path. Those IDs are transport-observation evidence for lost-ack reconciliation; they are not causal order and are not inferred from local state.

The `object_count` field counts protected transport wire objects consumed. A two-part logical publication therefore contributes two objects without being treated as two semantic publications.

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

This narrow path remains useful as a small executable oracle even though the runtime now also has a set-based multi-domain path.

## 11. Foreground resume: publication sets

`resume_scalar_recovery_outbox_set()` handles any number of pending publications in the complete multi-domain recovery record without selecting one by `PublicationId` order.

It snapshots the pending ID set, performs catch-up, then classifies entries using only exact equality with the resulting durable cursor:

```text
entry.expected_cursor == applied_cursor -> current set
entry.expected_cursor != applied_cursor -> stale set
```

For stale entries, the runtime decodes the exact durable protected publication and requires full authenticated revision evidence for every semantic merge domain carried by that publication before it may be retired as already observed.

Every stale publication proven observed is retired in one durable reconciliation set. If any stale publication remains unresolved, the runtime returns:

```text
NeedsRebase { unresolved stale PublicationIds }
```

and does not publish the current set. This conservative stop avoids inventing cross-generation scheduling semantics.

If all stale entries are resolved, the current equal-cursor set is published through one `publish_staged_batch()` call. Successful publication is followed by one `commit_reconciled_outbox_batch()` durability barrier; conflict leaves the whole set pending.

`resume_scalar_recovery_outbox_set_bounded()` adds exactly one follow-up pass after a batch conflict. One foreground callback can therefore perform at most one batch publication attempt plus one catch-up/reclassification pass.

## 12. Lost acknowledgement and durability evidence

### 12.1 Single publication

The filesystem restart test stages an exposed scalar publication into an AEAD-protected durable record, simulates remote acceptance followed by process death before acknowledgement handling, reopens the Unix backend, runs resume, rediscovers the accepted publication through authenticated catch-up, retires the outbox without a second publish, then reopens again and verifies the reconciled cursor/state/outbox.

A separate failure-injection test forces the post-catch-up outbox-retirement durability barrier to fail. The caught-up `{trusted state, new cursor, pending outbox}` record remains the complete durable state.

### 12.2 Publication batch and multi-domain recovery

The batch recovery tests stage multiple typed handoffs at the same cursor and exercise remote acceptance followed by failed local reconciliation. The local `DurableSyncRecord` keeps all exact pending publications and the old cursor until the next authenticated catch-up proves the accepted closures. Retirement then occurs as a durable set transition, and the transport publish count remains unchanged.

The multi-domain restart path additionally proves that exposure/finalization state for independent domains, cursor and exact outbox bytes survive encrypted filesystem reopen together without creating cross-domain causality.

### 12.3 Android development lost-ack probe

`apc-android-bridge` now exposes the same uncertainty pattern to the minimal Android harness. The host test builds a real finalized local scalar revision, stages it through `prepare_recovery_handoff()` / `stage_prepared_recovery_handoff()`, persists the protected `DurableSyncRecord` through `ProtectedSyncRecordStore<UnixFsDurabilityBackend>`, and sends its exact protected object through a harness-only `OpaqueTransport`.

That simulated remote first durably records the accepted object and increments a durable publish counter, then deliberately returns an error instead of the acknowledgement. Local state therefore remains at the old cursor with the outbox pending. A separately opened recovery phase fetches from that durable cursor, authenticates the already accepted protected object, reconciles the lost acknowledgement, advances the local cursor and requires the remote publish counter to remain exactly `1`.

The same routines are reachable from the APK as `lost-ack-stage` and `lost-ack-resume`. They are deliberately split so `adb shell am force-stop` can terminate the entire Android process between the uncertain remote outcome and recovery.

Host CI proves the Rust logic and real Unix development filesystem path. It does **not** prove that the APK has been built, installed or run on a physical Android device; that requires the ADB campaign described in `android/harness/README.md`.

## 13. Conflict evidence and bounded work

The single-publication conflict test creates local revision `200` and concurrent remote revision `900`. One publish loses the CAS race; one follow-up catch-up authenticates `900`, advances the cursor and ends at explicit `NeedsRebase` for the still-needed local publication.

The set-based conflict test stages multiple entries as one equal-cursor set. The batch loses one CAS race, one follow-up catch-up advances to the competing head, and the cycle stops with the unresolved IDs in a `NeedsRebase` set. There are at most two fetch passes around one publication attempt. No opaque ID becomes hidden retry priority.

## 14. Foreground-only lifecycle and Android boundary

`ForegroundSyncLifecycle` and `ForegroundTransport<T>` start transport closed. Platform code must explicitly enter foreground before `head`, `fetch_since` or `publish` can reach transport. Entering background blocks future transport calls without changing semantic state, cursor or outbox.

An already-running mutation may already have succeeded remotely when the application backgrounds. Lifecycle state does not guess the result; durable outbox plus authenticated foreground recovery determines what happened.

No worker, daemon, foreground service, alarm or background scheduler is required for correctness.

The first Android binding now exists in `apc-android-bridge` plus `android/harness`. A fresh process can report the native gate before `Activity.onStart()`, `onStart()` explicitly enters foreground, and `onStop()` closes the gate. Transport-bearing lost-ack probe commands are deferred until after that foreground transition; the JNI bridge also refuses them if the native gate is still closed.

This is a test boundary, not the final Android application architecture. It currently uses a fixed harness key and a local simulated remote transport. No production GitHub credentials or network permission are part of the harness yet.

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
- deterministic scalar and multi-domain recovery snapshot validation;
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
- remote batch acceptance followed by failed local reconciliation and later authenticated set recovery with no second publish;
- complete multipart publication assembly with reversed part order;
- complete and incomplete publications sharing a fetched range without partial semantic visibility or cursor advancement;
- two complete multipart publications interleaved independently of `PublicationId` byte order;
- identical duplicate multipart part delivery while pending;
- authenticated conflicting duplicate multipart part rejection;
- multi-domain remote observation leaving an unrelated dirty domain unsealed;
- lifecycle-gated multi-domain recovery across encrypted filesystem restart;
- Android-bridge host test reopening a protected recovery record from a real Unix filesystem backend;
- Android-bridge host lost-ack test where remote acceptance is durable, the response is lost, a fresh recovery phase authenticates the accepted state and the remote publish count remains exactly one.

## 16. Still intentionally unresolved

The executable path does **not** freeze or fully solve:

- final compact causal/checkpoint representation and old-baseline membership proofs;
- native `.apc` binary layout;
- complete-continuum portable trusted-state encoding;
- production lifecycle/tombstone semantics;
- production sequence/hierarchy semantics;
- attachment chunk manifests/reachability;
- content-key epoch selection;
- replica signatures and key evolution;
- replay/rollback policy;
- truly irreducible multi-domain atomic publication semantics;
- persistence/reconstruction policy for multipart assembly if a future transport API cannot provide a complete durable-cursor range in one logical fetch;
- automatic semantic re-export/rebase of unresolved stale publication sets across cursor generations;
- durable preservation of authenticated lost-ack proof across an additional crash between catch-up commit and later outbox retirement;
- production GitHub HTTP/GraphQL client and credentials;
- cancellation bridge for an already-running platform request;
- production Android key ownership/Keystore integration;
- actual APK/NDK/device validation of the checked-in Android harness;
- real Android storage behavior under sudden device power loss;
- long-offline transport generation/checkpoint retention.

The current scalar capsules may retain more causal material than the eventual compact format. Correctness remains ahead of compression.

## 17. Immediate next implementation work

The next slices should move from host-validated runtime composition into real handset evidence without changing semantic rules:

1. build the checked-in JNI bridge for `aarch64-linux-android`, assemble/install the minimal APK and run the lifecycle probe on a real device;
2. run the encrypted `stage -> force-stop -> verify` ADB restart probe and preserve the first device result as test evidence;
3. run `lost-ack-stage -> force-stop -> lost-ack-resume` and require authenticated reconciliation with the durable simulated-remote publish count still equal to one;
4. expand the device harness into a bounded kill-point matrix around staging, remote acceptance, catch-up and local reconciliation while preserving the foreground-only rule;
5. only after that device path is stable, bind the production GitHub HTTP/GraphQL client and credential flow behind the existing opaque transport seam;
6. keep Android hardware-key integration local to the platform boundary and separate from the portable content-key/key-evolution design;
7. treat physical power-loss testing as a separate campaign from `am force-stop`; process death alone is not evidence for storage-stack power-loss durability.

A.P.C. transport remains deliberately boring: move opaque authenticated objects and expose enough CAS/change-detection information for trusted semantic/runtime code to decide meaning.
