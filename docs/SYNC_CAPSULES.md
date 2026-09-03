# A.P.C. sync capsules

Status: design model. Not yet a frozen portable encoding.

## 1. Purpose

A sync capsule is a protected, transport-independent partial projection of A.P.C. state.

It exists so synchronization cost can scale with changed information rather than total continuum size.

A capsule is **not**:

- a Git commit;
- an operation log entry;
- a timestamped event;
- a complete `.apc` file;
- a transport-specific object.

## 2. Required semantic property

A valid capsule must contain sufficient state and merge metadata for the A.P.C. core to incorporate it without consulting transport ordering.

Conceptually:

```text
merge(local_state, capsule_state)
```

must use the same logical rules as merging two complete valid states for the domains represented by the capsule.

The transport may deliver capsules:

- more than once;
- later than another capsule published after them;
- in a different batch from related attachment chunks;
- after a long offline interval.

Correctness must therefore come from A.P.C. state semantics rather than arrival order.

## 3. Dirty-domain projection

The running application tracks which merge domains have changed since their last published projection.

For example:

```text
Atom A
  title          clean
  body           dirty
  children       clean

Atom B
  lifecycle      dirty
  location       dirty
```

A sync projection may contain only the dirty domains plus any causal/lifecycle dependencies needed to merge them correctly.

Repeated local modifications to one dirty domain may be collapsed before publication.

This is particularly important for typing:

```text
text revisions generated locally:
R1 -> R2 -> R3 -> R4 -> R5

publication boundary:
export the newest sufficient mergeable state,
not five transport events
```

Whether intermediate logical revisions can be discarded from the sync projection depends on the final causal encoding. The transport API must not assume they are required merely because they occurred.

## 4. Capsule identity

Each capsule has an opaque stable `CapsuleId`.

`CapsuleId`:

- is not a clock;
- is not an ordering value;
- is not a user identity;
- may be used for duplicate detection, integrity binding and transport filenames.

The exact bit width and encoding remain open until the identifier model is frozen.

## 5. Candidate logical envelope

A future capsule encoding may conceptually contain:

```text
Capsule
├── envelope version
├── continuum identity binding
├── capsule identity
├── protected state projection
│   ├── merge-domain state
│   ├── causal summary
│   ├── lifecycle state
│   └── attachment/chunk references
└── integrity/authentication material
```

This is conceptual structure only. It does not freeze plaintext metadata exposure, binary layout or cryptographic primitives.

## 6. Attachment data

Large binary data is separate from small structured merge state.

A change to a large attachment may create independently protected chunks referenced from a capsule.

Transport adapters may partition those chunks further to satisfy payload limits without changing the logical attachment identity.

The core requirement is that a receiver can validate which chunks belong to the referenced attachment state before making that state visible.

## 7. Atomic visibility of multipart publication

A single logical sync projection may require several transport files.

The receiver MUST NOT expose a partially received logical state as complete when required parts are missing.

A future encoding therefore needs one of:

- a small protected manifest identifying all required parts;
- self-describing part membership plus a completion condition;
- another equivalent mechanism.

Multipart completion is transport assembly. Semantic merge begins only after the required protected unit can be validated.

## 8. Coalescing constraints

Coalescing is permitted only when it preserves future merge behavior.

A publisher must not discard metadata merely because an intermediate state is no longer visible locally if another offline replica could still require that information to establish causality, lifecycle dominance or deletion safety.

The current explicit-ancestor reference model is therefore useful as an oracle for proving whether a compact capsule representation preserves the same result.

## 9. Checkpoints

A checkpoint is a compact sync projection intended to replace many older transport publications.

A checkpoint is not user-visible history and does not need to preserve intermediate states.

A valid checkpoint must preserve the merge-relevant information needed by replicas that are permitted to catch up from it.

The final checkpoint design depends on:

- compact causal metadata;
- lifecycle/tombstone semantics;
- position compaction rules;
- attachment reachability;
- retained-baseline policy.

## 10. Transport independence

A generic core interface should eventually resemble:

```text
mark_dirty(domain)
export_sync_projection(budget, baseline)
import_sync_capsule(capsule)
acknowledge_published(capsule_id / projection)
```

The exact API is not frozen.

GitHub-specific rules such as object-size ceilings, branches, commit retries, conditional HTTP requests and repository permissions live outside this interface.

This separation is mandatory: replacing GitHub with LAN or another transport must not require changing the logical merge model.

## 11. Protection boundary

A clear sync projection exists only inside the trusted A.P.C. core/sync layer.

Before a capsule or multipart part is handed to GitHub, LAN, removable-media sync or any other transport, its sensitive state must already be end-to-end protected.

Conceptually:

```text
dirty merge domains
      |
clear sync projection
      |
protect/authenticate
      |
opaque capsule / part
      |
transport adapter
```

Transport-side partitioning, retry, storage and delivery operate on opaque protected objects. A transport adapter must not need to decrypt them to perform its job.

A multipart publication must cryptographically bind part identity, publication identity and completeness metadata strongly enough that an attacker or broken transport cannot substitute a part from another publication without detection.

The exact authenticated-encryption and key-evolution construction remains open. The executable sync lab uses a deliberately non-cryptographic opaque test double only to test this API boundary.
