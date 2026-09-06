# A.P.C. durable synchronization recovery

Status: **transport-independent crash-recovery record, protected durable record store, foreground transport gate, session coordinator and stale-outbox rebase transition implemented; Android power-loss behavior and the final portable/local encoding are not frozen**.

This document records the local durability rules that sit between semantic merge state and an opaque transport such as GitHub. It supplements `SYNC.md`, `SYNC_CAPSULES.md`, `DURABILITY.md`, `CORE_IMPLEMENTATION.md` and `SYNC_IMPLEMENTATION.md`.

## 1. Required failure property

A sudden process or device failure during synchronization may cause repeated transfer or repeated merge work. It must not cause:

- loss of an already acknowledged durable local edit;
- a transport cursor to advance beyond the local state that was durably produced from it;
- partial multipart state to become semantically visible;
- a locally exposed causal identity to become private again after restart;
- an unknown publication outcome to be treated as definite failure or definite success without reconciliation.

The intended user-visible property is therefore:

> A synchronization interruption may repeat work, but it must not create a torn logical state.

## 2. Cursor/state invariant

Transport cursors are bookkeeping only. A Git commit identity, repository head or another adapter cursor is not A.P.C. causality and is never ordered as logical state.

The local durability invariant is stronger:

```text
trusted semantic/recovery state
+
last fully applied transport cursor
+
pending outbound publications
        |
        v
one crash-atomic local recovery unit
```

A cursor must never become durable independently ahead of the trusted local state corresponding to that cursor.

The forbidden ordering is:

```text
receive remote state R1
        |
persist cursor = R1
        |
CRASH
        |
local semantic state still corresponds to R0
```

After restart such a device could incorrectly ask the transport for changes after `R1` and permanently skip state that it never durably merged.

The allowed asymmetry is the opposite: local state may temporarily be newer than a durable transport cursor, because refetch and idempotent merge can safely repeat work.

## 3. Implemented recovery record

`apc-sync` now provides a development `DurableSyncRecord` containing:

```text
DurableSyncRecord
├── trusted_state
├── applied_cursor
└── outbox
    └── PublicationId -> DurableOutboxEntry
        ├── expected_cursor
        └── exact protected wire objects
```

`trusted_state` is intentionally opaque to this layer. The higher semantic/recovery layer constructs and validates it. This keeps the crash-recovery transport bookkeeping from becoming a second semantic model.

`TransportCursor` is also opaque bytes. Its byte order has no temporal or causal meaning.

The current `APCSREC1` encoding is deterministic and strict, but it is explicitly pre-format local recovery framing rather than a compatibility commitment.

## 4. Outbound ordering

The safe outbound order is:

```text
local durable edit
        |
semantic finalization/canonicalization policy
        |
record transport handoff / exposure
        |
construct protected publication once
        |
persist {
    exposed trusted state,
    expected transport cursor,
    exact protected wire bytes
}
        |
LOCAL DURABILITY BARRIER
        |
network I/O may begin
```

The important boundary is that exposure and the retry material become durable **before** the first network handoff that might succeed externally.

`DurableSyncRecord::prepare_outbox()` models the recovery-state transition. The higher-level `stage_outbound()` coordinator clones the current record, prepares the outbox against the currently durable applied cursor, persists the complete next recovery unit through `SyncRecordStore`, and only then updates the caller's in-memory state. If persistence fails, no network-eligible state transition is exposed to the caller.

The sync layer still cannot independently prove that `trusted_state` contains the required semantic exposure bookkeeping; that proof remains the responsibility of the finalization-to-sync bridge.

## 5. Exact-byte retry

A prepared outbound publication stores the complete already-protected wire objects verbatim.

This is intentional. The current XChaCha20-Poly1305 protection uses a fresh random nonce, so encrypting the same clear publication a second time would normally produce different ciphertext bytes and therefore a different content-addressed transport object name.

Crash retry should instead be:

```text
protect once
        |
durably retain exact bytes
        |
try transport
        |
process/device dies
        |
restart
        |
retry the exact same protected bytes
```

Repeated transport attempts therefore do not require reconstructing an allegedly equivalent new ciphertext publication.

Reusing one `PublicationId` with different durable outbox bytes is rejected. Re-preparing the exact same publication is idempotent.

`publish_staged()` reads only the already-durable outbox entry and never removes it merely because `OpaqueTransport::publish()` returned success. Publication success and durable outbox retirement are deliberately separate transitions.

## 6. Unknown acknowledgement

A network acknowledgement is not the exposure boundary and is not assumed to survive process death.

Example:

```text
publish(expected = R0)
        |
transport accepts and advances to R1
        |
phone dies before receiving ACK
```

After restart the durable outbox still exists and the local applied cursor may still be `R0`.

