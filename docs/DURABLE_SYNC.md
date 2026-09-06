# A.P.C. durable synchronization recovery

Status: **transport-independent crash recovery, protected durable record storage, foreground transport gating, session/reconciliation/rebase transitions, deterministic scalar trusted-state recovery, typed outbound semantic exposure and typed inbound observation are implemented; Android power-loss behavior and final portable/local encodings remain unfrozen**.

This document records the local durability rules between semantic state and an opaque transport such as GitHub. It supplements `SYNC.md`, `SYNC_CAPSULES.md`, `DURABILITY.md`, `CORE_IMPLEMENTATION.md` and `SYNC_IMPLEMENTATION.md`.

## 1. Required failure property

A sudden process/device failure may cause repeated transfer or repeated merge work. It must not cause:

- loss of an already acknowledged durable local edit;
- a transport cursor to become durable ahead of the semantic state produced from it;
- partial multipart state to become semantically visible;
- a locally exposed causal identity to become private again after restart;
- a dirty local edit to acquire a remote causal parent that was not observable when the edit began;
- an unknown transport outcome to be guessed as definite success or failure.

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

The first real scalar restart path now uses `apc-runtime::DevelopmentScalarTrustedStateCodec` instead of an in-memory test vault.

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

The distinction is important:

```text
APCLREC1   local scalar trusted-state recovery framing
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

`prepare_scalar_handoff()` now constructs the protected scalar publication itself from the selected causal dependency closure. Application glue no longer supplies an unrelated arbitrary set of “exposed IDs” plus arbitrary ciphertext bytes.

The builder first performs the semantic `handoff()` rule on a clone, proving every locally-owned revision in the transitive dependency closure is finalized. It then exports only the selected causal closure, not unrelated concurrent branches residing in the same register, and protects that projection through the real sync AEAD/wire path.

The resulting `PreparedScalarHandoff` couples the direct selected `RevisionId`s and exact protected wire bytes.

`stage_prepared_scalar_handoff()` then:

```text
clone live LocalScalarDomain
        |
candidate.handoff(prepared RevisionIds)
        |
encode exposed APCLREC1 snapshot
        |
stage_outbound(exposed trusted state, exact prepared bytes)
        |
commit protected DurableSyncRecord
        |
LOCAL DURABILITY BARRIER
        |
replace live domain
```

If handoff validation, trusted-state encoding or persistence fails, the live semantic domain and durable sync record remain unchanged.

If the process dies after persistence but before the final RAM assignment, restart recovers the already-exposed semantic snapshot from the same durable unit that contains the exact outbox bytes.

## 6. Exact-byte retry and unknown acknowledgement

A prepared publication stores complete already-protected wire objects verbatim.

Because XChaCha20-Poly1305 uses fresh random nonces, protecting the same clear projection again would normally produce different ciphertext and therefore a different content-addressed GitHub object path.

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

`publish_staged()` never regenerates ciphertext and never removes an outbox entry merely because a network call returned success.

A remote mutation may succeed while its response is lost:

```text
publish(expected = R0)
        |
remote accepts -> R1
        |
response lost
```

The local outbox remains durable at `R0`. Retry against `R0` can return conflict; fetch from the durable cursor then rediscovers the accepted protected object. Reconciliation decides the outcome from observable state rather than from an ACK guess.

## 7. Overlapping publications and stale outbox

Multiple pending publications may coexist.

If `P1` and `P2` were staged against `R0` and reconciling `P1` advances the durable cursor to `R1`, `P2` remains exactly the historical protected publication prepared at `R0`.

It must not be mutated in place and its `PublicationId` must not be reused for different bytes.

```text
P2 @ R0
        |
R0 -> R1
        |
semantic reconciliation
        |
P2 redundant -> retire appropriately

or

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

`commit_received()` already couples opaque trusted bytes and the new cursor in one clone-persist-swap transition.

`commit_received_scalar_domain()` now adds the semantic side of that boundary. `ReceivedScalarState` bundles:

```text
authenticated remote ScalarRegister
pre-observation RevisionId if dirty local work must be sealed
new transport head
```

The function clones the live domain, invokes `observe_remote()` on the candidate, encodes the candidate trusted snapshot, commits that snapshot and new cursor together, and only then replaces the live domain.

This directly protects the working-state invariant established by the research model. A local epoch that began while only frontier `F0` was visible cannot later claim newly received revision `R` as its causal parent merely because synchronization happened before publication.

On persistence failure the live domain remains dirty, the pre-observation local revision is absent from live causal state, and the old cursor remains durable.

## 9. Multipart inbound visibility

`MultipartInbox` authenticates and accumulates parts internally. No semantic projection is returned until the entire declared publication is present.

The first runtime helper `decode_single_scalar_domain_object()` intentionally accepts only a complete single-part publication with exactly one expected `DomainKey`. Multipart runtime orchestration above `MultipartInbox` remains future work; incomplete parts must never be fed piecemeal to `commit_received_scalar_domain()`.

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

`apc-storage-fs` therefore owns only crash-safe opaque-byte persistence. A duplicate semantic snapshot codec that had briefly existed in that crate was removed; semantic recovery encoding belongs in runtime.

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

## 12. Real outbound restart test

The runtime integration test executes the complete first outbound vertical slice:

