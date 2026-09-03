# A.P.C. lifecycle, location and content interaction research

Status: active research. This document records executable experiments that compose stable atom identity, delete-wins lifecycle, sequence location and durable working content. It does not freeze delete/restore policy or production storage.

Executable cases live in `tests/test_atom_interaction_lab.py` and use `reference_model/atom_interaction_lab.py`.

## 1. Question

The atomic-mutation experiment showed that strong cross-domain transactions are expensive under concurrency. That creates pressure to ask whether common operations that appear to touch several pieces of state can instead be represented as one semantic-domain update over stable identity.

The test composition uses:

```text
AtomId
├── lifecycle domain
├── location domain
└── content domain(s)
```

The current lifecycle candidate is the existing research-only delete-wins tombstone set. It is useful for interaction tests but does not yet define restore or tombstone compaction.

## 2. Move while content is dirty

The first scenario starts with two atoms and keeps atom A's content in a crash-safe dirty working epoch.

Another replica concurrently moves A after B.

When the remote structure becomes observable:

```text
before: A B
local:  A.content = dirty draft
remote: move A after B

after:  B A
```

The local content remains:

- dirty;
- attached to the same stable `AtomId`;
- not sealed into a new content causal revision;
- visible at A's new location.

Therefore remote movement does not need to manufacture content causality.

This is a direct payoff from separating:

```text
what is the atom?  -> AtomId
where is it?       -> location domain
what does it say?  -> content domain
```

A move is not a remove-content-plus-reinsert-content transaction.

## 3. Delete while content is dirty

A second replica deletes A while the local replica has an unsealed durable content draft.

After lifecycle merge:

```text
A is not visible
pending content remains durably retained internally
content domain creates no new causal revision merely because lifecycle changed
```

Ordinary further editing of A is rejected on a replica that already observes the delete.

This test deliberately separates two questions:

```text
is content retained as causal/storage state?
!=
is the atom visible/alive?
```

The current delete-wins candidate therefore does not need to erase content, rewrite location, and update every child field atomically just to hide an atom.

Whether hidden concurrent content may later participate in an explicit restore is still unresolved because restore itself is not yet accepted as a user-facing or logical operation.

## 4. Same-domain content conflict still creates a boundary

The separation is not permission to ignore relevant remote state.

If local A.content is dirty and a remote A.content revision becomes observable, the existing working-state rule still applies:

```text
seal local pre-remote content epoch
then merge remote content
```

The suite rejects same-content observation without the required pre-observation revision identity.

After the valid boundary, the local and remote content revisions remain genuinely concurrent in the content-domain frontier.

Thus:

```text
remote move/delete
    does not automatically causalize content

remote content change
    does affect content causality
```

The distinction follows semantic merge domain, not UI event order.

## 5. Delete wins visibility over concurrent content edit

Another scenario creates an offline content edit on one replica and a concurrent delete on another.

After merge:

- the content edit remains present in content causal state;
- lifecycle delete hides the atom;
- delete does not pretend that the content edit never existed;
- the content edit cannot resurrect the atom.

This is useful because deletion and content conflict do not need one artificial shared scalar register.

It also preserves enough information for future policy research around restore, proof of deletion stability and compaction.

## 6. Move cannot resurrect delete

The suite also merges a delete with a stale/concurrent move.

The move may still contribute location history, but the delete-wins lifecycle filter keeps the atom invisible.

This confirms the earlier architectural correction:

```text
location
!=
lifecycle
```

If deletion were encoded as `location = None`, a later causal location write could accidentally resurrect the object. The separated lifecycle candidate prevents that entire class of error.

## 7. Consequence for strong atomic transactions

These experiments explain why A.P.C. should avoid using multi-domain transactions merely because an operation has several visible consequences.

A remote move can change where an atom renders while its content continues unchanged by identity.

A delete can change visibility while preserving location anchors and content state underneath the lifecycle filter.

Therefore common operations can remain:

```text
move   -> one location-domain change
delete -> one lifecycle-domain change
edit   -> one content-domain change
```

rather than:

```text
move   -> rewrite location + content + container state atomically
delete -> erase location + content + descendants atomically
```

This substantially reduces exposure to the concurrent atomic-group tearing problem found in `ATOMIC_MUTATION_RESEARCH.md`.

## 8. Important unresolved policy: hidden pending local content

The remote-delete test intentionally retains a dirty local draft after the atom becomes hidden.

That proves retention and visibility can be separated, but it does not yet decide the final UX/lifecycle policy.

Possible future policies include:

- seal the hidden local draft for portable convergence before tombstone stabilization;
- retain it only as local crash-safe state until lifecycle reconciliation completes;
- include it in hidden causal state and later compact it once the delete is proven irreversible under the selected lifecycle policy;
- use it if an explicit restore operation is eventually defined.

The implementation MUST NOT silently discard such user data merely because a remote lifecycle change arrived. The exact retention horizon remains open.

## 9. Current decision

After this pass:

1. Stable AtomId successfully decouples content identity from sequence position in the tested move scenario.
2. A remote move does not require a content causal boundary merely because the atom appears elsewhere.
3. Lifecycle and content can remain separate merge domains: delete hides the atom without erasing content state.
4. A concurrent content edit does not resurrect a delete-wins atom.
5. A stale/concurrent move does not resurrect a delete-wins atom.
6. Same-domain remote content still requires the established pre-observation causal boundary.
7. Visibility dependency does not automatically imply causal ancestry dependency.
8. The separation of identity/location/lifecycle/content eliminates several apparent needs for strong multi-domain transactions.
9. Hidden pending local content under a remotely observed delete requires an explicit retention/compaction policy before lifecycle semantics can freeze.

## 10. Next experiments

The next pass should attack hierarchy and containment, where the interaction becomes harder:

- child insertion under a parent concurrently deleted on another replica;
- child content edit while an ancestor becomes deleted;
- whether descendants remain structurally addressable under hidden ancestors;
- move of a child between containers without remove+insert duplication;
- concurrent parent move and child insertion using historical anchors;
- delete-wins lifecycle plus future explicit restore candidate;
- safe compaction of hidden content and position anchors after deletion;
- whether container membership is itself a location domain that can reuse the single-element move construction.

Checkpoint, finalization, sync-capsule and atomic-mutation research continue independently.
