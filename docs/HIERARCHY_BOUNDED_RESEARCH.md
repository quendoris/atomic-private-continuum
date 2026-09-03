# A.P.C. bounded hierarchy fallback research

Status: active research. This document compares bounded validity fallbacks against the earlier full-history hierarchy resolver. It does not freeze hierarchy semantics or compaction.

Executable cases live in `tests/test_hierarchy_bounded_lab.py` and use `reference_model/hierarchy_bounded_lab.py`.

## 1. Starting point

The full-history resolver preserves old placement intent by repeatedly falling back through retained placement revisions when a current parent assignment participates in a merged cycle.

The hierarchy torture experiment constructed a valid adversarial state with one initially active cycle and fallback depth 64. The general cost can grow with retained move history.

The next question is therefore not whether fallback can be bounded. It can. The question is how much user placement intent must be sacrificed for that bound.

Three policies are now compared:

```text
full history
    invalid current placement
    -> previous placement revision
    -> previous placement revision
    -> ...

one witness
    invalid current placement
    -> one stored previous-known parent
    -> root if still invalid

root fallback
    invalid current placement
    -> root
```

The bounded policies are research candidates only.

## 2. Root fallback

The simplest candidate does not reactivate historical placement revisions.

When an active cycle is detected, the same deterministic opaque-ID rule selects one losing current placement edge. That atom is placed at root for the resolved state.

Because root has no parent edge, one atom cannot require another historical fallback step in the same resolved input state.

The policy therefore removes the unbounded per-atom historical walk.

Its cost is semantic: it may discard a perfectly useful immediately previous parent even when that previous placement would be valid in the merged graph.

## 3. One-witness fallback

The second candidate attaches one bounded validity witness to a move:

```text
current desired parent
+
previous parent observed before this move
```

The witness is not treated as a historical placement revision becoming active again. It is bounded fallback metadata associated with the current move.

If the current placement is rejected:

1. try the witness parent;
2. if the resulting state still requires this atom to lose a cycle, place it at root.

The executable model bounds one atom to at most two fallback steps for one resolved state:

```text
current -> witness -> root
```

The helper `previous_parent_witnesses()` derives these witnesses from the explicit-history oracle only for comparison. A production version would have to store/authenticate the bounded witness when the move is created; it cannot depend on reconstructing arbitrary old history after compaction.

## 4. Deep-history counterexample revisited

The earlier depth-64 adversarial state is reused.

Full-history policy:

```text
resolution iterations       64
max fallback depth          64
```

One-witness policy reaches the same final root placement for the attacked atom with at most:

```text
resolution iterations        2
max fallback steps/atom      2
```

Root fallback reaches the same final root placement with at most:

```text
resolution iterations        1
max fallback steps/atom      1
```

This does not prove a global O(1) hierarchy resolver: several different atoms/cycles may still require work. It does prove that the specific unbounded historical walk for one atom is removed by these bounded policies.

## 5. Measured intent preservation

A smaller scenario distinguishes the bounded policies.

One atom B has a causal move history:

```text
B under P
B -> Q
B -> R
```

A concurrent move makes only `B -> R` invalid by creating a cycle through R.

The policies resolve to:

```text
full history  -> Q
one witness   -> Q
root fallback -> root
```

Thus one witness preserves the immediately previous parent when that parent remains valid, while direct root fallback loses that intent.

A stronger counterexample then makes both R and Q invalid after merge.

The policies resolve to:

```text
full history -> P
one witness  -> root
```

This is the explicit tradeoff. One-witness metadata is bounded because it deliberately refuses to walk arbitrarily deep history. Consequently it cannot preserve all placement intent that the full-history oracle preserves.

## 6. Convergence

The tests merge sparse random branches plus guaranteed cross-move cycles in multiple randomized replica orders.

Both bounded policies produce the same resolved parent graph and rejected-current-revision set across the tested merge orders.

As before, the tie-break uses opaque canonical IDs only after causal register materialization. ID magnitude is not interpreted as recency or causal time.

## 7. Compaction consequence

The bounded policies separate two previously entangled costs.

Full-history validity semantics required arbitrary historical placement bodies to remain available because any one of them might become active later.

One-witness validity semantics require only bounded fallback information for the currently relevant move, subject to whatever causal evidence is independently required by the merge model.

Therefore:

```text
causal-history retention
!=
hierarchy-validity fallback retention
```

This is important. Solving bounded validity fallback does not by itself solve compact ID-based causality, offline membership proofs or authentication history. It removes one reason for keeping arbitrary old placement values.

## 8. Authentication consequence

If a previous-parent witness becomes part of portable hierarchy semantics, it must be covered by the immutable authenticated statement for that placement.

Otherwise a transport or replica could substitute a different fallback parent after the move was finalized.

The likely statement shape would therefore eventually bind at least:

```text
placement RevisionId
AtomId
requested ParentId
bounded fallback witness, if used
causal parent context
```

The exact cryptographic representation remains open.

## 9. Current result

After this pass:

1. Full-history fallback remains the strongest tested intent-preservation oracle but has unbounded history-dependent fallback depth.
2. Direct root fallback bounds one atom to one validity fallback step but can unnecessarily discard an immediately previous valid parent.
3. One-witness fallback bounds one atom to at most `current -> witness -> root` in the tested candidate.
4. One witness preserves the immediately previous valid parent in the tested single-invalid-move case.
5. One witness intentionally loses deeper placement history when both current and witness parents become invalid.
6. Both bounded candidates remain merge-order independent in the tested randomized state.
7. Bounded hierarchy-validity metadata and compact causal metadata are separate research problems.
8. If adopted, a validity witness must be immutable/authenticated portable semantics rather than an unauthenticated local hint.

## 10. Next experiments

The next useful work is statistical rather than another tiny hand-written example:

- compare full-history, one-witness and root policies over large random/offline branch storms;
- measure how often one-witness and root differ from full-history placement results;
- measure cycle frequency as a function of tree size and concurrent move density;
- measure conflict-component size and number of atoms forced to root;
- separate benign random workloads from adversarial cycle farms;
- test deletion and hidden ancestors with bounded fallback;
- test long-offline replicas whose witness parent was later deleted;
- test ordered position inside the selected parent after parent validity is resolved.

A local benchmark runner is appropriate for the larger statistical passes; the semantic unit tests should remain small enough for normal CI.
