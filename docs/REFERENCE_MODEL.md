# A.P.C. executable reference model

The reference model exists to make the current logical rules executable before production storage, cryptography, Android integration or GitHub synchronization are implemented.

It is intentionally small, explicit and inefficient where that keeps semantics easy to inspect.

Code: [`reference_model/apc_model.py`](../reference_model/apc_model.py)
Tests: [`tests/test_reference_model.py`](../tests/test_reference_model.py)

## 1. Scope

The current model implements only enough behavior to test the first stable merge rules:

- opaque ID-based logical identities;
- scalar causal precedence;
- deterministic scalar resolution for genuine concurrency;
- independent atom fields;
- insertion-only ordered collections;
- deterministic client-side state merge;
- continuum identity mismatch rejection.

It does not implement:

- binary `.apc` encoding;
- encryption or signatures;
- key evolution;
- attachment storage;
- deletion/tombstone compaction;
- movement/reordering of an existing member;
- Android state persistence;
- Git/GitHub transport.

Absence from this model does not imply absence from A.P.C.; it means the behavior has not yet been reduced to a tested primitive here.

## 2. Reference identifiers

Tests use hexadecimal strings and normally render them as 256-bit values for readability and deterministic fixtures.

This is not the final serialized identifier encoding.

The only semantic properties used by the model are:

1. IDs are opaque;
2. equality is exact;
3. canonical byte ordering exists for deterministic tie-breaks;
4. ID ordering never means chronological ordering.

## 3. Reference causal context

Each scalar revision carries the set of revision IDs already observed when it was created.

A new local scalar assignment observes every revision currently known to that register, including concurrent frontier revisions.

This makes causality obvious:

```text
R0 -> RA
R0 -> RB

RA || RB
```

After merging both and creating `RC`:

```text
RA -> RC
RB -> RC
```

Therefore `RC` causally supersedes both even if the byte value of `RC` is smaller than both IDs.

The reference representation stores explicit ancestor IDs and therefore grows with history. It is a correctness model, not a production metadata strategy. A compact causal representation must preserve the same observable relations before it replaces this model.

## 4. Scalar register

The scalar register is modeled as a set of authenticated logical revisions plus causal context.

Materialization works in two stages:

1. remove revisions causally superseded by another known revision;
2. if several incomparable frontier revisions remain, choose the one with the greatest canonical `RevisionId` byte sequence.

The second rule is only a deterministic concurrency tie-break. It is not recency.

The model intentionally retains the concurrent frontier instead of discarding losing concurrent revisions immediately, because a future causal revision must be able to prove that it observed and superseded them.

## 5. Ordered collection experiment

The first executable sequence experiment is deliberately not a production CRDT selection.

Each insertion has:

```text
placement_id
atom_id
optional left_atom_id
optional right_atom_id
```

For example, starting from:

```text
A
B
```

an insertion between them contributes constraints:

```text
A < X < B
```

Two replicas may independently contribute:

```text
A < X < B
A < Y < B
```

The merged constraint graph therefore preserves both insertions. A deterministic topological sort uses `PlacementId` bytes only to order simultaneously available incomparable members.

Expected result:

```text
A
X/Y
Y/X
B
```

with the same X/Y order on every implementation using the same rule.

This experiment is useful because it proves that concurrent insertion preservation does not require timestamps or server arbitration.

It is not yet sufficient for production because movement, delete interaction, malicious/invalid cyclic constraints and metadata growth require a complete design. The model rejects cycles rather than attempting to guess user intent.

## 6. Merge algebra

The reference state merge is required to satisfy logical:

```text
merge(A, A) = A
merge(A, B) = merge(B, A)
merge(merge(A, B), C) = merge(A, merge(B, C))
```

Tests compare logical materialized state and merge frontier information, not serialized bytes.

Duplicate transport delivery is therefore harmless at the logical layer.

## 7. Current tests

The initial test set checks:

- causal scalar precedence;
- concurrent scalar ID tie-break;
- a causally later revision winning despite a smaller ID;
- duplicate scalar delivery;
- two concurrent insertions in one sequence gap;
- 128 concurrent insertions merged repeatedly in randomized orders;
- cycle rejection in the experimental ordering graph;
- field independence inside one atom;
- commutativity, associativity and idempotence on composed reference states;
- continuum mismatch rejection.

The randomized sequence test is not a substitute for property-based generation, but it gives the first executable pressure test without adding dependencies.

## 8. Promotion rule

A mechanism demonstrated by the reference model does not automatically become part of the production format.

Promotion requires:

1. the semantics to satisfy `LOGIC.md`;
2. adversarial cases to be added to tests;
3. metadata growth to be understood;
4. behavior under long-offline replicas to be understood;
5. the production representation to preserve the same merge algebra;
6. independent implementations/test vectors where the mechanism becomes format-critical.

The reference model is allowed to be replaced whenever a simpler or stronger construction proves the same required behavior.
