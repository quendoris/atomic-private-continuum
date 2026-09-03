# A.P.C. ordered sequence — adversarial round 2

Status: active research.

This document records the second adversarial pass over the current ordered-sequence candidate. It supplements `SEQUENCE_RESEARCH.md` and does not freeze a portable format.

The executable cases are in `tests/test_sequence_adversarial.py`.

## 1. What survived

The current composition remains convergent under substantially less friendly cases than the initial examples:

```text
stable AtomId
      |
causal location register
      |
immutable PositionId
      |
Fugue-style position tree
```

The test suite now includes:

- crossing concurrent moves of different atoms;
- concurrent moves and deletes across offline replicas;
- 8 replicas performing 30 independently generated move/delete operations each;
- 20 randomized orders for merging those replica states;
- repeated moves of one atom for metadata accounting;
- adversarial append-only tree growth.

For the randomized multi-replica experiment, all merge orders produce the same complete reference state and the same visible order.

This supports the convergence claim for the current reference composition. It does **not** establish that every user-visible conflict policy is desirable.

## 2. New counterexample: an insertion does not follow a concurrently moved anchor

Starting state:

```text
A
B
C
```

Replica X moves `B` after `C`:

```text
A
C
B
```

Replica Y, while still seeing the original state, inserts `D` between `B` and `C`:

```text
A
B
D
C
```

The current stable-position model merges to:

```text
A
D
C
B
```

`D` remains next to the historical position of `B`; it does not follow the winning moved location of `B`.

This is not a convergence failure. All replicas agree on the same result. It is a semantic question exposed by separating `AtomId` from immutable `PositionId`.

A move changes which position is active for an atom. It does not rewrite positions that were concurrently created relative to the atom's old position.

The behavior is closely related to the context problem discussed for range moves in Martin Kleppmann, *Moving Elements in List CRDTs* (PaPoC 2020): edits anchored to old positions do not automatically move with the surrounding content.

The A.P.C. block model reduces this problem for child content that is structurally inside a moved container, because the child collection belongs to that container. It does not eliminate the problem for independently ordered neighboring blocks.

### Open policy question

A.P.C. must decide whether an insertion created relative to an atom should mean:

1. **historical-position anchoring** — remain where the observed position was;
2. **identity-following anchoring** — attempt to remain adjacent to the atom if that atom moves concurrently;
3. a more explicit context model that distinguishes these intents.

Identity-following is not automatically better. Following mutable identities can introduce new cycles or surprising movement when several anchors move independently. It requires its own model and adversarial tests before consideration.

## 3. Deleted anchors behave differently

The same immutable-position behavior is useful when the anchor is concurrently deleted.

Starting state:

```text
A
B
C
```

Replica X deletes `B`.
Replica Y inserts `D` between `B` and `C`.

Under the experimental delete-wins lifecycle model, the merged visible result is:

```text
A
D
C
```

The hidden historical position of `B` still provides stable ordering context for `D` without resurrecting `B`.

This is evidence that structural position metadata and visible atom lifecycle should not be conflated.

## 4. `location = None` is rejected as a final deletion model

The first movable-sequence experiment encoded deletion as a causal assignment:

```text
location(atom) = None
```

That is sufficient to study convergence, but it has an undesirable consequence.

A later ordinary placement of the same atom is simply a causal successor of the `None` revision and therefore makes the atom visible again:

```text
position P0
   -> None
   -> position P1
```

The executable counterexample confirms this behavior.

Therefore **location and lifecycle are separate merge domains**.

An ordinary move changes location. It must not implicitly redefine whether the atom is logically alive.

The research harness now includes `DeleteWinsSequenceLab`, which keeps location metadata unchanged and maintains deletion in a separate grow-only set. In that candidate:

- concurrent move vs delete leaves the atom deleted;
- revision-ID ordering cannot accidentally make a move resurrect it;
- an ordinary local move of an already deleted atom is rejected.

This candidate is deliberately not normative. A grow-only deletion set cannot express an explicit restore of the same `AtomId`, and safe tombstone compaction is still unresolved.

## 5. Crossing moves

The suite now contains a case in which different replicas move different atoms across overlapping parts of the list.

