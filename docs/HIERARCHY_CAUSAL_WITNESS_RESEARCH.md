# A.P.C. hierarchy causal-witness research

Status: active research. This document refines the interpretation of the bounded hierarchy fallback experiments. It does not freeze production hierarchy semantics.

Executable cases live in `tests/test_hierarchy_causal_witness.py`.

## 1. New observation

The earlier full-history fallback was useful as an aggressive intent-preservation oracle for causal move chains:

```text
P -> Q -> R
```

If `R` becomes invalid after merge, retained history can fall back to `Q`, then `P`.

However, a parent-location register can also contain concurrent alternatives that were never observed by one another.

Consider:

```text
base: B under P

replica X: B -> Q   revision X
replica Y: B -> R   revision Y
```

where X and Y are concurrent and X wins the ordinary register tie-break. A separate concurrent move then makes `B -> Q` globally invalid.

The full-history resolver rejects X and materializes the next surviving register winner. That may be Y:

```text
full-history fallback: B -> R
```

But X never observed Y. Therefore R is not the causal predecessor of X and is not necessarily a meaningful "previous placement" from the perspective of the rejected move.

This corrects an overly broad interpretation of full-history fallback as always preserving more user intent.

## 2. Causal witness

The one-witness candidate stores the parent value that the current placement actually observed before the move was created.

For X above:

```text
requested parent: Q
causal predecessor witness: P
```

If X is rejected by global hierarchy validity, the bounded policy tries P rather than activating an unrelated concurrent alternative Y.

Thus the witness has a stronger semantic interpretation than "the next historical value":

> it represents the parent state causally observed by the rejected move.

No wall clock, branch order or transport order is involved.

## 3. Executable concurrent-alternative counterexample

The test constructs:

```text
B under P
X: B -> Q @900
Y: B -> R @800
Q -> B @9000
```

X and Y are concurrent. The Q back-edge makes X invalid.

The explicit-history resolver produces:

```text
B -> R
```

because Y becomes the next register winner after X is rejected.

The one-witness resolver produces:

```text
B -> P
```

because P is the parent recorded in X's causal context.

The test explicitly verifies that revision X does not contain revision Y in its causal context.

## 4. Full-history fallback is not a total resolver

A second counterexample removes an even stronger assumption.

Start with:

```text
B under P
B -> Q
```

and merge concurrent back-edges:

```text
Q -> B
P -> B
```

The current `B -> Q` placement is invalid, so full-history rejects it and falls back to the foundational `B -> P` placement. That placement is also invalid. Rejecting it exhausts every retained placement revision for B.

The full-history research resolver therefore has no defined valid result and raises `ModelError`.

The bounded one-witness policy is total for this tested shape:

```text
B -> Q
  invalid
    -> witness P
       invalid
         -> root
```

Direct-root fallback is also total.

This means full-history is not merely expensive. Its literal historical-reactivation semantics can fail to produce any placement unless an additional safe fallback is defined.

The multi-seed campaign runner now records such oracle failures as data instead of aborting the campaign.

## 5. Consequence for the candidate hierarchy semantics

These findings make one-witness fallback more attractive for three independent reasons:

1. bounded work and bounded hierarchy-validity metadata;
2. fallback is tied to the causal predecessor observed by the rejected move instead of to an arbitrary surviving concurrent alternative;
3. an explicit root fallback makes the tested resolver total when both current and witness placements are invalid.

The full-history resolver remains valuable as an adversarial oracle and as a model of maximum historical reactivation, but it should no longer be treated as an unquestioned semantic ideal.

There is still a deliberate loss in the one-witness policy. If both the current requested parent and the causal predecessor witness are invalid, the current candidate falls to root rather than walking an arbitrarily deep causal placement chain.

That tradeoff remains explicit:

```text
bounded current -> causal witness -> root
```

versus:

```text
unbounded historical reactivation
```

## 6. Benchmark evidence motivating the next pass

The first workstation results strengthen the need for statistical comparison.

A 100,000-atom state with 1,000,000 independent branch revisions plus 64 forced cycles produced 65 active cycles. One-witness resolved all 65 through a witness and required no root fallback, while direct-root resolution produced a different parent graph.

A smaller oracle run with 5,000 atoms and 50,000 independent branch revisions produced 34 active cycles. Full-history, one-witness and root all resolved the same number of cycles, but all three final parent graphs differed.

This is expected under the concurrent-alternative counterexample: full-history may reactivate another concurrent placement while one-witness returns to the causal predecessor.

The next campaign should therefore measure per-atom disagreement counts, not only whole-graph digest equality, and should count seeds where full-history exhausts all placements.

## 7. Next experiments

The next statistical pass should measure across many deterministic seeds and move densities:

- spontaneous cycle count beyond intentionally forced cycles;
- atoms where one-witness differs from direct root;
- atoms where one-witness differs from full-history;
- full-history placement-exhaustion frequency;
- atoms where full-history fallback selected a placement not observed by the rejected current move;
- witness-to-root fallback frequency;
- resolution iterations and runtime;
- effect of branch density on those rates.

A production decision should be based on both semantic counterexamples and these distributions, not on one benchmark seed.
