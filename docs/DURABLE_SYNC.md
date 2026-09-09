# A.P.C. durable synchronization recovery

Status: **transport-independent crash recovery, protected durable record storage, foreground transport gating, multi-domain foreground-resume orchestration, set-based multi-publication resume, bounded conflict follow-up, complete authenticated multipart durable-cursor catch-up, session/reconciliation/rebase transitions, deterministic trusted-state recovery, typed outbound/inbound semantic boundaries and a JNI/Android process-death recovery harness are implemented; actual handset power-loss behavior and final portable/local encodings remain unfrozen**.

This document records the local durability rules between semantic state and an opaque transport such as GitHub. It supplements `SYNC.md`, `SYNC_CAPSULES.md`, `DURABILITY.md`, `CORE_IMPLEMENTATION.md` and `SYNC_IMPLEMENTATION.md`.

## 1. Required failure property

A sudden process/device failure may cause repeated transfer or repeated merge work. It must not cause:

- loss of an already acknowledged durable local edit;
- a transport cursor to become durable ahead of the semantic state produced from it;
- partial multipart state to become semantically visible;
- a locally exposed causal identity to become private again after restart;
- a dirty local edit to acquire a remote causal parent that was not observable when the edit began;
- an unknown transport outcome to be guessed as definite success or failure;
- an opaque publication ID or transport cursor byte ordering to become semantic or retry ordering by accident.

The user-visible invariant remains:

> A synchronization interruption may repeat work, but it must not create a torn logical state.

## 2. Durable cursor/state unit

Transport cursors are bookkeeping only. Git commit identity, repository head, arrival order and timestamps are not A.P.C. causality.

The durable sync invariant is:

```text
trusted semantic/recovery state
+
last fully applied transport cursor
+
pending exact outbound publications
        |
        v
one crash-atomic recovery unit
```

This ordering is forbidden:

```text
receive/merge R1
        |
persist cursor = R1
        |
CRASH
        |
recover semantic state still at R0
```

A restart could then ask for changes after `R1` and silently skip state it never durably retained.

The safe asymmetry is the opposite: semantic state may be newer than an old durable cursor temporarily, because authenticated refetch and idempotent merge can repeat work.

## 3. `DurableSyncRecord`

`apc-sync` currently stores:

```text
DurableSyncRecord
├── trusted_state
├── applied_cursor
└── outbox
    └── PublicationId -> DurableOutboxEntry
        ├── expected_cursor
        └── exact protected wire objects
```

The sync layer treats `trusted_state` as opaque bytes. Semantic/runtime code constructs and validates it. This prevents crash-recovery bookkeeping from becoming a second semantic model.

`TransportCursor` is also opaque bytes with no temporal/causal ordering meaning.

`APCSREC1` is the current deterministic development framing of the recovery record. It is local machinery, not the native `.apc` format.

`apc-runtime::GitHubCursorCodec` provides the concrete reversible `GitHubCommitOid` ↔ `TransportCursor` mapping used by the current GitHub composition without assigning order semantics to commit IDs.

## 4. Local trusted-state encoding

The first real scalar restart path uses `apc-runtime::DevelopmentScalarTrustedStateCodec`.

Its development framing, `APCLREC1`, encodes:

```text
LocalScalarSnapshot
├── WorkingSnapshot
│   ├── ScalarRegister
│   └── optional WorkingEpoch
│       ├── WorkingEpochId
│       ├── current durable local value
│       └── observed frontier
└── FinalizationSnapshot
    ├── locally-owned RevisionIds
    ├── FinalizedStatements
    ├── exposed local RevisionIds
    └── directly handed-off local RevisionIds
```

Both encode and decode validate through `LocalScalarDomain::restore()`. Invalid working frontiers or inconsistent finalization/exposure bookkeeping fail closed.

The complete current runtime recovery path uses `DevelopmentMultiScalarTrustedStateCodec` / `APCLSET1` to hold several independent scalar merge domains in one physical recovery image. One physical crash-atomic blob does not make those domains one semantic transaction and does not create cross-domain causality.

The distinction is important:

