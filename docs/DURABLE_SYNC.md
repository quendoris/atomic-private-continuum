# A.P.C. durable synchronization recovery

Status: **transport-independent crash recovery, protected durable record storage, foreground transport gating, scalar foreground-resume orchestration, set-based multi-publication resume, bounded conflict follow-up, session/reconciliation/rebase transitions, deterministic scalar trusted-state recovery and typed outbound/inbound semantic boundaries are implemented; Android power-loss behavior and final portable/local encodings remain unfrozen**.

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

`prepare_scalar_handoff()` constructs the protected scalar publication from the selected causal dependency closure. Application glue does not supply an unrelated arbitrary set of “exposed IDs” plus arbitrary ciphertext bytes.

The builder first performs the semantic `handoff()` rule on a clone, proving every locally-owned revision in the transitive dependency closure is finalized. It then exports only the selected causal closure, not unrelated concurrent branches residing in the same register, and protects that projection through the real sync AEAD/wire path.

The resulting `PreparedScalarHandoff` couples the direct selected `RevisionId`s and exact protected wire bytes.

`stage_prepared_scalar_handoff()` then performs:

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

`catch_up_single_scalar_domain()` reports `RevisionId`s proven to have arrived in authenticated remote scalar state during the exact catch-up pass. The runtime may retire a stale pending scalar publication without another transport mutation only when the complete causal revision set carried by that publication is a subset of those newly authenticated remote IDs.

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

The batch is transport coalescing, not a semantic multi-domain transaction. It does not assert that all carried merge-domain changes form one indivisible application operation.

### 7.2 Set-based runtime resume

`resume_scalar_outbox_set()` lifts those primitives into the scalar runtime without inventing ID order.

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

For the stale set, authenticated catch-up evidence is checked publication-by-publication. Every stale single-part scalar publication whose full causal closure is proven present in the authenticated remote state is retired as part of one durable reconciliation set.

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

`commit_received_scalar_domain()` adds the semantic side of that boundary. `ReceivedScalarState` bundles:

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

The current runtime catch-up still consumes complete single-part scalar objects through `decode_single_scalar_domain_object()`. Wiring complete authenticated multipart assembly into the durable-cursor catch-up boundary is still open. Until that is implemented, incomplete parts must never be fed piecemeal to `commit_received_scalar_domain()`, and the cursor must never be advanced past an incomplete publication merely because some other publication in the fetched range is complete.

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

After reopen the test verifies that finalized/exposed/handoff bookkeeping, pending local working state, applied cursor and exact protected retry bytes survived together. Raw committed filesystem bytes are checked not to contain selected known plaintext strings or the clear `APCLREC1` marker.

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

The single-publication restart path stages an exposed publication, simulates remote acceptance followed by process death before acknowledgement handling, reopens the protected Unix store, catches up from the durable cursor, rediscovers the accepted publication through authenticated state, retires the outbox without a second publish, then reopens again and verifies the reconciled result.

A separate failure test forces the later outbox-retirement durability barrier to fail. The first caught-up `{trusted state, new cursor, pending outbox}` record remains authoritative; the outbox cannot disappear only from RAM.

### 12.4 Accepted batch + failed local reconciliation

The set-based recovery test covers the analogous multi-publication boundary:

```text
P1 @ R1 ─┐
P2 @ R1 ─┴─> one batch publish
              |
              remote accepts -> R2
              |
              local batch-retirement persist fails
```

After that failure, the local recovery record is still exactly the old durable `{cursor R1, P1, P2}` state although transport is already at `R2`.

The next resume fetches the accepted protected objects from `R1 -> R2`, authenticates them, proves the complete causal closures for both pending publications, advances semantic state/cursor durably and retires both publications as one observed set. The transport publish call count remains one: recovery does not send either publication again.

## 13. Foreground-only transport gate and bounded resume

`ForegroundSyncLifecycle` and `ForegroundTransport<T>` start closed. Platform code must explicitly enter foreground before new `head`, `fetch_since` or `publish` calls reach transport.

Entering background blocks future transport I/O without touching semantic state, cursor or outbox. An already-running mutation may already have succeeded remotely; its outcome remains unknown and is recovered from durable facts.