```text
LocalScalarDomain
→ finalized local RevisionId
→ exact selected dependency closure
→ ScalarSyncProjection
→ XChaCha20-Poly1305 sync protection
→ APCSPRT1 wire
→ PreparedScalarHandoff
→ exposed APCLREC1 trusted state + exact outbox
→ APCSREC1
→ XChaCha20-Poly1305 local recovery protection
→ UnixFsDurabilityBackend
→ close/reopen
→ decrypt record
→ decode APCLREC1
→ restore LocalScalarDomain
→ recover exact outbox wire
→ decode + authenticate publication
```

After reopen the test verifies:

- finalized/exposed/handed-off semantic identity survived;
- pending local working state survived independently;
- the applied transport cursor survived;
- the exact protected publication bytes survived;
- the protected publication decodes to exactly the expected causal dependency closure;
- raw committed filesystem bytes do not contain known semantic plaintext or the clear `APCLREC1` marker.

## 13. Real inbound restart test

A second vertical test now proves the corresponding incoming dirty-observation path.

Two replicas start from the same scalar base:

```text
receiver: dirty local WorkingEpoch begun at base frontier
sender:   finalized remote RevisionId 900 from the same base
```

The sender's revision is exported through the real protected publication builder and authenticated on the receiver. The receiver then observes it with a reserved pre-observation local `RevisionId 200` and a newer GitHub transport head.

The candidate transition seals the dirty local epoch first using only the base frontier, then merges remote revision `900`. The resulting frontier is:

```text
{ local 200, remote 900 }
```

and both ancestry tests remain false:

```text
200 !< 900
900 !< 200
```

The merged APCLREC1 state and new cursor are protected and committed to the real Unix backend. After close/reopen the exact semantic domain, true concurrency and new GitHub cursor are recovered together. Known local and remote plaintext strings are absent from the raw committed bytes.

This is the first executable proof that the earlier “receipt != semantic observation” rule survives the real crypto/durability restart path rather than only an in-memory reference model.

## 14. Foreground-only transport gate

`ForegroundSyncLifecycle` and `ForegroundTransport<T>` start closed. Platform code must enter foreground before new `head`, `fetch_since` or `publish` calls reach transport.

Entering background blocks future transport I/O without touching semantic state, cursor or outbox.

An already-running mutation may have succeeded remotely before cancellation/backgrounding. Its outcome remains unknown and is recovered through durable outbox + reconciliation; lifecycle state never rewrites history.

The suite exercises remote acceptance, lost response, background blocking, foreground resume, stale-head conflict and refetch.

No daemon, Android foreground service, WorkManager job, alarm or background scheduler is required for correctness.

## 15. Current failure matrix

The deterministic Rust tests now cover at least:

1. outbound staging persistence failure;
2. network failure after durable staging;
3. remote acceptance followed by response loss;
4. inbound trusted-state/cursor persistence failure;
5. reconciliation persistence failure;
6. overlapping pending publications;
7. stale outbox replacement with a fresh `PublicationId`;
8. failed stale replacement preserving old state;
9. foreground/background transport blocking;
10. backgrounding during a remotely accepted in-flight mutation;
11. outbound semantic handoff + durable exposure/outbox atomicity;
12. unfinalized local dependency rejected before outbound protection;
13. deterministic local scalar recovery encode/decode validation;
14. real encrypted outbound filesystem restart;
15. authenticated inbound dirty observation preserving true concurrency;
16. failed inbound semantic persistence leaving the live dirty state and old cursor unchanged;
17. real encrypted inbound filesystem restart preserving merged semantics and cursor;
18. GitHub cursor round-trip with no ordering semantics.

## 16. What remains unproved/unfrozen

The current implementation does not yet prove:

- actual Android handset power-loss behavior;
- final `.apc` storage layout;
- final complete-continuum local recovery encoding;
- compact long-lived causal membership/checkpoint representation;
- multipart runtime receive orchestration;
- production lifecycle/sequence/hierarchy persistence semantics;
- attachment chunk reachability/recovery;
- replica authentication/key evolution;
- replay/rollback policy;
- irreducible multi-domain atomic publication semantics;
- production GitHub HTTP/GraphQL client and credentials;
- cancellation of an already-running platform HTTP mutation;
- foreground-resume orchestration;
- very-old transport generation/rebootstrap policy.

The existing `APCLREC1`, `APCSREC1`, `APCSYNC1` and `APCSPRT1` encodings must remain development contracts until the actual format freeze.

## 17. Android validation path

Once the first Android binding exists, the same state machine should be exercised through ADB:

```text
Rust unit/property tests
        |
Unix filesystem restart tests
        |
subprocess SIGKILL tests
        |
Android process kill
        |
kill during outbound transfer
        |
kill after remote acceptance / before ACK handling
        |
kill after authenticated inbound observation / before durable commit
        |
relaunch + invariant verification
        |
controlled device power-cycle tests
```

The oracle must inspect semantic state, working epoch, finalization/exposure bookkeeping, cursor and outbox — not merely whether the app opens.

## 18. Immediate next implementation work

The next durability-oriented slices should:

1. build foreground-resume runtime orchestration that runs durable-cursor catch-up and pending-outbox recovery through the same tested transitions;
2. add fetch-failure and repeated conflict/rebase-chain tests with several pending publications;
3. wire complete authenticated multipart assembly into the runtime receive boundary without partial visibility;
4. introduce a complete local trusted-state container abstraction for multiple independent domains without smuggling in cross-domain transaction semantics;
5. later reproduce the current outbound/inbound restart matrix through Android process-kill and power-cycle tests.

The responsibility split remains simple: semantic state decides meaning, crypto decides confidentiality/authenticity, durability decides what survives restart, lifecycle decides whether new transport I/O may begin, runtime composes those boundaries, and transport only moves opaque authenticated objects.
