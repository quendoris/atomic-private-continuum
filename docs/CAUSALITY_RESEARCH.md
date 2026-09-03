# A.P.C. compact causality research

Status: active research. This document records executable causal-metadata experiments. It does not freeze the portable format.

Executable cases live in `tests/test_causality_lab.py` and use `reference_model/causality_lab.py`.

## 1. Constraint

A.P.C. semantic causality must not depend on wall-clock time, GitHub timestamps, server order, Lamport timestamps or a globally meaningful numeric sequence.

The current research direction therefore keeps causal identity ID-based.

A revision ID identifies a causal node. Its lexical/numeric value does not mean earlier or later. The canonical ID comparison is used only as the deterministic winner for genuinely concurrent scalar frontier values.

This also means that per-replica counters, version vectors and dotted version vectors are not adopted as the primary A.P.C. causal representation at this stage. They remain useful comparison families, but their counter semantics do not match the current ID-only design constraint.

## 2. Explicit-ancestor oracle

The original `ScalarRegister` records every observed revision ID in every new revision.

For a linear history:

```text
R1 -> R2 -> R3 -> ... -> Rn
```

revision `Rn` stores every prior ID.

The total retained causal references are therefore:

```text
0 + 1 + 2 + ... + (n - 1)
= n(n - 1) / 2
```

This representation is intentionally easy to reason about and remains the correctness oracle. It is already rejected as a production representation because retained references are quadratic.

## 3. Direct-frontier DAG candidate

The first compact ID-only candidate stores only the causal frontier observed when a revision is created.

Example:

```text
R1
 |
R2
 |
R3
```

is stored as:

```text
R1.parents = {}
R2.parents = {R1}
R3.parents = {R2}
```

not:

```text
R3.parents = {R1, R2}
```

If two replicas diverge:

```text
      A
     /
base
     \
      B
```

the merged frontier is `{A, B}`.

A later revision that has observed both records:

```text
C.parents = {A, B}
```

After that join, the frontier is `{C}` and the following ordinary revision needs only `{C}` as its direct causal parent.

No counter or clock is required.

## 4. Oracle equivalence result

The test suite creates the same edit/merge history in both:

- the explicit-all-ancestors `ScalarRegister` oracle;
- the direct-frontier `FrontierCausalRegister` candidate.

The tested properties match:

- visible scalar winner;
- causal frontier IDs;
- causal successor precedence even when its ID sorts below its ancestor;
- genuine concurrency detection;
- stale-state merge behavior;
- randomized replica merge histories.

A randomized experiment currently performs 1,200 assign/merge steps across 12 replicas and checks oracle equivalence continuously, not only at the final state.

This is evidence for the direct-frontier representation as a correct compression of the oracle for the tested scalar semantics. It is not yet a general proof for every future merge domain.

## 5. Linear metadata result

For 256 sequential revisions the executable test measures:

```text
explicit ancestor references   32640
direct frontier references       255
```

The candidate therefore changes the retained-reference growth for a simple causal chain from quadratic to linear.

The register still retains 256 causal nodes. This experiment compresses edges, not yet historical nodes.

## 6. Concurrency cost

The candidate's immediate parent cost follows unresolved causal frontier width.

The suite creates 512 concurrent branches from one base revision. Before a join, the merged frontier contains 512 IDs.

The first revision created after observing all branches therefore has 512 direct parents:

```text
A1  A2  A3 ... A512
 \   |   |      /
       JOIN
```

After that join, the next ordinary revision again has one parent.

This is an important trade-off:

```text
cost tracks unresolved concurrency
rather than total lifetime history
```

The experiment does not claim that a 512-parent node is the final production encoding. A deterministic ID-only join/checkpoint witness may compress wide frontiers further, but such a mechanism needs its own proof and lifecycle rules.

## 7. Stale replicas do not roll state backward

A register snapshot taken at revision 20 is merged into the same causal chain after revision 79.

The visible result remains revision 79 in either merge direction.

This follows from causal reachability, not arrival order. A very old replica can therefore reappear without obtaining authority to overwrite later causal state merely because it synchronized last.

This test does not solve tombstone compaction or transport-generation expiry. Those remain separate questions.

## 8. Baseline-aware sync delta

The direct-parent DAG also allows a transport projection to send only causal nodes missing from a receiver that already has a valid baseline.

The current experiment uses:

```text
sender:   R1 ... R100
receiver: R1 ... R90
```

The generated delta contains only:

```text
R91 ... R100
```

with `R90` declared as an external parent dependency.

The receiver can apply the delta because it already possesses `R90`.

If the same delta is delivered to a receiver without the required baseline, the reference model rejects it instead of guessing causal order.

This gives the sync layer a clean rule:

> a compact capsule may reference already-known causal IDs, but semantic application requires every referenced dependency to be locally available or covered by a trusted retained checkpoint.

A future sync inbox may buffer such a capsule until dependencies arrive. Transport arrival order still does not become semantic order.

## 9. What this does not solve

The current candidate retains the complete direct-parent DAG forever. Therefore total causal-node count still grows with edit history.

That is better than quadratic ancestor sets but it is still not the desired final state for a continuum edited for years.

The remaining problem is safe causal compaction.

Compaction cannot simply delete old nodes because a long-offline replica may later present state whose relationship to the retained frontier depends on those nodes.

A production solution needs a way to replace old causal subgraphs with a compact statement that preserves the comparisons future valid merges require.

Candidate directions include deterministic ID-only join/checkpoint witnesses and retained sync generations. Neither is accepted yet.

## 10. Relation to user history

The direct causal DAG is merge metadata, not a user-facing edit log.

It exists only because future merge correctness may require evidence that one revision observed another.

A.P.C. still does not require preservation of every intermediate user-visible state, keystroke or GitHub transport commit.

If causal nodes can later be safely summarized without changing any valid merge result, they should be compacted.

## 11. Current decision

The research status after this pass is:

1. Full explicit ancestor sets remain the correctness oracle and remain rejected for production storage.
2. Direct-frontier parent references preserve the tested scalar causal semantics while reducing linear-history reference growth from quadratic to linear.
3. The candidate uses IDs only; no trusted clock or logical counter participates in correctness.
4. Wide concurrent frontiers remain a cost and are now measurable explicitly.
5. Baseline-aware incremental sync can send only missing causal nodes rather than the complete register state.
6. A missing causal baseline must cause buffering/rebootstrap/retrieval, never an inferred ordering decision.
7. Safe causal-node compaction remains unresolved and is now the primary causality research problem.

## 12. Next experiments

The next pass should attack:

- deterministic ID-only causal join/checkpoint witnesses;
- safe replacement of old direct-parent subgraphs while one replica remains offline;
- duplicate and out-of-order compact deltas across several sync generations;
- concurrent frontier widths in the thousands without quadratic merge implementation artifacts;
- interaction between compact causality and lifecycle tombstones;
- whether one causal summary can safely cover several independent merge domains or whether summaries must remain domain-local;
- capsule size and merge cost before and after compaction.

Sequence/moved-anchor research continues independently in parallel.