The client must not silently discard the outbox and must not assume the first request failed. It may retry the exact protected bytes; if the expected head is now stale, the conflict becomes a reconciliation signal. The client then fetches from its durable cursor, authenticates and merges the returned protected state, and determines the publication outcome from observable transport state rather than from a lost ACK.

The executable session coordinator implements this separation:

- `publish_staged()` retries the exact durable bytes against the exact staged expected cursor;
- `fetch_from_durable_cursor()` fetches only from the cursor paired with the current durable local record;
- `commit_reconciled_outbox()` retires exactly one named publication only while durably committing the reconciled trusted state and observed transport head together;
- other staged/in-flight publications survive reconciliation of one entry.

The failure suite now also models the stronger case where the transport mutates remote state and then returns an error as though the response were lost. The durable outbox remains. Retry against the original expected cursor returns a conflict, and fetch from the durable cursor rediscovers the accepted exact object. No success/failure guess is required.

## 7. Incoming ordering

Inbound multipart handling remains semantically invisible until complete authenticated assembly.

The safe sequence is:

```text
fetch protected objects after durable cursor
        |
authenticate each part
        |
assemble complete publication
        |
decode + validate
        |
semantic merge
        |
construct new trusted local state
        |
update applied cursor in the same DurableSyncRecord
        |
LOCAL DURABILITY BARRIER
        |
new cursor may now be trusted after restart
```

Incomplete multipart state may be retained as a traffic optimization, but correctness does not depend on retaining it. It may be discarded on restart and refetched.

`DurableSyncRecord::apply_received()` intentionally preserves all pending outbound publications while advancing `trusted_state` and `applied_cursor` together.

`commit_received()` wraps that transition in the same clone-persist-swap rule as outbound staging: cursor encoding and durable persistence must both succeed before the caller's in-memory record advances. A failed durable commit therefore cannot leave the running process with a cursor newer than the recoverable state.

## 8. Protected durable record store

`ProtectedSyncRecordStore<B>` bridges the transport-independent recovery record to any `DurabilityBackend<Vec<u8>>`.

Its path is:

```text
DurableSyncRecord
        |
APCSREC1 deterministic recovery encoding
        |
XChaCha20-Poly1305 protection
        |
DurabilityBackend<Vec<u8>>
        |
commit_durable()
```

The caller supplies a non-empty local context that is combined with an internal durable-sync domain separator before AEAD protection. A different key or context cannot open the committed record. The backend therefore never receives clear `trusted_state`, cursor or outbox framing from this adapter.

This remains development recovery machinery rather than the native `.apc` format. The purpose is to establish the failure boundary before freezing a portable/local storage encoding.

## 9. Foreground-only transport gate

`ForegroundSyncLifecycle` and `ForegroundTransport<T>` encode the first executable foreground-only rule.

The lifecycle starts backgrounded. A platform binding must explicitly enter foreground before transport calls can begin. After `enter_background()`, new `head`, `fetch_since` and `publish` calls fail locally without touching the wrapped transport.

```text
process starts / no foreground signal
        |
transport blocked
        |
enter_foreground()
        |
new transport I/O allowed
        |
enter_background()
        |
new transport I/O blocked immediately
```

The gate does not create workers, alarms, timers or a daemon, so background correctness cannot accidentally depend on a scheduler.

There is an important race boundary: a request can already be in progress when the application backgrounds. The gate deliberately does not invent an outcome after that point. Platform/runtime integration may cancel the underlying request, but such cancellation can leave the external outcome unknown. The durable outbox and reconciliation protocol are the mechanism that makes this safe.

The integration suite now exercises that race directly: the wrapped transport accepts a publication, advances remote state, then the simulated application backgrounds and the response is lost. While backgrounded, retry is blocked locally and never reaches the transport. After foreground resume, the same durable retry discovers a stale-head conflict and fetch rediscovers the accepted bytes.

Therefore the lifecycle rule is:

> Backgrounding prevents new sync I/O; an already-started mutation is recovered as an unknown-outcome operation, never guessed from lifecycle state.

Immediate catch-up on resume remains a platform orchestration responsibility. It should call the same durable-cursor/outbox session path rather than a separate background protocol.

## 10. Overlapping publications and stale outbox entries

Multiple pending publications may coexist. Reconciling one entry must not silently rewrite another entry that was prepared earlier.

Suppose both `P1` and `P2` were durably staged against transport cursor `R0`. If `P1` is reconciled and the durable applied cursor advances to `R1`, `P2` still contains its exact original protected bytes and still targets `R0`.

That stale entry must not be mutated in place:

```text
P2 @ R0
        |
R0 -> R1 after reconciliation
        |
P2 now stale
        |
DO NOT mutate P2 bytes
DO NOT reuse P2 PublicationId
        |
merge/re-export/re-protect against R1
        |
fresh PublicationId P3 @ R1
```

