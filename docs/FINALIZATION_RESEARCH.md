# A.P.C. portable revision finalization research

Status: active research. This document records executable experiments around the boundary between crash-safe local causal state and final portable authenticated revisions. It does not select a signature, forward-secure signature scheme or key-evolution primitive.

Executable cases live in `tests/test_finalization_lab.py` and use `reference_model/finalization_lab.py`.

## 1. Question

Previous experiments established three different layers:

```text
local durable working state
        !=
causal observation state
        !=
external transport publication
```

Private causal squashing then showed that never-exposed dominated local causal nodes can sometimes be removed before publication.

The next question is:

> At what point must a local causal identity become a final portable authenticated revision?

Two broad candidates were compared:

```text
early-finalized
    every causal observation boundary immediately freezes/authenticates a portable revision

provisional
    local causal identities remain crash-safe but unauthenticated until the external boundary requires final portable state
```

## 2. Working-epoch identity alone is not a safe conflict identity

A tempting design is to let a local `WorkingEpochId` participate in temporary conflict resolution and then mint an unrelated fresh `RevisionId` when the state is finally published.

The executable counterexample rejects that if scalar concurrency is still resolved by canonical `RevisionId`.

Suppose a remote concurrent revision has ID `100`.

During local provisional materialization:

```text
WorkingEpoch/conflict ID = 200
remote RevisionId        = 100
```

The local value wins the deterministic concurrent tie-break.

If finalization later replaces that causal/conflict identity with fresh `RevisionId = 50`, the exact same unresolved conflict becomes:

```text
final local RevisionId = 50
remote RevisionId      = 100
```

and the remote value wins.

The user-visible result changed only because authentication/finalization happened.

That is not acceptable.

Therefore a device-local `WorkingEpochId` may still exist as storage/recovery identity, but it cannot be the only identity used when an unresolved local value participates in portable causal semantics.

## 3. Stable reserved causal identity

The tested safe direction is:

```text
WorkingEpochId              device-local durable record identity
reserved RevisionId         stable causal/conflict identity
FinalizedStatement          immutable authenticated statement created later
```

A `RevisionId` may therefore be allocated before it is cryptographically finalized.

Its ID remains opaque and does not encode time. The important property is only that the same causal identity used for conflict semantics survives finalization.

The experiment confirms that using the same reserved `RevisionId` before and after finalization preserves the conflict result.

This refines the previous provisional-epoch hypothesis:

> provisional does not necessarily mean "no RevisionId exists". It means the causal identity may exist without yet freezing/signing the final portable statement.

## 4. Finalization freezes the statement

The test-only finalization ledger snapshots the fields that a future authentication statement would cover:

```text
revision_id
value
parents
```

After finalization, changing those fields is rejected by the model.

This matters because private causal squashing can rewrite parent relationships before exposure. A rewrite that is logically safe before finalization may become cryptographically impossible after a signature or authenticated key transition has committed to the old parent set.

Thus:

```text
causal canonicalization / private squashing
        BEFORE
portable authentication finalization
```

is the useful optimization order.

## 5. Early finalization versus provisional finalization

The suite creates 64 local causal observation markers interleaved with a remote causal chain, followed by one final local revision.

In the **early-finalized** model every local observation marker is immediately frozen as a portable statement.

Result:

```text
local final statements          65
modeled signing transitions     65
```

The same private squashing transformation is then rejected because it would remove or rewrite already-finalized statements.

In the **provisional** model the same 64 intermediate local causal identities remain private and unfinalized.

Before external publication they are causally dominated by the final local state and exposure-aware squashing removes all 64.

Then only the surviving local frontier is finalized and handed off:

```text
removed private local nodes     64
final local statements           1
modeled signing transitions      1
```

The current scalar frontier and materialized value are preserved.

This is a major reduction in authentication/key-evolution pressure when many remote observation boundaries occur before any local state is actually published.

## 6. The signing-transition counter is not cryptography

The reference model deliberately counts one logical signing transition per newly finalized local statement.

This does **not** claim that the eventual production primitive must rotate a key once per revision.

It exists to expose a constraint:

- if the selected key-evolution construction binds a new authentication state to every finalized revision;
- and old private authentication state becomes unusable;
- then early finalization can make later private causal canonicalization expensive or impossible.