```text
APCLREC1   local scalar trusted-state recovery framing
APCLSET1   multi-domain local recovery container
APCSREC1   durable sync record framing
APCSYNC1   clear scalar sync-projection framing
APCSPRT1   protected sync-part wire framing
.apc       final native portable format — not frozen
```

These development encodings establish executable boundaries without conflating them into one future format.

## 5. Safe outbound order

The safe outbound order is:

```text
local durable work
        |
seal/finalize semantic statement when required
        |
select causal revision(s) for publication
        |
validate local dependency closure is finalized
        |
build exact semantic dependency projection
        |
AEAD protect + wire encode once
        |
record handoff/exposure on candidate semantic state
        |
persist {
    exposed trusted state,
    expected cursor,
    exact protected wire bytes
}
        |
LOCAL DURABILITY BARRIER
        |
replace live semantic state
        |
network I/O may begin
```

The key rule is that semantic exposure and exact retry material become durable before network handoff can make the identity externally observable.

`prepare_scalar_handoff()` constructs the protected scalar publication from the selected causal dependency closure. Application glue does not supply an unrelated arbitrary set of “exposed IDs” plus arbitrary ciphertext bytes.

The builder first performs the semantic `handoff()` rule on a clone, proving every locally-owned revision in the transitive dependency closure is finalized. It then exports only the selected causal closure, not unrelated concurrent branches residing in the same register, and protects that projection through the real sync AEAD/wire path.

The resulting `PreparedScalarHandoff` couples the direct selected `RevisionId`s and exact protected wire bytes.

The multi-domain path applies the same rule through `prepare_recovery_handoff()` and `stage_prepared_recovery_handoff()` so exposure bookkeeping and exact retry bytes become durable together with the complete local recovery container.

If handoff validation, trusted-state encoding or persistence fails, the live semantic state and durable sync record remain unchanged.

If the process dies after persistence but before the final RAM assignment, restart recovers the already-exposed semantic snapshot from the same durable unit that contains the exact outbox bytes.

## 6. Exact-byte retry, unknown acknowledgement and authenticated rediscovery

A prepared publication stores complete already-protected wire objects verbatim.

Because XChaCha20-Poly1305 uses fresh random nonces, protecting the same clear projection again would normally produce different ciphertext and therefore a different content-addressed transport object.

Safe retry is therefore:

```text
protect once
        |
persist exact bytes
        |
try transport
        |
process/network dies
        |
restart
        |
retry those exact bytes
```

A remote mutation may succeed while its response is lost:

```text
publish(expected = R0)
        |
remote accepts -> R1
        |
response lost
```

The local outbox remains durable at `R0`. Retry against `R0` may return conflict; fetch from the durable cursor then rediscovers the accepted protected object. Reconciliation decides the outcome from observable state rather than from an ACK guess.

Catch-up reports `RevisionId`s proven to have arrived in authenticated remote state during the exact catch-up pass, grouped by semantic merge domain in the multi-domain path. The runtime may retire a stale pending publication without another transport mutation only when every carried causal revision in every carried domain is present in that fresh authenticated evidence.

Merely finding the same `RevisionId` in already-local state is not enough. The proof is transport-observation evidence from the exact authenticated catch-up.

If the process crashes after the catch-up cursor/state commit but before the later outbox-retirement durability barrier, that ephemeral proof is lost. Correctness still holds: the outbox remains durable and later follows the ordinary stale/rebase path. The current implementation deliberately does not invent persistent transport-observation evidence merely to preserve an optimization.

## 7. Overlapping publications and set-based retry

Multiple pending publications may coexist. Their `PublicationId`s are opaque identities, not clocks, sequence numbers or retry priority.

The runtime rule is:

> Never choose “the first pending publication” merely because a container sorts `PublicationId` bytes.

### 7.1 Equal-cursor transport batches

`publish_staged_batch()` accepts a non-empty set of staged `PublicationId`s and verifies before network I/O that every member exists and every member targets the same `expected_cursor`. It then sends the exact already-protected objects from all members in one transport mutation.

```text
P1 @ R0 ─┐
P2 @ R0 ─┼─> one exact-byte batch publish(expected = R0)
P3 @ R0 ─┘
```

The IDs are canonically ordered only to make transport input deterministic. That byte order has no temporal, causal or priority meaning.