The narrow single-publication orchestration remains available as `resume_single_scalar_domain()` and `resume_single_scalar_domain_bounded()`.

The multi-publication path is now:

```text
resume_scalar_outbox_set()
        |
catch-up
        |
reconcile stale entries proven observed
        |
unresolved stale set? -> NeedsRebase, stop
        |
current equal-cursor set
        |
one exact-byte batch publish
        |
durable batch retirement
```

`resume_scalar_outbox_set_bounded()` adds exactly one follow-up set-based resume pass after a batch CAS conflict. There is deliberately no `while conflict` loop. One foreground callback can therefore do at most one initial batch publication attempt plus one catch-up/reclassification pass.

A conflict test stages two entries in reverse `PublicationId` order, publishes them as one set, injects a remote head advance, performs exactly one follow-up catch-up, then stops with both entries in `NeedsRebase`. It performs two fetches and one publish; no ID becomes hidden retry priority.

No daemon, Android foreground service, WorkManager job, alarm or background scheduler is required for correctness.

## 14. Current failure matrix

The deterministic Rust suite now covers, among other cases:

1. outbound staging persistence failure;
2. network failure after durable staging;
3. remote acceptance followed by response loss;
4. inbound trusted-state/cursor persistence failure;
5. single-publication reconciliation persistence failure;
6. multiple simultaneous outbox entries at the recovery layer;
7. stale-entry replacement with a fresh `PublicationId`;
8. failed stale replacement preserving old state;
9. foreground/background transport blocking;
10. backgrounding during a remotely accepted in-flight mutation;
11. outbound semantic handoff + durable exposure/outbox atomicity;
12. unfinalized local dependency rejected before outbound protection;
13. deterministic local scalar recovery encode/decode validation;
14. real encrypted outbound filesystem restart;
15. authenticated inbound dirty observation preserving true concurrency;
16. failed inbound semantic persistence leaving live dirty state and old cursor unchanged;
17. real encrypted inbound filesystem restart preserving merged semantics and cursor;
18. GitHub cursor round-trip with no ordering semantics;
19. lost-ACK authenticated rediscovery with no second publish;
20. failed lost-ACK reconciliation durability barrier preserving pending outbox;
21. one bounded single-publication post-conflict catch-up ending at explicit `NeedsRebase`;
22. same-cursor publication set sent in one exact-byte transport mutation;
23. mixed-cursor publication set rejected before transport I/O;
24. batch reconciliation durability failure preserving every selected entry;
25. set-based runtime publishing several same-cursor pending entries in one mutation without ID priority;
26. unresolved stale set blocking a newer/current set without implicit cross-generation ordering;
27. authenticated catch-up reconciling an entire stale lost-ACK set without republish;
28. bounded batch conflict performing exactly one follow-up catch-up and stopping at set `NeedsRebase`;
29. remote acceptance of a batch followed by local reconciliation failure and later authenticated set recovery with no second publish.

## 15. What remains unproved/unfrozen

The current implementation does not yet prove or freeze:

- actual Android handset power-loss behavior;
- final `.apc` storage layout;
- final complete-continuum local recovery encoding;
- compact long-lived causal membership/checkpoint representation;
- multipart runtime receive orchestration and incomplete-range cursor policy;
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

The existing `APCLREC1`, `APCSREC1`, `APCSYNC1` and `APCSPRT1` encodings remain development contracts until the actual format freeze.

## 16. Android validation path

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

## 17. Immediate next implementation work

The next durability-oriented slices should:

1. wire complete authenticated multipart assembly into durable-cursor runtime catch-up without ever advancing the cursor past an incomplete publication;
2. add repeated multi-publication conflict/rebase-chain and fetch-failure adversarial tests;
3. introduce a complete local trusted-state container abstraction for several independent merge domains without smuggling in cross-domain transaction semantics;
4. keep stale-set semantic re-export above the transport/session layer and require a fresh `PublicationId` for every replacement;
5. later reproduce the current outbound/inbound restart matrix through Android process-kill and power-cycle tests.

The responsibility split remains simple: semantic state decides meaning, crypto decides confidentiality/authenticity, durability decides what survives restart, lifecycle decides whether new transport I/O may begin, runtime composes those boundaries, and transport only moves opaque authenticated objects.
