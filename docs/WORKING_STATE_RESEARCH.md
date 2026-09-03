# A.P.C. durable working-state causality research

Status: active research. This document records executable experiments that separate local crash durability from portable causal revision creation. It does not freeze storage, UI or synchronization encoding.

Executable cases live in `tests/test_working_state_lab.py` and use `reference_model/working_state_lab.py`.

## 1. Question

A.P.C. already separates local durability from remote publication. The causality research raises a related question:

> Must every locally durable edit create a new portable causal revision?

For continuous text entry, doing so would create causal metadata at keystroke frequency even though many nearby local states are never independently observed by another replica.

The experiment therefore separates:

```text
local durable working state
        !=
portable causal state
        !=
transport publication
```

A local value may be crash-safe immediately while several such values are coalesced into one causal revision later.

## 2. Working edit epoch

The first local edit after a clean causal state starts a working edit epoch.

At that point the implementation captures the exact causal frontier that the local editor has observed:

```text
observed frontier F
        |
local durable edit 1
local durable edit 2
...
local durable edit N
```

The individual working values are durable local state, but no portable causal revision is created yet.

When the epoch is sealed, one revision is created:

```text
L.parents = F
L.value   = final durable working value
```

The revision ID still has no time semantics.

## 3. Keystroke coalescing result

The executable test performs 10,000 durable local writes to one scalar domain.

Before sealing:

```text
durable writes                  10000
locally created causal revisions    0
```

After sealing:

```text
durable writes                  10000
locally created causal revisions    1
```

This shows that crash durability does not require causal-node creation at keystroke frequency.

The result is stronger than transport batching: the intermediate values are not merely withheld from GitHub; they are not portable causal nodes at all.

## 4. Crash recovery requirement

A pending working epoch cannot exist only in volatile editor memory.

The test snapshot persists enough local state to recover:

- the latest durable working value;
- whether the domain is dirty;
- the causal frontier observed when the epoch started;
- already-created but not yet transported local causal IDs.

After simulated restart, the working value and its original observation frontier are restored and the eventual causal revision uses the same observed parents.

Therefore coalescing does not weaken the existing durability invariant.

## 5. Remote arrival exposes a causality trap

Consider local work created from base `B`:

```text
B -> local working edits (not yet causalized)
```

while another replica independently creates remote revision `R` from the same base.

A naive implementation could merge `R` first and then, at publication time, create local revision `L` using the *latest* causal frontier:

```text
B -> R -> L
```

That graph says `L` observed `R`.

But the local working value was actually produced before `R` became observable. The graph would therefore invent causal knowledge that never existed and could incorrectly suppress a genuine concurrent remote value.

The executable counterexample deliberately gives `L` a smaller ID than `R`. The naive model still makes `L` win because it falsely becomes a causal descendant of `R`.

This is rejected.

## 6. Observation boundary rule

The current candidate introduces a semantic observation boundary.

If a remote causal state is about to become observable while a local working epoch is dirty:

```text
1. seal the pre-remote local working epoch using its captured frontier;
2. merge the remote causal state;
3. expose the merged materialized state;
4. any subsequent local working epoch captures the new merged frontier.
```

Example:

```text
       Lpre
      /
B ----
      \
       R
```

After remote observation the frontier is `{Lpre, R}` because the pre-remote work and remote work were genuinely concurrent.

If the user then edits after the merged state is actually observable, the next sealed local revision is:

```text
Lpre ----\
          -> Lpost
R -------/
```

`Lpost` causally supersedes both sides even if its opaque revision ID sorts below both parents.

The test confirms this behavior.

## 7. Receipt is not observation

The method tested here is intentionally an *apply/observation* boundary, not a network-receipt boundary.

Receiving or downloading an opaque protected capsule does not by itself mean the user/editor has observed its semantic content.

Therefore the future sync pipeline should distinguish at least:

```text
transport receipt
        |
authenticate / validate
        |
semantic merge becomes applicable
        |
working-domain observation boundary
        |
render / edit from merged state
```

This distinction matters when a local working edit is still pending.

## 8. Revision count follows causal observation changes

A second experiment performs 8,000 durable local edits across four working epochs with three remote observation boundaries.

Result:

```text
durable local writes             8000
locally created causal revisions    4
```

The causal-node count therefore tracks meaningful observation boundaries rather than keystrokes.

This is the desired scaling direction for sustained local editing.

## 9. Scope limitation

The current model contains one scalar merge domain only.

It does not yet answer whether observing a remote change in one independent atom/field should force a causal boundary in another dirty domain.

That depends on whether causal context is domain-local, atom-local, transaction-local or shared more broadly. The answer must follow merge semantics rather than UI timing.

Likewise, a future collaborative text primitive may have more precise sub-block observation semantics than the current scalar text experiment.

## 10. Newly exposed optimization: private causal squashing

The observation-boundary rule can temporarily create a local revision `Lpre` before merging remote state.

If `Lpre` has never crossed the transport boundary and a later local revision `Lpost` causally dominates it, no external replica could have created a branch that names `Lpre` as a parent.

This suggests a further candidate optimization:

```text
unpublished local causal node
        +
later local descendant
        ->
possible safe local squashing / parent rewiring
```

Once an ID has been handed to an external transport, it must be treated as exposed even if acknowledgement is lost; another replica may already have received it.

Therefore any such squashing rule must distinguish:

```text
private local causal identity
vs
externally exposed causal identity
```

This is not implemented or accepted yet. It is the next experiment.

## 11. Current decision

After this pass:

1. Local crash durability and portable causal revision creation are separate mechanisms.
2. Thousands of durable working-state writes may coalesce into one portable causal revision when causal observation context does not change.
3. A pending local value must retain the causal frontier it actually observed; publication time must not silently substitute a newer frontier.
4. Remote state that becomes semantically observable while local work is pending creates a causal observation boundary.
5. Pre-observation local work must remain concurrent with newly observed remote work unless a later revision genuinely observes and supersedes both.
6. Transport receipt and semantic observation are separate events.
7. The pending working state, including its captured observation frontier, is part of crash-recovery state.
8. Causal revision frequency can scale with observation boundaries rather than keystrokes.
9. Unpublished dominated local causal nodes may admit further safe squashing, but exposure rules must be proven first.

## 12. Next experiments

The next pass should attack:

- safe squashing of dominated causal nodes that have never been externally exposed;
- the exact moment an ID becomes irrevocably exposed (transport handoff versus acknowledgement);
- publication loss/duplicate delivery after exposure;
- multiple dirty domains receiving a remote update in only one domain;
- atomic multi-domain edits and whether they require one shared observation boundary;
- interaction with lifecycle tombstones and moved sequence elements;
- crash points around `seal -> merge remote -> persist merged working state`;
- capsule generation after several private observation epochs.

Checkpoint and moved-anchor research continue in parallel.