The immutable position tree does not develop an ordering cycle because moves allocate fresh positions rather than mutating existing position relations.

The merged state:

- converges in either merge order;
- contains every non-deleted atom exactly once;
- does not duplicate moved atoms.

This is a useful property of separating identity from position, but the exact user-visible winner among incompatible spatial intentions remains a semantic matter rather than a convergence matter.

## 6. Metadata result: explicit causal ancestry is quadratic

The current `ScalarRegister` deliberately stores full observed revision-ID sets because that makes causality easy to inspect.

The adversarial measurement makes the cost explicit.

With three atoms and 128 repeated moves of one atom, the reference model contains:

```text
positions                 131
visible atoms                3
inactive old positions     128
location revisions         131
explicit causal ID links  8256
```

The `8256` links are:

```text
1 + 2 + ... + 128
```

because every new location revision explicitly records every prior revision of that merge unit.

This confirms that the current full-ancestor-set representation is suitable only as a correctness oracle. It is rejected as a production causal encoding.

The production model still needs an ID-based compact causal proof/summary that preserves the required partial order without trusting device or server clocks.

## 7. Metadata result: naive position-tree depth can be linear

A second executable measurement appends 256 positions sequentially to one end of a Fugue-style tree.

The resulting maximum structural depth is 256.

Thus the literal reference tree can form a linear-depth path under ordinary append behavior. A naive comparator that repeatedly walks parent chains cannot be accepted as the production indexing strategy at A.P.C. scale.

This does **not** by itself reject Fugue semantics. Fugue's tree representation has constant-size position records but retains the global position tree, and practical implementations use additional optimizations. Published descriptions discuss batching sequential insertion runs and indexing/waypoint-style optimizations.

For A.P.C. the distinction is important:

```text
ordering semantics != physical index implementation
```

We may retain a semantic ordering relation while using a substantially different indexed representation inside `.apc`.

## 8. Current decisions after round 2

The research status is now:

1. The first topological constraint graph remains rejected as a production sequence because it permits avoidable run interleaving.
2. Fugue/FugueMax-style immutable position semantics remain the strongest current ordering candidate.
3. Stable `AtomId`, immutable `PositionId`, and position/location `RevisionId` remain useful separate concepts.
4. Single-element move through a causal location register remains viable for convergence and duplicate avoidance.
5. Deletion MUST NOT be modeled merely as `location = None` in the final logical model.
6. Lifecycle and location must be independently modeled.
7. Full explicit ancestor sets are rejected as a production causal encoding because their retained references grow quadratically under repeated edits.
8. The naive parent-walking position-tree implementation is a reference oracle, not a production index.
9. Concurrent insertion relative to a moved anchor is now an explicit unresolved semantic problem.

## 9. Next experiments

The next research pass should focus on the newly exposed boundaries rather than adding unrelated features:

1. model and compare historical-position, identity-following and context-aware anchor semantics;
2. generate adversarial moved-anchor graphs, including both anchors moving concurrently in opposite directions;
3. test a closer FugueMax ordering model against the current opaque-ID Fugue-style sibling rule;
4. compare at least one RGA-family and one variable-position/LSEQ-family candidate on the same non-interleaving and metadata tests;
5. prototype compact causal metadata while keeping wall-clock time completely outside correctness;
6. analyze lifecycle policies and safe compaction for deleted atoms and inactive positions;
7. separate semantic tree depth from production lookup/index cost and measure optimized representations at much larger scales.

## References

- Matthew Weidner and Martin Kleppmann, *The Art of the Fugue: Minimizing Interleaving in Collaborative Text Editing*, arXiv:2305.00583; IEEE TPDS 36(11), 2025.
- Matthew Weidner, *Fugue: A Basic List CRDT*, 2022.
- Martin Kleppmann, *Moving Elements in List CRDTs*, PaPoC 2020, DOI `10.1145/3380787.3393677`.
- Brice Nedelec, Pascal Molli, Achour Mostefaoui, Emmanuel Desmontils, *LSEQ: an Adaptive Structure for Sequences in Distributed Collaborative Editing*, DocEng 2013, DOI `10.1145/2494266.2494278`.