A future cryptographic design may support batching, skipped epochs or another studied construction. Those possibilities must be demonstrated by the chosen primitive rather than assumed by the logical model.

## 7. Publication between observation epochs

Provisional state is not permission to erase anything that might already be external.

The suite publishes one local revision, then performs another private observation epoch and later creates a final local revision.

The first published revision survives private squashing because its identity has already crossed the external boundary.

Only the later never-exposed dominated local marker is removed.

The final state therefore requires two local finalizations in that scenario:

```text
published local boundary 1
+
final later local boundary
=
2 final local statements
```

This is expected. Optimization opportunity follows actual exposure history, not merely current dominance.

## 8. Handoff, acknowledgement and retry

Transport handoff remains the conservative exposure boundary.

Once a finalized revision is handed to transport, local loss of the acknowledgement cannot prove that the external side did not receive it.

The test performs:

```text
finalize
handoff
ACK lost
finalize retry
handoff retry
```

and confirms that finalization is idempotent for the same immutable statement: the modeled signing-transition count remains one.

The state is already considered exposed after the first handoff.

## 9. Unfinalized dependencies cannot cross the transport boundary

A finalized child may still name a local parent that is only provisional.

Publishing that child would leak/use the provisional parent's causal identity without a corresponding immutable authenticated local statement.

The lab rejects such handoff.

Before transport, the implementation must either:

- safely squash/canonicalize the private dependency away;
- or finalize the required local dependency chain.

This makes the publication pipeline explicit:

```text
private working/causal state
        |
canonicalize/squash what is safe
        |
freeze required portable statements
        |
authenticate / key-evolve as selected
        |
protect sync capsule
        |
transport handoff
```

## 10. Crash recovery

Provisional causal identity is still durable state.

The experiment snapshots and restores:

- local causal IDs;
- which IDs remain private;
- which statements have already been finalized;
- which IDs have already been handed off;
- the modeled signing-transition count.

After restart, a provisional ID remains provisional and can be finalized once. A previously finalized statement remains finalized, and retrying finalization does not create another transition.

Production storage must make the real equivalent of these boundaries crash-atomic.

## 11. Relationship to key evolution

The experiment strengthens several requirements on the future per-replica authentication design.

The authentication layer should not force A.P.C. to cryptographically finalize every local causal observation marker before the marker is actually needed outside the replica.

At the same time, once a portable statement is finalized and especially once it is handed off, its authenticated contents must be immutable.

A promising layering is therefore:

```text
local durable WorkingEpochId
        |
reserved opaque RevisionId when causal conflict identity is needed
        |
private canonicalization / squashing
        |
final portable revision statement
        |
per-replica authentication/key evolution
```

The exact mapping between one finalized revision and one key-evolution transition remains open.

## 12. Current decision

After this pass:

1. A fresh unrelated `RevisionId` at finalization can change an unresolved scalar conflict winner and is rejected.
2. Device-local `WorkingEpochId` and portable `RevisionId` may remain separate concepts, but causal conflict semantics require a stable identity that survives finalization.
3. An opaque `RevisionId` may be reserved before it is authenticated/finalized; allocation is not the same as cryptographic finalization.
4. Finalization freezes the portable statement fields covered by authentication.
5. Private causal squashing should occur before finalization whenever possible.
6. In the 64-observation experiment, provisional finalization reduces modeled local final statements/signing transitions from 65 to 1 before first publication.
7. Already-exposed local causal boundaries remain immutable and cannot be erased by this optimization.
8. Exposure begins at transport handoff, not acknowledgement.
9. Handoff must not depend on an unfinalized local causal identity.
10. Provisional/finalized/exposed boundaries are crash-recovery state, not volatile UI state.

## 13. Next experiments

The next pass should attack:

- whether causal identity reservation can be delayed even further for domains that never encounter a concurrent remote value;
- multiple independent dirty domains, where a remote change touches only one domain;
- atomic multi-domain local edits and shared versus domain-local observation boundaries;
- interaction with lifecycle delete tombstones;
- interaction with movable sequence locations;
- crash points between canonicalization, finalization, protected-capsule creation and transport handoff;
- whether a batch of several final portable revisions can share one safe per-replica authentication/key-evolution transition under a studied cryptographic construction;
- finalization/capsule size under long sessions with frequent remote polling but infrequent actual causal conflicts.

Checkpoint, sync-capsule and moved-anchor research continue in parallel.