`OutboxRebase` plus `commit_rebased_outbox()` implement the crash-atomic bookkeeping transition for this case. The caller supplies the already-reconciled trusted state, a fresh publication identity and newly protected exact wire objects. The coordinator clones the durable record, retires the named stale entry, prepares the fresh entry against the newer cursor, persists the complete resulting recovery unit once, and only then swaps the caller's in-memory record.

The transition rejects reuse of the stale `PublicationId`. A persistence failure leaves the old publication, old cursor and old trusted state unchanged.

This function deliberately does not decide whether a stale publication should be rebased or is now semantically redundant. That decision belongs above the transport bookkeeping layer after semantic merge.

## 11. Implemented crash/restart and failure tests

The Rust suite tests the durable recovery boundary at several levels.

One filesystem test commits `{old state, R0}`, constructs `{merged state, R1}`, writes and synchronizes the new candidate object, but deliberately does not publish it as the committed root. After closing and reopening the backend, recovery still returns `{old state, R0}`. After a complete durable commit, recovery returns `{merged state, R1}`.

A second test durably stores an outbox, closes/reopens the backend, verifies the exact protected wire bytes survive, applies an incoming cursor while retaining that outbox, closes/reopens again, then reconciles and retires only the named publication. The complete recovery record is protected with the real authenticated-encryption layer before filesystem persistence.

The deterministic session failure matrix now covers:

1. outbound outbox persistence failure leaves the caller's in-memory record and outbox unchanged;
2. ordinary network failure after durable staging leaves the exact protected retry bytes and durable outbox intact;
3. remote acceptance followed by response loss remains an unknown outcome that is recovered by conflict + refetch;
4. inbound merged-state persistence failure cannot advance only the running process's cursor;
5. reconciliation persistence failure cannot retire the pending outbox or advance the cursor;
6. reconciling one pending publication preserves every other pending publication verbatim;
7. stale publication rebase requires a fresh `PublicationId` and commits stale-retirement + replacement atomically;
8. rebase persistence failure preserves the complete previous recovery state;
9. foreground backgrounding blocks all new transport operations without touching the wrapped transport;
10. background transition during a remotely accepted in-flight publication is recoverable after resume without guessing the external outcome.

The filesystem/GitHub lost-ACK integration test additionally combines the protected record store, session coordinator and GitHub CAS-like adapter and verifies restart/retry/reconciliation through the development durable backend.

## 12. What this does not yet prove

The current tests establish crash-consistent behavior at the Rust durability contract and Unix development backend. They do **not** yet prove actual handset power-loss behavior.

In particular:

- process death is not identical to loss of electrical power;
- Android filesystem/storage-stack durability behavior must be validated on a real device;
- the final Android storage backend may differ from the current Unix development backend;
- the final trusted-state encoding is not frozen;
- the final cursor encoding for GitHub and other transports is not frozen;
- finalization/private-squashing semantics still need an explicit bridge into publication preparation;
- replay/rollback policy remains open;
- long-offline rebootstrap/generation retention remains separate from this local crash rule;
- the current foreground gate prevents new calls but does not itself cancel an already-running platform HTTP request.

The implementation must preserve these seams rather than treating the development recovery record as the native `.apc` format.

## 13. Android validation path

Once the first Android binding exists, the same state machine should be exercised through ADB rather than by manual UI testing.

A useful progression is:

```text
Rust unit/property tests
        |
Unix filesystem restart tests
        |
subprocess SIGKILL tests
        |
Android process kill through ADB
        |
kill during outbound transfer
        |
kill after remote acceptance / before local ACK handling
        |
kill after inbound merge / before local durable commit
        |
relaunch and invariant verification
        |
eventual controlled device power-cycle tests
```

The test oracle should inspect durable state, cursor and outbox rather than merely checking that the application opens.

## 14. Immediate next implementation work

The next slice should:

1. give the GitHub adapter an explicit production reversible conversion between `GitHubCommitOid` and local opaque `TransportCursor` bytes without introducing ordering semantics;
2. define the narrow finalization-to-sync preparation seam so exposure bookkeeping is constructed by trusted semantic code rather than passed as an unstructured byte image by application glue;
3. add fetch/merge failure injection around stale-outbox rebase and chains of several concurrently pending publications;
4. add a platform cancellation bridge for already-running foreground HTTP operations while preserving unknown-outcome recovery through the durable outbox;
5. make foreground resume trigger immediate durable-cursor catch-up/reconciliation through the same session path;
6. later reproduce the same matrix on Android through ADB.

The intended architecture remains simple: semantic state decides meaning, crypto decides authenticity/confidentiality, durability decides what survives restart, lifecycle decides whether new transport I/O may begin, and transport only moves opaque authenticated objects.