A mixed cursor class is rejected before transport I/O:

```text
P1 @ R0
P2 @ R1
   |
   X  no implicit ordering / no guessed winner
```

A successful batch transport call does not itself erase any outbox entry. `commit_reconciled_outbox_batch()` durably retires a selected publication set with one cursor/trusted-state persistence barrier. Its transition is clone-persist-swap: unknown membership, cursor encoding failure or durability failure leaves the caller's complete old record unchanged. Unselected outbox entries are preserved.

The batch is transport coalescing, not a semantic multi-domain transaction.

### 7.2 Set-based runtime resume

`resume_scalar_recovery_outbox_set()` lifts those primitives into the complete multi-domain scalar recovery runtime without inventing ID order.

After catch-up it classifies the initially pending outbox strictly by equality with the resulting durable cursor:

```text
initial pending set
        |
catch up from durable cursor
        |
new durable applied_cursor = R
        |
├── entry.expected_cursor == R  -> current set
└── entry.expected_cursor != R  -> stale set
```

No cursor bytes are numerically or lexicographically compared.

For the stale set, authenticated catch-up evidence is checked publication-by-publication and domain-by-domain. Every stale publication whose full carried causal closure is proven present in authenticated remote state may be retired as part of one durable reconciliation set.

If any stale publication remains unresolved, the cycle returns:

```text
NeedsRebase { unresolved stale PublicationIds }
```

and **does not publish the current-cursor set**. This is deliberately conservative. It avoids smuggling a cross-generation scheduling rule into opaque IDs or transport cursor ordering.

If no stale publication remains, all current-cursor entries are published together as one exact-byte batch. Success is followed by one durable batch retirement; conflict leaves the whole set pending.

### 7.3 Stale replacement

If a historical publication is still semantically needed after reconciliation, it is never mutated in place:

```text
P @ R0
        |
R0 -> R1
        |
semantic reconciliation
        |
still needed
        |
re-export/re-protect
fresh PublicationId
fresh exact bytes @ R1
        |
commit_rebased_outbox()
```

`commit_rebased_outbox()` performs stale retirement plus fresh replacement as one durable transition. A persistence failure preserves the complete old record.

The sync layer deliberately does not decide whether stale semantic content is redundant or still needs publication.

## 8. Safe inbound order

Inbound state follows the opposite direction:

```text
fetch protected object(s) after durable cursor
        |
wire validation
        |
AEAD authentication
        |
complete publication assembly
        |
decode + semantic validation
        |
semantic observation/merge on candidate state
        |
if same domain is dirty:
    seal local working epoch first using its captured old frontier
        |
encode merged trusted state
        |
commit_received(merged trusted state, new cursor)
        |
LOCAL DURABILITY BARRIER
        |
replace live semantic state
```

`commit_received()` couples opaque trusted bytes and the new cursor in one clone-persist-swap transition.

The multi-domain catch-up supplies pre-observation local `RevisionId`s keyed by merge domain. Remote state touching one domain may seal dirty local work only in that exact domain; an unrelated dirty domain remains unsealed.

This directly protects the working-state invariant established by the research model. A local epoch that began while only frontier `F0` was visible cannot later claim newly received revision `R` as its causal parent merely because synchronization happened before publication.

On persistence failure the live recovery state remains unchanged and the old cursor remains durable.

## 9. Multipart inbound visibility

`MultipartInbox` authenticates parts and exposes a projection only after every declared part of that publication has arrived. The complete fetched-range decoder lifts that primitive to one fetched transport range.

The durable-cursor rule is:

```text
fetch R0 -> R1
        |
decode every protected wire object
        |
authenticate every part
        |
assemble every publication in range
        |
any publication incomplete?
      /             \
    yes             no
     |               |
 fail closed       merge all complete
     |             authenticated state
 state unchanged      |
 cursor stays R0      |
                    semantic observation
                        |
                    one durability barrier
                        |
                    cursor = R1
```

The multipart inbox is intentionally ephemeral at this stage. An incomplete fetched range is not persisted as half-assembled semantic state; the old durable cursor remains authoritative so the same range can be fetched again later. A complete publication that happens to share a range with an incomplete publication is discarded with the failed candidate rather than becoming visible early.

