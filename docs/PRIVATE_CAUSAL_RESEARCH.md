# A.P.C. private causal squashing research

Status: active research. This document records executable experiments around causal identities that have never been exposed outside one replica. It does not freeze signing, revision finalization or portable format rules.

Executable cases live in `tests/test_private_causal_lab.py` and use `reference_model/private_causal_lab.py`.

## 1. Question

The durable working-state experiment introduced causal observation boundaries. A dirty local epoch may be sealed before a newly arrived remote state becomes observable so the pre-remote work remains genuinely concurrent with that remote state.

That can temporarily create a local causal node such as:

```text
       Lpre
      /
B ----
      \
       R
       \
        Lpost
```

where `Lpost` later observes and causally supersedes both `Lpre` and `R`.

If `Lpre` was never externally visible, the next question is:

> Does A.P.C. need to preserve that dominated private causal identity forever?

## 2. Exposure distinction

The experiment distinguishes:

```text
private local causal identity
vs
externally exposed causal identity
```

A revision becomes externally exposed no later than the moment a representation naming that revision or depending on it is handed to an external transport.

Acknowledgement is too late as the exposure boundary. A network or GitHub request can succeed remotely while the local acknowledgement is lost.

Therefore:

```text
transport handoff
    -> possibly exposed forever
```

not:

```text
transport ACK
    -> first exposure
```

## 3. Ancestor exposure closure

If revision `Lpost` is sent while it directly names private parent `Lpre`, then `Lpre` is no longer safely private even if its own node was not sent as a separate file.

A receiver must either already know `Lpre` or obtain enough information to validate the dependency. Its identity has crossed the boundary through `Lpost`.

The lab therefore marks exposure transitively across named causal ancestors.

This creates a strict ordering rule for optimization:

```text
squash private dominated causal nodes
        BEFORE
final transport handoff
```

Once a descendant has been handed to transport with the old parent relation, the private parent can no longer be assumed unknown to the outside world.

## 4. Logical squashing experiment

The first executable transformation removes a private, unexposed, non-frontier node and rewires retained descendants to the removed node's nearest retained causal parents.

Example before:

```text
B -> Lpre -> Lpost
 \          /
  -> R ----
```

If `Lpre` was never exposed and `Lpost` is the current local frontier, the test-only logical transformation can remove `Lpre`.

Because `R` already descends from `B`, transitive reduction may leave:

```text
R -> Lpost
```

while preserving the same current logical frontier and materialized value.

The test confirms that causal reachability from retained external revisions to the current frontier is preserved.

## 5. Exposure blocks removal

Two counter-tests establish the safety boundary.

### `Lpre` exposed directly

If `Lpre` is marked exposed before squashing, it is retained even after `Lpost` dominates it.

### `Lpost` exposed before squashing

If `Lpost` is handed to transport while its parent set still names `Lpre`, exposure closure marks `Lpre` exposed as well. A later squashing attempt removes nothing.

The same logical state can therefore have different safe compaction opportunities depending on whether optimization occurred before or after the external boundary.

## 6. Repeated observation epochs

The suite creates 64 local observation epochs interleaved with a remote causal chain.

Each epoch creates one temporary pre-remote local causal revision. A final local revision eventually observes the last remote state and dominates the earlier private local markers.

Before squashing, all 64 private observation markers are retained.

After exposure-aware squashing:

```text
removed private dominated local nodes   64
current logical frontier                 1
```

The external remote chain remains in this lab because it was already externally sourced/exposed; separate checkpoint research is responsible for compacting externally meaningful historical causality.

This demonstrates an important composition:

```text
working-state coalescing
    reduces causal-node creation

private squashing
    removes never-shared dominated local nodes

checkpointing
    addresses old externally meaningful causality
```

These are different optimizations and should remain separate.

## 7. Cryptographic finalization problem

The current lab deliberately rewrites parent sets while retaining the same opaque revision ID so logical equivalence can be tested easily.

That is not automatically valid for production.

Once a portable revision is authenticated or signed, changing its parent set would normally invalidate the authenticated statement. If a revision ID is content-derived, rewriting parents could also require a different ID.

This exposes a likely architectural distinction:

```text
local provisional causal/observation marker
        !=
final portable authenticated RevisionId
```

Possible directions include:

- keep private observation markers as device-local `WorkingEpochId` objects and mint/finalize portable revisions only when required;
- permit provisional portable-like revisions locally, but assign a new final revision identity and authentication proof after safe squashing;
- define another studied authenticated construction that explicitly permits safe pre-exposure canonicalization.

No choice is accepted yet.

The important rule is already clear: **cryptographic finalization must not occur so early that it prevents safe elimination of causal identities no other replica could possibly know.**

## 8. Relationship to user durability

Removing a private dominated causal marker must not remove the latest durable user state.

The optimization applies only after another retained local state causally and semantically supersedes that marker.

If pre-remote local work remains an unresolved concurrent value, its causal identity is still part of the current frontier and is not removable.

Therefore private squashing is not a mechanism for discarding losing or inconvenient user edits.

## 9. Current decision

After this pass:

1. Never-exposed dominated local causal identities are a distinct compaction class from externally meaningful historical causality.
2. The tested logical model can bypass such private nodes while preserving the current frontier, materialized scalar value and retained causal reachability.
3. Current frontier IDs are never removed by private squashing.
4. Any exposed revision ID is never removed by this mechanism.
5. Exposure must be considered to occur at transport handoff, not acknowledgement.
6. Exposing a descendant exposes the causal identities it still names as dependencies.
7. Safe squashing therefore belongs before transport finalization/publication.
8. Production signing and revision-ID finalization must account for this optimization; the test-only same-ID parent rewrite is not itself a production proposal.
9. A provisional local causal identity separate from final portable `RevisionId` is now a serious candidate and requires direct testing.

## 10. Next experiments

The next pass should compare two concrete local models:

- **early-finalized model:** every observation boundary immediately mints a final portable RevisionId and authentication statement;
- **provisional-epoch model:** observation boundaries persist crash-safely as local epoch metadata, while final portable causal revisions are minted only at an external publication boundary.

The comparison should test:

- same final scalar semantics;
- crash points before and after remote observation;
- unresolved concurrent local work;
- repeated remote observations before any publication;
- publication between two observation epochs;
- duplicate/lost transport acknowledgement;
- number of final portable causal identities produced;
- compatibility with future per-replica signing-key evolution.

Checkpoint, sync-capsule and moved-anchor research continue independently in parallel.
