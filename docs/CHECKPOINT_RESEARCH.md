# A.P.C. causal checkpoint research

Status: active research. This document records executable compaction experiments. It does not freeze the portable format or cryptographic construction.

Executable cases live in `tests/test_checkpoint_lab.py` and use `reference_model/checkpoint_lab.py`.

## 1. Question

The direct-frontier DAG reduced ordinary causal-reference growth from quadratic to linear, but it still retains causal nodes indefinitely.

The next question is stricter:

> Can A.P.C. discard old causal nodes while still correctly accepting a new branch created by a replica that has been offline since an old historical revision?

The experiment keeps A.P.C.'s current constraints:

- no wall-clock ordering;
- no GitHub/server ordering;
- no Lamport timestamp;
- no per-replica causal counter in the current candidate;
- opaque revision IDs identify causal nodes;
- genuine scalar concurrency is still materialized using the existing deterministic revision-ID tie-break.

## 2. Exact-coverage checkpoint oracle

The first checkpoint model compacts a complete direct-parent DAG into two parts:

```text
retained current causal frontier revisions
+
exact set of historical revision IDs already covered
```

For a linear history:

```text
R1 -> R2 -> ... -> R100
```

the compacted active state may retain only `R100` as a live causal revision while keeping `R1..R99` in an exact historical-ID coverage set.

The coverage set is deliberately not proposed as the final production structure. It is an oracle for the information a future compaction mechanism must preserve or prove somehow.

## 3. Frontier logical IDs cannot be replaced by fresh checkpoint IDs

A tempting compaction is:

```text
old graph -> fresh CheckpointId C
```

and then use `C` as the new logical frontier.

The executable counterexample rejects that.

Suppose the real current logical frontier is revision `100`, and an old offline replica produces a genuinely concurrent revision `75`.

Under the existing scalar rule:

```text
100 || 75
```

revision `100` wins the deterministic concurrent tie-break.

If compaction replaced revision `100` with an unrelated fresh checkpoint ID `60`, the same future conflict would become:

```text
60 || 75
```

and revision `75` would win.

That changes user-visible semantics solely because storage was compacted.

Therefore:

> checkpoint/storage identity and logical revision identity are separate concepts.

Compaction MUST preserve every logical frontier identity whose ID can still participate in future semantic conflict resolution.

## 4. Long-offline branch reconnection works with exact coverage

The test suite creates:

```text
current: R1 -> ... -> R100

offline snapshot at R20:
R1 -> ... -> R20 -> X
```

The current side compacts `R1..R99`, retaining only `R100` plus exact historical-ID coverage.

The returning replica transports only its genuinely new revision `X`, whose direct parent remains `R20`.

Because `R20` is in the exact coverage set, the compacted current state can validate that the missing parent belongs to an already incorporated historical baseline without restoring the full old DAG.

The merged frontier is correctly:

```text
{R100, X}
```

not:

```text
{X}
```

`X` did not observe revisions `R21..R100`, so it is concurrent with the current frontier rather than causally newer than it.

The experiment also creates 64 stale branches from random historical points, delivers their new revisions in random order, and compares the compacted result with the full un-compacted causal oracle. Frontier IDs and materialized scalar values match.

## 5. Dropping historical membership creates an unavoidable ambiguity

The same stale branch is rejected if the compacted state keeps only the current frontier and discards all information that `R20` was previously covered.

The receiver then sees:

```text
X.parents = {R20}
```

but has no valid basis for deciding whether `R20`:

- belongs to its own compacted past;
- belongs to another unrelated history;
- is missing because transport is incomplete;
- is invalid.

A.P.C. must never guess this relationship from arrival order or ID magnitude.

Therefore exact stale-baseline reconnection requires either retained historical membership information or an independently verifiable proof of that membership.

## 6. Coverage metadata remains linear

The exact oracle removes old revision objects from the active DAG, but its historical-ID coverage set still grows with the number of compacted opaque IDs.

The executable measurement gives:

```text
256-revision chain after compaction:
retained live revisions      1
covered opaque IDs         255

511 historical IDs covered:
raw 256-bit ID payload   16352 bytes
```