Parts may arrive in any order. Complete multipart publications may be interleaved. All fully assembled state is merged before semantic observation, so transport completion order cannot become causal order and a dirty local working epoch is sealed at most once per touched domain for the range.

Identical duplicate parts while a publication is still pending are harmless. An authenticated conflicting duplicate for the same publication/part slot fails closed with `MultipartPartCollision`. The current exact-byte retry contract still requires a publication identity to retain its original protected bytes; re-protection or semantic replacement uses a fresh `PublicationId`.

## 10. Protected durable record store

`ProtectedSyncRecordStore<B>` maps the recovery unit into opaque authenticated bytes before calling a `DurabilityBackend<Vec<u8>>`:

```text
DurableSyncRecord
        |
APCSREC1
        |
XChaCha20-Poly1305
        |
opaque bytes
        |
DurabilityBackend<Vec<u8>>
        |
commit_durable()
```

The caller supplies a non-empty local context combined with an internal domain separator. Wrong key/context or modified ciphertext fails authentication.

`apc-storage-fs` owns only crash-safe opaque-byte persistence. Semantic recovery encoding belongs in runtime.

## 11. Development Unix durability backend

`UnixFsDurabilityBackend` uses immutable candidate files plus an atomically replaced committed-root manifest. Candidate names are local physical bookkeeping only and have no semantic order.

The durable protocol requires:

1. write candidate bytes;
2. sync candidate file;
3. sync candidate directory entry;
4. write/sync replacement root manifest;
5. rename root manifest;
6. sync containing directory.

A durable-but-unpublished candidate is ignored after reopen. A corrupted root or candidate framing fails closed.

This establishes the Rust/Unix durability contract. It is not yet proof of Android storage-stack behavior under sudden power loss.

## 12. Restart and recovery evidence

### 12.1 Outbound vertical restart

The runtime integration test executes:

```text
LocalScalarDomain
→ finalized local RevisionId
→ exact selected dependency closure
→ ScalarSyncProjection
→ XChaCha20-Poly1305 sync protection
→ APCSPRT1 wire
→ exposed trusted state + exact outbox
→ APCSREC1
→ XChaCha20-Poly1305 local recovery protection
→ UnixFsDurabilityBackend
→ close/reopen
→ decrypt record
→ decode trusted state
→ restore semantic state
→ recover exact outbox wire
→ decode + authenticate publication
```

After reopen the test verifies that finalized/exposed/handoff bookkeeping, working state, applied cursor and exact protected retry bytes survived together.

### 12.2 Inbound dirty-observation restart

Two replicas start from the same scalar base:

```text
receiver: dirty local WorkingEpoch begun at base frontier
sender:   finalized remote RevisionId 900 from the same base
```

The sender's revision is protected through the real outbound builder. The receiver authenticates it and observes it with reserved local `RevisionId 200` plus a newer transport head. The candidate transition seals the dirty local epoch first using only the old base frontier, then merges remote revision `900`.

After close/reopen the frontier remains:

```text
{ local 200, remote 900 }
```

with neither revision an ancestor of the other, and the newer transport cursor survives with the exact semantic state.

### 12.3 Lost ACK and second durability barrier

The restart path stages an exposed publication, simulates remote acceptance followed by process death before acknowledgement handling, reopens the protected Unix store, catches up from the durable cursor, rediscovers the accepted publication through authenticated state, retires the outbox without a second publish, then reopens again and verifies the reconciled result.

A separate failure test forces the later outbox-retirement durability barrier to fail. The first caught-up `{trusted state, new cursor, pending outbox}` record remains authoritative; the outbox cannot disappear only from RAM.

### 12.4 Accepted batch + failed local reconciliation

The set-based recovery test covers the analogous multi-publication boundary. After remote acceptance followed by failed local reconciliation, the local recovery record remains the old durable cursor plus every exact pending publication. The next resume authenticates the accepted objects, advances state/cursor durably and retires the proven set without a second publish.

### 12.5 Multipart range atomicity

