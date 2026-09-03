# A.P.C. hierarchy torture research

Status: active research. This document records stress measurements for the current deterministic hierarchy cycle-resolution candidate. It does not freeze the hierarchy algorithm, cycle policy or compaction model.

Executable cases live in `tests/test_hierarchy_torture_lab.py` and use `reference_model/hierarchy_torture_lab.py`.

## 1. Purpose

The current containment candidate separates stable atom identity from parent location:

```text
Child AtomId
    |
causal parent-location register
    |
current ParentId
```

Concurrent cross-container moves of one child therefore do not duplicate that child. A separate validity layer resolves parent cycles by rejecting one active placement revision and falling back to retained historical placement state.

The previous small examples established convergence on two- and three-node cycles. The unresolved question was the cost of that fallback shape under larger and deliberately adversarial states.

The torture harness records:

```text
atom count
placement revisions retained
historical placement revisions
initial active cycle count
resolution iterations
rejected placement revisions
maximum fallback depth for one atom
total fallback steps
```

These are logical-model counts, not serialized byte-size or production timing claims.

## 2. Thousand-atom multi-replica experiment

The first deterministic stress state contains 1,000 atoms.

Eight offline replicas each perform 25 sparse random parent moves. The first 64 atoms are excluded from those random edits. Independent replica branches then create sixteen guaranteed two-node cross-move cycles among the reserved atoms.

The merged state contains:

```text
atoms                         1000
total placement revisions    1232
historical extra revisions    232
initial active cycles          16
```

The same replica states are merged in ten randomized orders.

Every order produces the same:

- active parent assignment;
- rejected revision set;
- torture metrics.

The resolver needs:

```text
rejected revisions             16
resolution iterations          16
maximum fallback depth          1
```

for this state.

This is encouraging for sparse ordinary concurrency: the candidate does not amplify these sixteen independent cycles into large fallback cascades, and merge arrival order remains non-semantic.

It is not a proof of bounded cost.

## 3. Adversarial repeated-fallback construction

A second test deliberately attacks the historical fallback assumption.

One atom `B` has a causal history of 64 successive placements:

```text
B -> C1
B -> C2
...
B -> C64
```

Independent offline replica branches concurrently make every `Ci` point back to `B`:

```text
C1  -> B
C2  -> B
...
C64 -> B
```

Only the newest pair is initially an active cycle:

```text
B -> C64 -> B
```

The resolver rejects the newest active B placement. B then falls back to `C63`, revealing another cycle. That rejection reveals `C62`, and so on.

Measured result:

```text
atoms                           65
total placement revisions      193
historical extra revisions     128
initial active cycles            1
resolution iterations           64
rejected revisions              64
maximum fallback depth          64
total fallback steps            64
```

The final valid state returns B to its retained root placement.

This is a constructive counterexample to any assumption that fallback depth is naturally close to one.

For an atom with `h` retained historical placements, an adversarial merged state can force the current candidate to reveal and reject placements one by one. The tested shape therefore admits fallback work linear in retained move history.

## 4. Consequence for compaction

The earlier hierarchy-validity experiment already showed that a causally superseded placement may become semantically relevant again when a newer placement is globally invalid.

The torture construction strengthens that result:

> preserving the most recent fallback is not sufficient to reproduce the full historical-fallback semantics in the general case.

An arbitrarily deep sequence of old placements can become successively active as newer placements are rejected.

Therefore the literal candidate creates tension between two goals:

```text
preserve maximum historical placement intent
        vs
bound retained metadata and validity-resolution work
```

Keeping every historical placement preserves the tested fallback semantics but leaves move-history growth unbounded until a stronger compaction proof exists.

Discarding history bounds storage but changes what happens when later merged constraints invalidate the current placement.

## 5. Algorithmic resource-amplification concern

The deep-fallback example also exposes a logical resource-amplification surface.

A merged state with only one initially visible cycle can force many rounds of:

```text
materialize
find invalid cycle
reject one placement
materialize historical fallback
repeat
```

Even if every revision is cryptographically authentic, a buggy or adversarial authorized replica could construct a state that exercises a long fallback chain.

Production hierarchy validity therefore needs an explicit complexity bound or a bounded fallback policy. Authentication alone does not solve this algorithmic issue.

## 6. Deterministic cycle-processing order

The torture harness also removes an implementation-level ambiguity from the research trace.

When several disjoint active cycles exist simultaneously, it chooses which cycle to process first by a canonical atom-ID set ordering. The losing edge inside that cycle is still chosen by the existing opaque `RevisionId` tie-break.

Reversing the in-memory mapping insertion order produces the same resolved hierarchy and metrics.

This does not make ID magnitude a clock. IDs remain deterministic tie-break material only after causality has selected the active placement revisions.

## 7. Current decision

After this pass:

1. The stable per-child parent-location model continues to converge under the tested 1,000-atom multi-replica state.
2. Sixteen guaranteed concurrent cycles resolve identically across ten randomized merge orders.
3. Sparse ordinary concurrency in that test requires one fallback per cycle.
4. The historical-fallback candidate has an explicit worst-case construction with fallback depth 64 from one initially active cycle.
5. The construction generalizes: fallback work can grow with retained placement history.
6. Full historical fallback therefore cannot be accepted as production semantics merely because small/random examples usually resolve at depth zero or one.
7. Historical placement retention and validity-resolution complexity are now one coupled design problem.
8. The next hierarchy candidate must make its loss-of-intent versus metadata/complexity tradeoff explicit rather than hiding it in compaction.

## 8. Next experiments

The next comparison should place the current full-history fallback against bounded alternatives, for example:

- one explicit last-known-valid placement witness plus a deterministic safe fallback;
- direct root/orphan fallback when the current placement is rejected;
- a compact authenticated validity witness that proves a prior placement without retaining arbitrary revision bodies;
- a policy that rejects a larger invalid move component once rather than walking an unbounded historical chain.

Each alternative should be tested for:

- convergence and merge-order independence;
- preservation of the user's immediately previous valid parent when possible;
- behavior when the fallback itself becomes invalid after merge;
- metadata per move;
- worst-case resolution iterations;
- compatibility with deletion and long-offline replicas;
- whether compaction changes user-visible placement semantics.

Sequence moved-anchor, checkpoint and finalization research continue in parallel.
