# A.P.C. hierarchy validity-resolution research

Status: active research. This document records an executable candidate for resolving parent cycles created by concurrent hierarchical moves. It is not a production tree algorithm.

Executable cases live in `tests/test_hierarchy_validity_lab.py` and use `reference_model/hierarchy_validity_lab.py`.

## 1. Problem restatement

The first hierarchy experiment produced an important combination:

```text
one stable child AtomId
+
one causal parent-location register
```

This avoids duplicate children under concurrent cross-container moves.

However, independently materialized parent registers can still form an invalid active graph:

```text
A.parent = B
B.parent = A
```

Every individual register converges. The hierarchy does not.

The next experiment therefore asks whether a deterministic validity layer can preserve the stable-location model without reintroducing remove+insert transactions.

## 2. First candidate: reject one active move and fall back

The test-only resolver operates on the complete retained parent-location revision sets.

Conceptually:

```text
1. materialize every parent register normally
2. detect an active parent cycle
3. select one active placement revision in that cycle by a deterministic opaque-ID tie-break
4. reject that placement as globally invalid
5. allow that atom's next historical placement to materialize
6. repeat until the parent graph is acyclic
```

The current experiment rejects the lowest canonical `RevisionId` among active cycle edges.

This choice is not claimed to be uniquely correct. Its purpose is to test whether the general *reject-invalid-move + historical fallback* shape can converge without clocks.

The revision ID is not interpreted as time. It is only a deterministic tie-break inside a set of simultaneously invalid active edges.

## 3. Two-node cross-move result

Starting from two root atoms:

```text
A
B
```

concurrent replicas create:

```text
A -> parent B   revision 800
B -> parent A   revision 900
```

The raw merged hierarchy is cyclic.

The validity candidate rejects revision `800`, allowing A's previous root placement to become active again:

```text
A
└── B
```

The resulting graph is valid and deterministic.

## 4. Three-node cycle result

The suite also creates:

```text
A.parent = B   id 900
B.parent = C   id 800
C.parent = A   id 700
```

The resolver rejects `700`, restoring C's previous root placement:

```text
C
└── B
    └── A
```

Only one active move is rejected. The other valid concurrent moves remain effective.

This is preferable to discarding the entire concurrent move set merely because one cycle exists.

## 5. Merge-order convergence

A four-replica experiment includes a three-node cycle plus an unrelated fourth move.

The replica states are merged in twenty randomized orders.

After deterministic validity resolution every run produces the same:

- active parent assignment;
- rejected revision set.

Thus the tested candidate does not make transport arrival or Git merge order semantic.

## 6. Causal successor still beats opaque ID magnitude

The suite then starts from a cycle where move `800` is rejected.

A later placement of the same atom causally observes/supersedes that move but intentionally receives opaque revision ID `50`.

The new placement becomes the register's active causal successor despite its smaller ID. It removes the cycle and the validity resolver rejects nothing.

Therefore the candidate still preserves the core rule:

```text
causal successor precedence
>
ID tie-break
```

ID magnitude is not turned into a hidden logical clock.

## 7. The cost: historical placement becomes semantic fallback state

The candidate only works because the rejected move's earlier placement still exists.

For example:

```text
A root @ R100
A -> B @ R800
```

if R800 is rejected due to a merged cycle, R100 becomes active again.

If compaction had deleted R100 with no replacement proof or accepted-fallback witness, the resolver would no longer know where A should exist.

This exposes another compaction constraint:

> historical location state may remain semantically relevant even after being causally superseded if a later placement can become globally invalid under merged tree constraints.

That is stronger than ordinary scalar causality, where a causally dominated value can normally be forgotten once all required merge evidence is compacted safely.

## 8. Why this candidate is not yet accepted

Several hard questions remain.

First, retaining enough historical placements for arbitrary future fallback may reintroduce long-term metadata growth.

Second, larger graphs can contain several overlapping cycles. Rejecting one edge may expose an older placement that forms a different cycle, so the resolver may need multiple rounds.

Third, a future move may have been created after observing a complex tree state even though its own parent-location domain does not encode every cross-domain tree observation. Tree-validity context may therefore need its own witness or proof rather than pretending all parent registers are semantically independent.

Fourth, the selected tie-break policy affects which user's concurrent move is discarded. It must be stable and understandable enough to justify as semantic policy, not merely convenient for code.

Fifth, production authentication/finalization must define whether a placement may be authenticated yet later treated as globally invalid. Rejection is not forgery; it is validity resolution over individually authentic concurrent statements.

## 9. Interaction with delete

The current resolver operates over structural parent revisions independently of lifecycle filtering.

That is deliberately conservative: even a currently hidden subtree retains a structurally valid parent graph.

A future optimization might avoid resolving cycles entirely inside permanently deleted/compacted subtrees, but that cannot be assumed while restore, stale descendants and deletion stability remain unresolved.

## 10. Current decision

After this pass:

1. Stable per-child parent-location remains a viable containment direction despite the raw cycle counterexample.
2. A deterministic state-derived validity layer can resolve tested 2-node and 3-node move cycles without clocks or transport order.
3. Rejecting one invalid active placement and falling back to a historical placement preserves more concurrent intent than rejecting every move in the component.
4. Randomized merge order produces the same resolved hierarchy in the tested cases.
5. Causal successor semantics still dominate opaque-ID order inside one parent register.
6. Historical placements become potentially semantic fallback state, creating a new compaction burden.
7. The current lowest-ID rejection policy is a research tie-break, not frozen user semantics.
8. Tree validity may require additional cross-domain validity context even if ordinary content causality remains domain-local.

## 11. Next experiments

The next pass should stress this candidate with:

- random trees with hundreds or thousands of concurrent cross-container moves;
- multiple simultaneous and overlapping cycles;
- repeated fallback where one rejected move reveals another invalid historical placement;
- stale offline branches that reference a placement compacted on another replica;
- delete/restore candidates intersecting cycle resolution;
- ordered position within parent containers;
- comparison with a validity model that retains an explicit last-known-valid placement witness instead of arbitrary history;
- metadata growth under years of moves if historical fallback remains required;
- proof that deterministic rejection remains associative/idempotent when validity state itself is compacted.

This candidate should remain a falsification target until those tests pass.
