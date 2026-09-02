# A.P.C. ordered sequence research

Status: active research. This document records candidate behavior and rejected assumptions. It is not a frozen format specification.

## 1. Requirements under test

The primary A.P.C. ordered collection must support:

- stable element identity independent of position;
- insertion without a central allocator or wall-clock ordering;
- deterministic convergence under arbitrary replica merge order;
- preservation of concurrent independent insertions;
- long offline periods;
- a future single-element move operation without duplicating the moved atom;
- deletion without accidental resurrection;
- large replica counts;
- acceptable metadata growth.

Convergence alone is insufficient. User-visible ordering quality also matters.

## 2. Counterexample to the first constraint-graph model

The first reference model represented an insertion by constraints against the local left and right atom IDs and materialized their union with a deterministic topological sort.

That model converges, but it can interleave two independently inserted runs.

Starting state:

```text
A
B
```

Replica X inserts, locally and in order:

```text
X1
X2
```

between A and B.

Replica Y concurrently inserts:

```text
Y1
Y2
```

between the same A and B.

The constraint union can legally materialize:

```text
A
X1
Y1
X2
Y2
B
```

even though neither user produced an interleaved run.

This is now an executable regression/counterexample in `tests/test_sequence_lab.py`.

The constraint-graph candidate therefore remains useful as a minimal convergence model but is rejected as the production sequence structure.

## 3. Why non-interleaving is a requirement

The interleaving problem is documented in replicated-list research. Weidner and Kleppmann show that many list CRDT and OT algorithms can interleave concurrent insertion runs and define a stronger property, maximal non-interleaving.

For A.P.C. this matters even though the first editor is block-oriented rather than character-oriented. Two users may concurrently paste multiple lines, create several checklist items, or insert several blocks into one gap. The result should not arbitrarily weave those independent runs together when a deterministic non-interleaving result is possible.

References:

- Matthew Weidner and Martin Kleppmann, *The Art of the Fugue: Minimizing Interleaving in Collaborative Text Editing*, IEEE TPDS 36(11), 2025. DOI: `10.1109/TPDS.2025.3611880`.
- Preprint: `arXiv:2305.00583`.

A.P.C. has not yet adopted FugueMax as a normative algorithm, but non-interleaving is now an explicit selection criterion.

## 4. Candidate families

### 4.1 RGA-style fixed identity

RGA-family structures use stable insertion identities and predecessor/ancestry relations. They are conceptually simple and are widely used as the basis of practical replicated lists.

Advantages for A.P.C.:

- stable identities;
- natural state-union formulations;
- straightforward insert/delete semantics;
- no position renumbering.

Costs:

- deleted/obsolete structural metadata may remain as tombstones;
- ordering quality under concurrent insertion runs must be evaluated rather than assumed;
- move is not a native operation in classic RGA.

Reference:

- H.-G. Roh, M. Jeon, J.-S. Kim, J. Lee, *Replicated abstract data types: Building blocks for collaborative applications*, JPDC 71(3), 2011.

### 4.2 Logoot/LSEQ-style variable position identifiers

Logoot-family structures encode list order into dense position identifiers. LSEQ changes allocation strategy to reduce pathological identifier growth.

Advantages:

- positions can be compared without retaining a full tombstone tree;
- insertion naturally creates a stable position between two positions.

Costs:

- identifiers are variable-sized and may grow under difficult editing patterns;
- allocation strategy affects behavior and metadata size;
- non-interleaving quality must still be evaluated;
- move still requires an additional semantic layer.

Reference:

- B. Nédelec et al., *LSEQ: an Adaptive Structure for Sequences in Distributed Collaborative Editing*, 2013.

### 4.3 Fugue/FugueMax-style position tree

Fugue represents immutable positions in a tree. A new position records a parent position and whether it is a left or right child; a deterministic tree walk defines total order. The published Fugue work focuses specifically on avoiding undesirable interleaving.

The research implementation in `reference_model/sequence_lab.py` contains a small **Fugue-style** position tree. It is not claimed to be a conforming implementation of Fugue or FugueMax because A.P.C. currently uses opaque canonical IDs as the sibling tie-break rather than freezing the paper's causal-dot encoding.

Current experimental result:

- two concurrent multi-element insertion runs remain contiguous in the tested scenario;
- random insertion sequences preserve the exact local order;
- state union converges under random merge order.

The candidate remains open pending broader adversarial testing and metadata measurements.

References:

- Matthew Weidner, *Fugue: A Basic List CRDT*, 2022.
- Weidner and Kleppmann, *The Art of the Fugue*, 2025.

