# A.P.C. durability contract

Status: **implementation boundary defined; physical storage layout not frozen**.

This document refines the continuity requirements for the portable core. It specifies when a local state change may be acknowledged as committed and what must remain recoverable after process death or device power loss. It does not choose the final `.apc` binary layout, filesystem protocol, database, journal or page format.

Normative requirements remain in `REQUIREMENTS.md`.

## 1. Core invariant

A user-visible state change has only two acceptable outcomes:

```text
committed and recoverable

or

not acknowledged as committed
```

There must not be an acknowledged state that exists only in volatile memory.

Once a commit operation reports success, a later process crash or power loss must recover that committed state or a causally later committed state. It must not recover an earlier state merely because the acknowledgement raced the physical durability boundary.

## 2. Logical commit protocol

The current core exposes a backend-independent durability protocol:

```text
complete candidate state
        |
write candidate
        |
durability barrier for candidate data
        |
atomically publish candidate as committed root
        |
durability barrier for committed root
        |
ACK success to caller
```

The two durability barriers serve different purposes.

The candidate barrier ensures that the published root never intentionally points at data that has not itself reached the backend durability boundary.

The committed-root barrier ensures that once success is returned, the publication decision itself survives power loss.

## 3. Crash outcomes

The protocol distinguishes the following boundaries.

| Crash point | Permitted recovered state |
| --- | --- |
| before candidate write | old committed state |
| after candidate write, before candidate barrier | old committed state |
| after candidate barrier, before publication | old committed state |
| after publication, before committed-root barrier | old or new complete state |
| after committed-root barrier, before caller receives ACK | new complete state |
| after ACK | new complete state |

Before the committed-root barrier, the caller has not received a successful commit acknowledgement. Therefore recovering either the old state or the new complete state after an ambiguous publication is acceptable.

After the committed-root barrier, recovering the old state is not acceptable even if the process dies before the caller receives the return value. This is an ordinary uncertain-outcome retry problem, not permission to roll durability backward.

At no crash point is a hybrid or structurally partial committed state acceptable.

## 4. Physical implementation freedom

The logical protocol deliberately does not require one physical design.

A conforming backend may use, for example:

- copy-on-write pages plus a small committed-root record;
- double-buffered manifests;
- a write-ahead journal;
- database transactions with documented durability semantics;
- another mechanism that provides equivalent ordering and recovery guarantees.

The core API must not expose a temporary implementation choice as portable format semantics.

In particular, filesystem rename, directory synchronization, block-device cache behavior and platform-specific persistence calls belong to the concrete backend and must be validated on each supported platform before that backend is considered production-safe.

## 5. Candidate state is not committed state

A fully written and durable candidate is still not the committed state until publication.

This separation permits interrupted commits to leave orphaned candidate data without corrupting the last committed root. Orphan reclamation is storage maintenance and must not affect logical state.

Absence of acknowledgement also does not prove that the new state was not committed. A crash after the committed-root barrier but before the return value reaches the caller can leave the new state durable. Retry logic must therefore be idempotent or recover current committed state before deciding what to do next.

## 6. Relationship to working-state semantics

Durability of local editing and causal finalization are separate concerns.

```text
WorkingEpoch
    -> crash-safe local snapshot
    -> later seal to RevisionId
    -> optional finalization
    -> later transport exposure
```

A storage backend must be capable of persisting pending working epochs, their observed causal frontier, finalized statement bookkeeping and exposure bookkeeping together when those values form one local recovery boundary.

A physical commit must not restore `WorkingScalar` from one generation while restoring `FinalizationLedger` from another generation.

`LocalScalarSnapshot<T>` exists specifically so those already-validated boundaries can be persisted as one recovery unit.

## 7. Current Rust boundary

`crates/apc-core/src/durability.rs` defines:

- `DurabilityBackend<S>` — the minimum backend operations required by the protocol;
- `commit_durable()` — the ordering coordinator that returns success only after both durability barriers.

The current tests use a crash-injecting in-memory backend. The simulator deliberately permits both legal outcomes for an unsynchronized root publication and verifies that:

- a volatile candidate never replaces the old committed state;
- a durable but unpublished candidate never replaces the old committed state;
- a crash after publication but before the root barrier can recover old or new, but never a hybrid;
- after the root barrier, the new state survives a simulated crash;
- after `commit_durable()` returns success, the new state survives a simulated crash.

The simulator is not a production storage engine.

## 8. Next implementation work

Before freezing any `.apc` physical encoding, the next storage work should establish:

1. a concrete local filesystem backend for development;
2. deterministic encoding for the recovery object used by that backend, explicitly marked pre-format while unstable;
3. fault injection around every write, flush, publication and reopen boundary;
4. process-kill tests on desktop;
5. Android filesystem tests through ADB once the first native binding/test harness exists;
6. verification that successful acknowledgement survives actual process death and device restart conditions supported by the test environment.

The physical backend may change after these tests. The acknowledgement contract may not.
