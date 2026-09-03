# A.P.C. compact causality research

Status: active research. This document records executable causal-metadata experiments. It does not freeze the portable format.

Executable cases live in `tests/test_causality_lab.py` and use `reference_model/causality_lab.py`. Causal-node compaction experiments continue in `CHECKPOINT_RESEARCH.md`, `tests/test_checkpoint_lab.py` and `reference_model/checkpoint_lab.py`.

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

## 9. Causal-node compaction result

The first executable checkpoint pass is documented in `CHECKPOINT_RESEARCH.md`.

It establishes three useful boundaries for the current ID-only model:

1. dominated historical causal node bodies can be removed from the hot DAG while exact covered-ID membership still permits tested long-offline branches to reconnect;
2. the logical frontier revision IDs themselves cannot be replaced by arbitrary fresh checkpoint IDs because that can change deterministic concurrent scalar winners;
3. if both old nodes and all knowledge that their IDs were covered are discarded, a returning branch with an old parent becomes unverifiable and must be rejected rather than ordered by guesswork.

The experiment therefore moves the open problem from generic "delete old DAG nodes" to a more precise question:

> how should A.P.C. preserve or prove historical causal membership without forcing the hot representation to grow with every lifetime revision?

The exact-coverage oracle still stores one opaque membership ID per compacted historical revision, so total exact coverage metadata remains linear.

## 10. Relation to user history

The direct causal DAG and checkpoint coverage are merge metadata, not a user-facing edit log.

They exist only because future merge correctness may require evidence that one revision observed another or that an old baseline belonged to an already incorporated history.

A.P.C. still does not require preservation of every intermediate user-visible state, keystroke or GitHub transport commit.

Local crash durability is also distinct from portable causal-revision creation. A future experiment will test whether many durable local edits can safely collapse into one portable causal revision at an observation/publication boundary.

If causal nodes or historical membership can later be safely summarized without changing any valid merge result, they should be compacted.

## 11. Current decision

The research status after the causality and first checkpoint passes is:

1. Full explicit ancestor sets remain the correctness oracle and remain rejected for production storage.
2. Direct-frontier parent references preserve the tested scalar causal semantics while reducing linear-history reference growth from quadratic to linear.
3. The candidate uses IDs only; no trusted clock or logical counter participates in correctness.
4. Wide concurrent frontiers remain a cost and are now measurable explicitly.
5. Baseline-aware incremental sync can send only missing causal nodes rather than the complete register state.
6. A missing causal baseline must cause buffering/rebootstrap/retrieval, never an inferred ordering decision.
7. Hot causal node bodies can be compacted more aggressively than the first direct-frontier model suggested, but exact arbitrary-old-ID membership still has a retained cost.
8. Checkpoint identity MUST NOT silently replace logical revision identity.
9. Safe long-term historical-membership compaction is now the primary causality research problem.

## 12. Next experiments

The next pass should compare explicit alternatives for that membership problem rather than invent another generic checkpoint object:

- exact historical membership kept in a cold index outside the hot DAG;
- authenticated set commitments plus membership proofs, without selecting a cryptographic accumulator prematurely;
- retained transport/causal generations with an explicit stale-baseline horizon and safe rebootstrap path;
- causal-revision coalescing so frequent local durable edits do not automatically create equal numbers of portable causal nodes;
- duplicate and out-of-order compact deltas across several generations;
- interaction between compaction and lifecycle tombstones;
- whether one causal summary can safely cover several independent merge domains or must remain domain-local;
- capsule size and merge cost before and after each compaction strategy.

Sequence/moved-anchor research continues independently in parallel.
