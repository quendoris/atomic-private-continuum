# A.P.C. logical conformance tests — draft

These are implementation-independent tests for the logical model in `LOGIC.md`.

They are not tied to a binary encoding or programming language. Concrete test vectors may later add canonical serialized bytes.

## 1. Scalar causal precedence

Given one scalar field with:

```text
R0: value = "draft"
```

and a revision:

```text
R1: value = "final"
causal context observes R0
```

merge MUST materialize `"final"` regardless of the byte ordering of `R0` and `R1`.

This proves that causal precedence is stronger than the concurrent-ID tie-break.

## 2. Concurrent scalar tie-break

Let two replicas independently observe `R0` and produce:

```text
RA: value = "alpha"
RB: value = "beta"
```

with:

```text
RA || RB
```

If canonical unsigned byte ordering gives:

```text
RA < RB
```

the materialized value MUST be `"beta"` on every implementation.

The merge result MUST be identical for:

```text
merge(A, B)
merge(B, A)
```

No timestamp or arrival order may alter it.

## 3. Later causal edit with a smaller ID

Start from the concurrent state in test 2.

After a replica has incorporated both `RA` and `RB`, it produces:

```text
RC: value = "gamma"
causal context observes RA and RB
```

Choose an `RC` whose canonical byte value is lower than both `RA` and `RB`.

The result MUST still be `"gamma"`.

This test is mandatory because it proves that revision-ID ordering is only a concurrency tie-break and is never interpreted as recency.

## 4. Concurrent insertion preservation

Starting sequence:

```text
A
B
```

Replica X inserts atom `X1` between A and B.
Replica Y independently inserts atom `Y1` between A and B.

Every merged result MUST contain exactly:

```text
A
X1
Y1
B
```

or:

```text
A
Y1
X1
B
```

according to the canonical order defined by the selected sequence construction.

It MUST NOT contain only one insertion.

All conforming implementations MUST choose the same relative order for X1 and Y1.

## 5. Field independence

Starting atom:

```text
sticker S
├── title = "Old"
└── children = [A]
```

Replica X changes:

```text
title = "New"
```

Replica Y concurrently inserts child B.

Merged state MUST be:

```text
sticker S
├── title = "New"
└── children = [A, B]
```

The title write MUST NOT overwrite the children collection.

## 6. Independent atoms

If replica X edits atom A and replica Y edits atom B, with `A.id != B.id`, both edits MUST survive merge regardless of concurrency.

No container-level replacement is allowed to erase the independent atom update.

## 7. Merge algebra

For arbitrary valid logical states A, B and C belonging to the same continuum, property-based tests MUST verify:

```text
merge(A, A) == A
merge(A, B) == merge(B, A)
merge(merge(A, B), C) == merge(A, merge(B, C))
```

The comparison is logical-state equality, not ciphertext byte equality.

Test generation SHOULD include:

- independent atom edits;
- concurrent scalar writes;
- causal scalar chains;
- concurrent sequence inserts;
- simultaneous changes to different fields of one atom;
- unknown optional extension data;
- deleted/tombstoned atoms once deletion semantics are fixed.

## 8. Duplicate delivery

Applying the same remote state repeatedly MUST NOT create duplicate atoms, duplicate list items or new logical revisions.

Conceptually:

```text
merge(local, remote) == merge(merge(local, remote), remote)
```

This is required because transports may retry downloads or publications.

## 9. Timestamp irrelevance

Take two logically identical states and alter only non-semantic human-readable timestamps, filesystem modification times, Git commit times or transport arrival order.

Logical merge output MUST remain unchanged.

A test implementation SHOULD deliberately set clocks backwards and far into the future to prove that merge code never consults them.

## 10. Git metadata irrelevance

The same two `.apc` logical states presented under different:

- Git commit hashes;
- branch names;
- GitHub usernames;
- commit messages;

MUST produce the same A.P.C. merge result.

## 11. Continuum mismatch

States with different `ContinuumId` values MUST NOT be semantically merged as replicas of the same continuum.

The core must return an explicit mismatch/error result without altering either input.

## 12. Corrupt/unverifiable input

If an input cannot satisfy the format's required authenticity/integrity checks, merge MUST fail before its untrusted payload affects authoritative local state.

A failed merge MUST NOT partially apply atoms from the invalid input.

## 13. Unknown optional extension preservation

When an implementation reads a state containing an unknown extension explicitly marked as preservable/optional, then edits unrelated known content and writes the state again, the unknown extension MUST remain present and unchanged unless the extension specification permits canonical transformation.

An unknown critical extension MUST prevent unsafe semantic modification rather than being silently discarded.

## 14. Large replica set

Generated convergence tests SHOULD include states produced by hundreds or thousands of distinct `ReplicaId` values.

Correctness MUST NOT depend on a hard-coded small replica count.

Performance may degrade and is measured separately; semantic convergence may not.

## 15. Key-evolution independence

For two replicas with independent authentication evolution:

```text
A7 -> A8
B12 -> B13
```

both transitions MUST be valid concurrently if each is valid under its own authenticated replica chain.

Processing A's transition MUST NOT make B12 invalid merely because B transitioned from the same prior continuum state at approximately the same real-world time.

## 16. Same-replica fork detection

Once a concrete replica key-evolution scheme is selected, conformance tests MUST include an intentionally cloned current private replica state producing two incompatible next states from one key state.

The implementation must follow the specified fork-handling rule. It MUST NOT silently interpret that situation as ordinary two-replica concurrency.

## 17. Crash/durability boundary

For every user-visible content mutation acknowledged as committed:

1. commit the mutation;
2. terminate the process or simulate abrupt power loss immediately after the durability boundary;
3. reopen the continuum;
4. verify that the acknowledged mutation is present exactly once.

Repeating the crash before the durability boundary may yield either the prior complete state or the new complete state according to the chosen transactional design, but MUST NOT yield a structurally torn logical state.

## 18. Test philosophy

Example tests demonstrate intended behavior. Property tests establish that the merge algebra survives combinations that were not manually anticipated.

The first core prototype should therefore treat merge algebra tests as part of the architecture rather than as release polish added later.