The raw-payload metric ignores container/index overhead.

With opaque 256-bit IDs, a straightforward exact membership set therefore still scales linearly with historical revision count. Sorting or a better index can reduce implementation overhead but does not make the historical membership requirement disappear.

This is a different problem from the direct-frontier edge optimization:

```text
explicit ancestor edges: O(n^2) -> O(n)
active causal nodes:      can be compacted strongly
exact arbitrary-old-ID membership: still grows with history
```

## 7. Important boundary exposed by the experiment

The following four goals cannot simply be assumed to coexist for free:

1. opaque unstructured causal IDs;
2. exact merge with an arbitrarily old offline replica;
3. deletion of all old causal/history membership information;
4. no external membership proof or rebootstrap mechanism.

The current test demonstrates the conflict concretely: remove the old-ID coverage knowledge and a valid stale branch becomes indistinguishable from a branch with an unknown baseline at the compaction API boundary.

This is not yet a formal impossibility theorem for every possible cryptographic construction. It is a design boundary that the next candidates must address explicitly rather than hiding behind a generic `checkpoint` object.

## 8. Candidate directions

Several directions remain compatible with the current architecture and should be tested rather than selected by intuition.

### A. Exact cold coverage index

Keep exact historical-ID membership, but move most of it out of the hot active DAG/index.

Pros:

- simple semantics;
- exact stale-replica reconnection;
- easy oracle comparison.

Cost:

- total retained membership remains linear.

### B. Authenticated set commitment plus membership proofs

Keep a compact authenticated commitment in active state and obtain a proof when an old ID must be recognized.

Open questions:

- who can generate a proof for a replica that was offline before compaction;
- what auxiliary proof/index data must remain available;
- whether proof storage simply moves the linear cost elsewhere;
- interaction with encryption, signing and malicious inputs.

No cryptographic accumulator or Merkle construction is selected yet.

### C. Retained transport generations / causal horizon

Keep exact mergeability for a bounded number of generations. A replica older than the retained horizon must obtain a newer bootstrap/checkpoint before normal incremental sync resumes.

This bounds active history but introduces an explicit stale-baseline policy.

A rebootstrap policy must not silently discard unsynchronized user edits.

### D. More structured causal identity

Per-replica counters/version vectors can summarize long histories much more aggressively, but that changes the current ID-only causal model and therefore is not adopted merely for convenience.

It remains a comparison family if the pure opaque-ID model reaches an unacceptable lower bound.

## 9. Causal revision frequency is a separate optimization

Local crash durability does not necessarily require creation of a new portable causal node for every keystroke.

A.P.C. already separates:

```text
local durable editing
!=
remote publication
```

The same separation may reduce causal-node creation substantially:

```text
many locally durable working-state changes
        |
coalesced causal publication / observation boundary
        |
one portable causal revision
```

This requires its own experiment because remote changes can arrive while a local working edit is pending. The causal context captured by the eventual revision must still match what the user/replica actually observed.

Reducing causal-node creation before checkpointing may be as important as compacting nodes afterward.

## 10. Current decision

After this pass:

1. Direct-frontier causality remains the strongest current ID-only causal candidate.
2. Old causal node bodies can be removed from the hot active DAG while retaining exact stale-branch merge semantics in the tested scalar cases.
3. Current logical frontier revision IDs MUST survive compaction; replacing them with arbitrary checkpoint IDs can change conflict winners.
4. Exact covered-ID membership is sufficient for tested long-offline reconnection, but it remains linear metadata.
5. Removing covered-ID knowledge without a proof/horizon mechanism makes valid stale parents unverifiable; the implementation must reject rather than infer.
6. Checkpoint identity, causal revision identity, transport generation identity and cryptographic commitment identity must remain separate concepts.
7. The next checkpoint experiment should compare cold exact coverage, proof-backed coverage and retained-generation policies.
8. A separate experiment should reduce the number of causal revisions created from sustained local editing while preserving crash durability and observation semantics.

Sequence/moved-anchor research continues independently in parallel.