The multipart catch-up tests cover both sides of the range boundary. Reversed part order and interleaved complete publications converge before one semantic observation boundary. A complete publication beside an incomplete one causes no semantic mutation or cursor advance. Identical duplicate pending parts are harmless; authenticated conflicting duplicates fail closed.

### 12.6 Android/JNI development recovery probes

`apc-android-bridge` now exposes two restartable ADB scenarios on top of the same Rust durability/runtime machinery.

The basic `stage -> force-stop -> verify` path writes a real exposed multi-domain trusted state and exact protected outbox through `ProtectedSyncRecordStore<UnixFsDurabilityBackend>` under Android app-private `filesDir`. `verify` opens that store from a fresh process, authenticates/decrypts it, decodes the trusted state and checks the cursor, handoff/exposure markers and exact pending wire object.

The stronger `lost-ack-stage -> force-stop -> lost-ack-resume` path uses a harness-only `OpaqueTransport` whose remote state is itself durably persisted. `lost-ack-stage` lets the remote accept the exact protected object, durably increments a publish counter, and then deliberately loses the response. The local store remains at the old cursor with the outbox pending. After process death, `lost-ack-resume` refetches the already accepted protected object, authenticates it, reconciles the stale outbox and requires the durable remote publish counter to remain exactly `1`.

The host Rust suite runs both probe sequences against real Unix filesystem backends and is green. That proves the probe logic and host filesystem path only. A checked-in APK/JNI harness makes the same routines available for ADB, but a physical-device pass must be collected separately before Android process-death behavior is claimed as observed evidence.

The probe's fixed symmetric key, local simulated remote framing and storage locations are test fixtures only. They are not production Android security or portable format decisions.

## 13. Foreground-only transport gate and bounded resume

`ForegroundSyncLifecycle` and `ForegroundTransport<T>` start closed. Platform code must explicitly enter foreground before new `head`, `fetch_since` or `publish` calls reach transport.

Entering background blocks future transport I/O without touching semantic state, cursor or outbox. An already-running mutation may already have succeeded remotely; its outcome remains unknown and is recovered from durable facts.

`ForegroundRecoveryRuntime<T>` composes the complete multi-domain recovery/outbox cycle above that gate. The first pass may perform one equal-cursor set publication. A CAS conflict permits exactly one further catch-up/reclassification pass; there is deliberately no unbounded conflict loop.

The Android Activity calls the native foreground transition in `onStart()` and the background transition in `onStop()`. Transport-bearing lost-ack probe commands are deferred until after `onStart()`, and the JNI boundary independently refuses those commands while the native gate is closed.

No daemon, Android foreground service, WorkManager job, alarm or background scheduler is required for correctness.

## 14. Current failure matrix

The deterministic Rust suite now covers, among other cases:

1. outbound staging persistence failure;
2. network failure after durable staging;
3. remote acceptance followed by response loss;
4. inbound trusted-state/cursor persistence failure;
5. reconciliation persistence failure;
6. multiple simultaneous outbox entries;
7. stale-entry replacement with a fresh `PublicationId`;
8. failed stale replacement preserving old state;
9. foreground/background transport blocking;
10. backgrounding during a remotely accepted in-flight mutation;
11. outbound semantic handoff + durable exposure/outbox atomicity;
12. unfinalized local dependency rejected before outbound protection;
13. deterministic scalar and multi-domain recovery encode/decode validation;
14. real encrypted outbound filesystem restart;
15. authenticated inbound dirty observation preserving true concurrency;
16. failed inbound semantic persistence leaving live dirty state and old cursor unchanged;
17. real encrypted inbound filesystem restart preserving merged semantics and cursor;
18. GitHub cursor round-trip with no ordering semantics;
19. lost-ACK authenticated rediscovery with no second publish;
20. failed lost-ACK reconciliation durability barrier preserving pending outbox;
21. bounded post-conflict catch-up ending at explicit `NeedsRebase`;
22. same-cursor publication set sent in one exact-byte transport mutation;
23. mixed-cursor publication set rejected before transport I/O;
24. batch reconciliation durability failure preserving every selected entry;
25. set-based runtime publishing several same-cursor pending entries without ID priority;
26. unresolved stale set blocking a current set without implicit cross-generation ordering;
27. authenticated catch-up reconciling an entire stale lost-ACK set without republish;
28. bounded batch conflict performing exactly one follow-up catch-up;
29. remote batch acceptance followed by failed local reconciliation and later authenticated set recovery with no second publish;
30. complete multipart catch-up with reversed part order and one semantic observation boundary;
31. a complete publication beside an incomplete multipart publication causing no semantic mutation or cursor advance;
32. interleaved complete multipart publications remaining independent of `PublicationId` ordering;
33. identical duplicate multipart part delivery while pending remaining harmless;
34. authenticated conflicting duplicate multipart part delivery failing closed;
35. multi-domain remote observation leaving unrelated dirty domains unsealed;
36. lifecycle-gated multi-domain recovery across encrypted filesystem restart;
37. Android-bridge host encrypted recovery probe surviving a real Unix filesystem reopen;
38. Android-bridge host lost-ack probe durably accepting remotely, losing the response, reopening/reconciling and proving no second publish via a durable publish counter.

