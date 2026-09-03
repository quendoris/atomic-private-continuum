# A.P.C. atomic multi-domain mutation research

Status: active research. This document records executable experiments about operations that require several merge domains to become visible as one semantic unit. It does not define a production transaction protocol.

Executable cases live in `tests/test_atomic_mutation_lab.py` and use `reference_model/atomic_mutation_lab.py`.

## 1. Why this experiment exists

The domain-local causality experiments established a useful default:

```text
independent merge domains
    -> independent causal observation boundaries
```

That is desirable because unrelated remote changes should not manufacture semantic causality or force local working-state seals.

However, some future operation may require several domain effects to be visible together. For such an operation, partial visibility could create a state that the user or invariant never intended.

The research question is therefore:

> Can A.P.C. retain domain-local causality while providing explicit all-or-none visibility for the rare operation that genuinely requires several domains to change together?

## 2. Atomic delivery is not the hard part

The first model gives one mutation a `MutationId` and a declared set of member merge domains.

A receiver buffers members until all declared pieces are present. It then validates all members first and swaps all prepared domain states into visibility together.

A two-domain mutation changing:

```text
left  60 -> 50
right 40 -> 50
```

is therefore never exposed as:

```text
left  50
right 40
```

merely because one transport part arrived earlier.

This property is conceptually similar to the existing multipart protected-capsule rule: incomplete transport state is not semantic state.

The test also injects a member with a missing causal dependency and confirms that failure leaves every touched domain unchanged.

Thus crash/transport all-or-none application is manageable.

## 3. Atomicity does not imply cross-domain causality

When one explicit atomic mutation is created, each member revision can still capture only the causal frontier of its own merge domain.

For domains `A` and `B`:

```text
Mutation M
├── member A.parents = frontier(A)
└── member B.parents = frontier(B)
```

The mutation grouping says that the effects must become visible together. It does not by itself say that the A revision causally observed the B revision or vice versa.

Therefore:

```text
visibility coupling
!=
causal ancestry coupling
```

This preserves the domain-local model for the simple no-concurrency case.

## 4. Concurrent atomic mutations expose the real problem

The difficult case is not delivery. It is merge.

The executable counterexample starts from:

```text
A = 0
B = 0
```

Two replicas concurrently create complete atomic mutations:

```text
X = (A=1, B=1)
Y = (A=2, B=2)
```

The member revision IDs are deliberately chosen so ordinary independent scalar merge picks:

```text
A from X
B from Y
```

The resulting state is:

```text
(A=1, B=2)
```

Neither user created that tuple.

Every individual merge domain converged correctly according to its scalar rule, yet the semantic atomic mutation tore during concurrent merge.

This falsifies the naive idea:

> buffer an atomic group during transport, then forget the group and merge all member domains independently forever.

Transport atomicity alone is insufficient.

## 5. Overlapping groups are harder than one shared tie-break

A tempting repair is to give all members of an atomic mutation the same group conflict rank so two mutations touching the exact same domains select one winner consistently.

That does not solve the general case.

The suite also creates:

```text
X touches {A, B}
Y touches {B, C}
```

If `Y` wins the overlapping conflict in `B`, ordinary independent merge can still retain `X`'s effect in `A`.

If X promised true all-or-none semantics, it is torn even though the conflicting overlap existed only in B.

This means strong atomic semantics can create a conflict component larger than one merge domain. Pairwise local tie-breaks are not automatically enough.

Possible consequences include:

- the explicitly atomic mutation itself becomes a coupled merge unit;
- overlapping atomic mutations temporarily form a larger conflict component;
- the format restricts which operations are allowed to claim strong cross-domain atomicity;
- many apparently multi-domain operations are redesigned so they become one logical-domain update instead.

No production rule is selected yet.

## 6. Architectural pressure: avoid unnecessary transactions

The result strengthens several earlier A.P.C. separations.

For example:

```text
AtomId
location domain
lifecycle domain
content domains
```

allows a move to update only location while content remains attached to stable identity. A move therefore does not need a transaction that rewrites location plus content.

Likewise, delete can be represented by lifecycle state without physically deleting the location anchor or every content field at the same instant.

This is valuable because every operation that can be expressed as one semantic merge-domain change avoids the much harder concurrent atomic-group problem entirely.

Therefore explicit multi-domain atomic mutation should be treated as a costly semantic primitive, not as a convenient batching mechanism.

## 7. Current decision

After this pass:

1. Domain-local causality remains the default for independent domains.
2. A declared multi-domain mutation can be buffered and applied all-or-none at a visibility/crash boundary without creating cross-domain causal ancestry by default.
3. Atomic delivery is not sufficient for strong atomic semantics under concurrent merge.
4. Two concurrent atomic mutations can tear into a hybrid state if their members later merge independently.
5. Overlapping atomic groups show that a shared per-group tie-break alone is not obviously sufficient for general strong atomicity.
6. Strong atomic mutation therefore requires a coupled conflict rule or a restriction/redesign that avoids the multi-domain transaction.
7. A.P.C. SHOULD prefer stable identity plus independent location/lifecycle/content domains where that turns an apparent multi-domain operation into one-domain semantic change.
8. `MutationId`, member `RevisionId`, transport publication identity and authentication identity remain separate concepts.

## 8. Next experiments

The next pass should attack the interactions that may let A.P.C. avoid transactions in practice:

- dirty content while the same atom is remotely moved;
- dirty content while the atom is remotely deleted;
- concurrent content edit plus delete-wins lifecycle state;
- insertion into or editing under a concurrently deleted container;
- whether move plus container membership can remain one location domain rather than a remove+insert transaction;
- explicit atomic operations that genuinely cannot be reduced to one merge domain;
- conflict-component growth for chains of overlapping atomic groups;
- whether a composite atomic merge domain can later split back into independent domains without changing semantics.

Checkpoint, finalization and sync-capsule research continue independently.
