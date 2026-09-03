# A.P.C. hierarchy and containment research

Status: active research. This document records executable experiments around hierarchical containment, parent deletion and cross-container moves. It does not freeze tree semantics or cycle resolution.

Executable cases live in `tests/test_hierarchy_lab.py` and use `reference_model/hierarchy_lab.py`.

## 1. Candidate: parent membership as one location domain

A child does not need to be represented as independently removed from one container and inserted into another.

The experiment instead gives each stable `AtomId` one causal parent-location register:

```text
child AtomId
    |
parent-location register
    |
current parent AtomId or root
```

Moving a child between containers is therefore one semantic location update.

This mirrors the earlier single-list move result:

```text
stable identity
!=
current location
```

and avoids a remove+insert transaction that could duplicate the same logical child under concurrency.

## 2. Concurrent moves of one child do not duplicate it

Two replicas concurrently move the same child to different parents.

The parent-location register merges the two candidate locations using the existing causal scalar rule. Exactly one location materializes, while the child `AtomId` remains singular.

This is a strong result for cross-container move semantics: stable identity plus one location register can avoid the classic duplicate-object problem of delete+reinsert moves.

The exact parent-position representation still needs ordering information inside each container; this lab isolates only containment identity.

## 3. Parent delete plus concurrent child insertion

One replica deletes parent P while another offline replica inserts a new child C under P.

After merge:

```text
P = deleted
C.parent = P
```

The child record is retained, but visibility traversal hides C because its ancestor is deleted.

Thus deletion need not erase or rewrite all descendant records atomically.

However, this exposes an important UX/lifecycle question: a user can create valid offline data under a parent that another replica has concurrently deleted, and the merged state may hide that new data.

The model preserves the data so a future policy can still reason about it. It does not yet decide whether A.P.C. should:

- keep the child hidden under delete-wins semantics;
- rehome it deterministically to a surviving ancestor;
- surface a reconciliation state;
- or make it available only if explicit restore exists.

Silently destroying the child is rejected.

## 4. Child moved out while parent is concurrently deleted

A different scenario starts with child C under P.

One replica deletes P while another concurrently moves C to root.

After merge the parent-location revision for C causally supersedes its old placement under P, so C's active parent becomes root. The delete of P therefore no longer hides C.

Result:

```text
P hidden
C visible at root
```

This gives a useful natural behavior: deleting a container does not have to mean that every concurrently escaping descendant is itself deleted.

Whether this is the final desired subtree-delete policy remains open, but it demonstrates that lifecycle filtering and stable child location can express the distinction without a multi-domain transaction.

## 5. Delete does not rewrite descendant parent identity

When P is deleted, child C may continue to retain:

```text
C.parent = P
```

while being invisible because the ancestor path crosses a deleted atom.

That retained structural relationship matters for:

- long-offline merge;
- future restore research;
- deterministic compaction proofs;
- child insertions that were created before the delete became observable.

Again:

```text
visibility
!=
physical/causal record existence
```

## 6. Critical counterexample: independent parent registers can create cycles

The promising location-register model has a serious tree-specific failure mode.

Start with two root atoms:

```text
A
B
```

Replica X moves A under B:

```text
B
└── A
```

Replica Y concurrently moves B under A:

```text
A
└── B
```

Each move is locally valid.

After ordinary independent parent-register merge, both winning locations may be active simultaneously:

```text
A.parent = B
B.parent = A
```

The result is a parent cycle.

The executable model detects and rejects visibility traversal rather than inventing an ordering.

This is the hierarchy analogue of the broader move-validity problem: independent per-element location convergence is not sufficient to guarantee a globally valid tree.

## 7. Why a simple tie-break is not obviously enough

A tempting repair is to detect a cycle and discard the move with the smaller or larger `RevisionId`.

That is not yet accepted.

Discarding one active move requires answering what location becomes active instead:

- the previous historical placement;
- another concurrent frontier candidate;
- root;
- a deterministic valid ancestor;
- or some separately retained move intent.

The current scalar register normally treats causally dominated old locations as historical rather than active conflict candidates. Restoring one because a later move became globally invalid adds a new lifecycle/validity rule that must itself converge under larger cycles and out-of-order delivery.

Therefore cycle handling needs an explicit tree-move validity model rather than an ad-hoc UI repair.

## 8. Architectural consequence

The experiments now separate two statements that are both true:

1. **Stable identity plus one parent-location register is excellent for avoiding duplicate children and remove+insert transactions.**
2. **Hierarchical validity is a global constraint that cannot be guaranteed by independently materializing every parent register.**

So the likely production direction is not to abandon stable parent location, but to layer deterministic validity resolution over it.

That resolution must remain:

- state-based or otherwise compatible with A.P.C. merge semantics;
- deterministic;
- independent of wall-clock order;
- robust to long-offline replicas;
- safe under cycles involving more than two nodes;
- compatible with delete-wins lifecycle and hidden ancestors.

## 9. Current decision

After this pass:

1. Cross-container membership SHOULD continue to be researched as one stable child-location domain rather than remove+insert duplication.
2. Concurrent moves of the same child can select one parent without duplicating the child.
3. Parent deletion may hide descendants through reachability without rewriting every descendant record.
4. A child inserted under a concurrently deleted parent is retained but hidden in the current delete-wins candidate; final UX/policy is unresolved.
5. A child concurrently moved out of a deleted parent can remain visible outside that subtree in the tested model.
6. Independent parent-location registers can create invalid active cycles under concurrent cross-moves.
7. Tree validity therefore requires an additional deterministic cycle/invalid-move policy before hierarchical moves can freeze.
8. Cycle repair must not infer order from timestamps, GitHub arrival or ID magnitude as causality.

## 10. Next experiments

The next hierarchy pass should attack:

- deterministic cycle resolution for 2-node, 3-node and longer concurrent move cycles;
- whether an invalid move can safely fall back to the last causally valid placement without resurrecting arbitrarily old state;
- multiple concurrent moves per node where only some combinations form a cycle;
- parent deletion combined with a cycle candidate;
- ordered position within each parent together with parent-location selection;
- historical anchor behavior when a parent itself moves;
- child insertion under a parent that is moved and deleted concurrently;
- compaction requirements for hidden descendants and obsolete parent positions.

The strongest next candidate should be compared against the earlier 2024 JSON-move adversarial cases without adopting operation-log/Lamport semantics merely because that work uses them.