## 15. What remains unproved/unfrozen

The current implementation does not yet prove or freeze:

- actual APK cross-compilation/installation/device execution evidence for the checked-in harness;
- actual Android handset power-loss behavior;
- production Android content-key ownership/Keystore integration;
- final `.apc` storage layout;
- final complete-continuum local recovery encoding;
- compact long-lived causal membership/checkpoint representation;
- persistence or reconstruction policy for multipart assembly if a future transport API cannot return a complete durable-cursor range in one logical fetch;
- production lifecycle/sequence/hierarchy persistence semantics;
- attachment chunk reachability/recovery;
- replica authentication/key evolution;
- replay/rollback policy;
- irreducible multi-domain atomic publication semantics;
- production GitHub HTTP/GraphQL client and credentials;
- cancellation of an already-running platform HTTP mutation;
- automatic semantic re-export/rebase policy for unresolved stale publication sets across cursor generations;
- persistent transport-observation evidence across a crash between catch-up commit and later outbox reconciliation;
- very-old transport generation/rebootstrap policy.

The existing `APCLREC1`, `APCLSET1`, `APCSREC1`, `APCSYNC1` and `APCSPRT1` encodings remain development contracts until the actual format freeze.

## 16. Android validation path

The Android binding now exists; the next evidence step is to run it rather than redesign the state machine:

```text
Rust unit/property tests                       ✓
        |
Unix filesystem restart tests                 ✓
        |
JNI + minimal Android harness checked in      ✓
        |
NDK build + APK install on real handset       pending
        |
stage -> am force-stop -> verify              pending
        |
lost-ack-stage -> force-stop -> resume        pending
        |
kill-point matrix during sync transitions     pending
        |
controlled device power-cycle tests           later, separate campaign
```

`adb shell am force-stop` is process-death evidence, not sudden power-loss evidence. It must not be used as a substitute for a physical power-loss campaign.

The oracle must inspect semantic state, working epoch, finalization/exposure bookkeeping, cursor and outbox — not merely whether the app opens. The lost-ack harness additionally requires that the simulated remote durable publish count remain exactly one after recovery.

## 17. Immediate next implementation work

The next durability-oriented slices should:

1. build the JNI bridge for `aarch64-linux-android`, assemble/install the minimal APK and run the lifecycle probe on a real device;
2. run the basic encrypted recovery `stage -> force-stop -> verify` path and preserve the result;
3. run the lost-ack `stage -> force-stop -> resume` path and require authenticated reconciliation with no second publish;
4. expand the Android harness into a bounded kill-point matrix around local staging, remote acceptance, authenticated catch-up and local reconciliation;
5. once the device process-death path is stable, bind the real GitHub HTTP/GraphQL transport and its credentials behind the existing opaque transport seam;
6. integrate Android hardware-backed key wrapping as local platform policy without making it portable core semantics;
7. keep real device power-loss testing separate from process-death testing and do not claim the stronger durability guarantee until that campaign exists.

The responsibility split remains simple: semantic state decides meaning, crypto decides confidentiality/authenticity, durability decides what survives restart, lifecycle decides whether new transport I/O may begin, runtime composes those boundaries, and transport only moves opaque authenticated objects.