## 5. Single-element move

Move must not be implemented as semantic delete plus independent reinsert of the same atom. Concurrent delete/reinsert moves can duplicate one logical element.

Kleppmann's 2020 list-move construction separates element identity from position and stores the element's current stable position in a replicated register. Concurrent moves of one element select one position as winner. The paper notes that the winner may be selected by any deterministic rule; A.P.C. does not need a wall-clock timestamp for this.

A.P.C. can compose this idea with its existing causal scalar register:

```text
AtomId
  |
  +-- location register
        |
        +-- immutable PositionId
```

A move:

1. allocates a fresh immutable position in the underlying sequence-position structure;
2. creates a new causal revision of the atom's location register pointing to that position;
3. leaves the atom identity unchanged.

If two replicas concurrently move the same atom, the existing A.P.C. scalar rule selects one visible location:

- causal successor wins when one move observed the other;
- genuinely concurrent moves use the canonical opaque `RevisionId` tie-break.

The losing position may remain as structural metadata, but the atom materializes once.

This composition is implemented experimentally by `MovableSequenceLab`.

Reference:

- Martin Kleppmann, *Moving Elements in List CRDTs*, PaPoC 2020. DOI: `10.1145/3380787.3393677`.

## 6. Stable positions are different from atom identities

A move target cannot be a mutable integer index.

It must resolve to stable sequence context. The research model therefore distinguishes:

```text
AtomId      stable user-information identity
PositionId  stable immutable sequence position
RevisionId  identity of a location-register revision
```

Moving an atom creates a new `PositionId`; it does not create a new `AtomId`.

This also means that a destination is interpreted relative to stable positions that existed when the move was created. Later movement of a neighboring atom does not rewrite historical position structure.

## 7. Delete

The lab currently models deletion as a causal location-register assignment to `None`.

This provides useful experiments:

- a causal delete after a move hides the atom;
- duplicate delivery is naturally harmless;
- a concurrent delete and move converges deterministically through the scalar register.

However, **the concurrent delete-vs-move/edit policy is not accepted as final**. Deterministic convergence does not by itself prove that the user-visible policy is correct.

Before deletion becomes normative we still need to decide:

- delete-wins, move-wins, or context-dependent behavior for true concurrency;
- how deletion of a container interacts with edits to descendants;
- when obsolete location/position metadata may be safely compacted;
- how a very old offline replica is prevented from resurrecting deleted information.

## 8. Move ranges

The 2020 list-move work provides a single-element move and explicitly identifies moving contiguous ranges as a harder problem.

A.P.C. reduces the immediate pressure on range moves because the initial editor is block-atomic:

- a line/list item is an atom;
- moving one block is one element move;
- editing text inside that block is a separate merge domain.

This does not eliminate the future need for range moves. Multi-selection drag, block grouping, or richer collaborative text may require an atomic range/subtree move. That is not part of the initial ordered-collection contract.

More recent JSON CRDT work demonstrates that moves interacting with ordered lists, maps, deletion and tree cycles require substantially more validity logic. That work is useful evidence and a source of adversarial cases, but its operation-set/Lamport representation is not directly adopted by A.P.C.'s state-based portable model.

Reference:

- Liangrun Da and Martin Kleppmann, *Extending JSON CRDTs with Move Operations*, PaPoC 2024. `arXiv:2311.14007`.

## 9. Metadata cost

The candidates make different trade-offs.

The current Fugue-style tree keeps immutable position nodes, including positions that are no longer active after move/delete. This makes merge simple but means structural metadata can grow with editing history.

The reference implementation intentionally does not hide this cost.

A production candidate must measure at least:

- bytes per inserted block;
- bytes retained per deleted block;
- bytes retained per move;
- maximum/average tree depth;
- compare/materialization cost;
- cost after long offline divergence;
- behavior at millions of blocks and thousands of replicas.

Compaction cannot be added by deleting metadata until there is a proof that doing so cannot change a future valid merge.

## 10. Current decision

The first topological constraint sequence has been falsified as a production candidate because it permits avoidable run interleaving.

The current research direction is:

```text
stable AtomId
      |
causal location register
      |
immutable sequence PositionId
      |
Fugue/FugueMax-style ordering candidate
```

This is not a format freeze.

The next experiments are:

1. broader non-interleaving adversarial generation;
2. randomized move/delete concurrency;
3. metadata-growth measurement;
4. comparison of a Fugue-style tree against at least one RGA-style and one variable-position candidate;
5. lifecycle/compaction analysis for old positions and deleted atoms.
