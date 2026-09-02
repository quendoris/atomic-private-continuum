# A.P.C. logical model — design draft

This document defines the logical behavior of A.P.C. independently of binary encoding, UI toolkit, operating system and synchronization transport.

The model is state-based. A.P.C. does not require a persistent action/event log to define the portable continuum. Diagnostic logs and optional history are auxiliary encrypted data and are not part of this model.

## 1. State

A continuum is one logical state composed of independently addressable atoms.

Conceptually:

```text
ContinuumState
├── continuum_id
├── format_version
├── root ordered collection
├── atom map
├── causal/merge metadata
└── portable cryptographic metadata
```

The physical `.apc` container may index or partition this state internally, but those storage details do not change the logical model.

## 2. Identifiers

Correctness MUST NOT depend on wall-clock time, device time, Git timestamps or arrival order.

The logical model uses opaque identifiers.

Current identifier classes are:

- `ContinuumId` — identifies one continuum;
- `AtomId` — identifies one persistent atom;
- `ReplicaId` — identifies one independently evolving replica identity;
- `RevisionId` — identifies one logical revision of one merge unit;
- `KeyStateId` — identifies portable authentication/key-evolution state where required;
- attachment/content identifiers as required by the final container design.

For the initial design, identifiers used as globally unique logical identities SHOULD provide at least 256 bits of collision-resistant space and SHOULD be generated from a cryptographically secure random source unless a later specification explicitly defines a content-derived identifier.

An identifier MUST NOT encode semantic recency.

Where a deterministic total order over opaque IDs is required only as a tie-break, implementations MUST use canonical unsigned lexicographic byte order.

A larger ID is not "newer". It only wins a defined tie-break between states that have no causal order.

## 3. Causality

A.P.C. requires a partial causal order, not a global clock.

For revisions `A` and `B`:

- `A < B` means that the writer of `B` had incorporated `A` into the relevant causal context before producing `B`;
- `A || B` means neither revision causally descends from the other.

The portable format MUST carry enough causal metadata to determine the relationships required by merge even after long offline periods.

That metadata is not an event history. It exists only because deterministic merge requires proof of what a revision had already observed.

The exact compact representation of causal context is intentionally unresolved. It MUST NOT rely on wall-clock timestamps. An explicit ID-linked ancestry graph is acceptable for prototypes, but a production design must evaluate metadata growth and safe compaction before it becomes normative.

## 4. Atom

An atom is the smallest independently addressable persistent information object.

Conceptually:

```text
Atom
├── id: AtomId
├── type
├── lifecycle
├── fields
├── collections
└── references
```

An atom is not necessarily a visual card. UI objects may be composed from multiple merge units inside one atom, and complex structures are normally composed from multiple atoms.

Examples:

- a text line or paragraph;
- a heading;
- a list item;
- a container/sticker;
- an attachment descriptor;
- an annotation;
- a future extension type.

## 5. Merge units

Different data requires different conflict behavior. A.P.C. therefore does not merge an atom as one indivisible value.

The initial logical primitive classes are:

### 5.1 Scalar register

A scalar register contains one materialized value, for example:

- title text;
- checklist checked state;
- attachment caption;
- a single reference;
- a type-specific option.

Each scalar revision has a `RevisionId` and causal context.

Merge semantics:

1. If revision `B` causally descends from `A`, `B` supersedes `A`.
2. If `A` and `B` are genuinely concurrent, both are concurrency candidates and the materialized value is selected deterministically by canonical `RevisionId` order.
3. Arrival order MUST NOT affect the result.
4. A conforming state representation MUST retain enough causal information that a later merge cannot incorrectly resurrect a revision that was already causally superseded.

A production encoding may internally retain a small concurrent frontier rather than only the visible winner. The visible value and the merge metadata are separate concepts.

### 5.2 Ordered collection

An ordered collection contains independently identified members, normally `AtomId` references.

It is used for:

- the top-level vertical continuum;
- lines/items inside a list;
- children of a container/sticker;
- other ordered structures.

Required semantics:

1. Concurrent insertion of distinct members MUST preserve every inserted member.
2. Member identity MUST NOT depend on position.
3. Concurrent insertions into the same logical gap MUST converge to one deterministic order.
4. Moving one member MUST NOT rewrite the identities of unrelated members.
5. Concurrent movement of the same member MUST converge deterministically.
6. The ordering scheme MUST NOT depend on clocks.

The exact sequence structure is not yet fixed. Candidate designs may use position identifiers, an RGA/LSEQ/Logoot-like structure or another construction, but the chosen structure must satisfy the algebraic merge properties in this document and scale without global re-numbering as a correctness requirement.

### 5.3 Unordered collection

Some future fields may require sets of independent references or tags.

Such fields MUST declare their add/remove conflict policy explicitly. A.P.C. MUST NOT assign one implicit add-wins or remove-wins policy to every unordered collection.

### 5.4 Immutable content reference

Large binary content is referenced from an atom rather than treated as a scalar byte string that must be loaded into memory.

Replacing an attachment reference and modifying annotations around that attachment are separate merge domains.

## 6. Text structure

The primary editor is logically an ordered sequence of blocks, not one monolithic text string.

A hard structural line/block boundary creates a separately addressable atom. Soft wrapping caused by screen width does not.

A simple text block may initially store its textual payload as a scalar field. This means concurrent replacement of the same text block follows scalar semantics, while concurrent insertion of different neighboring text blocks preserves both.

This does not prevent a future richer collaborative text primitive. Such a primitive must be introduced as a compatible type/merge extension rather than by silently changing scalar semantics.

Example:

```text
root sequence
├── text atom: "Specification"
├── attachment atom: spec.pdf
├── text atom: "Notes"
└── sticker atom
    └── children sequence
        ├── checklist item A
        └── checklist item B
```

## 7. Field independence

Merge is performed at the narrowest declared merge domain.

Example:

```text
sticker
├── title       scalar register
└── children    ordered collection
```

If replica A changes `title` while replica B inserts a child, merging MUST preserve the child and independently resolve the title field. Replacing the title MUST NOT replace the sticker object or its child collection.

This rule is central to A.P.C. atomicity.

## 8. Logical merge

Define logical merge as:

```text
M : State × State -> State
```

For valid states belonging to the same continuum and compatible format semantics, logical merge MUST be:

- **deterministic** — the same logical inputs produce the same logical result;
- **commutative** — `M(A, B) = M(B, A)`;
- **associative** — `M(M(A, B), C) = M(A, M(B, C))`;
- **idempotent** — `M(A, A) = A`.

Equality here means logical state equality. Physical ciphertext bytes may differ because authenticated encryption may use fresh nonces or a new container layout.

A merge implementation MUST:

1. authenticate and validate both inputs before accepting their protected content;
2. confirm that they refer to the same continuum;
3. merge atom identity sets;
4. merge each atom's lifecycle, fields, collections and references independently according to declared semantics;
5. preserve unknown compatible extensions without corrupting them;
6. reject states whose required critical semantics cannot be interpreted safely;
7. emit one valid logical state.

A transport never performs these semantic steps.

## 9. Scalar concurrency example

Starting state:

```text
title = "Draft" @ R0
```

Replica A, offline:

```text
title = "Protocol" @ RA
causal context includes R0
```

Replica B, independently:

```text
title = "Specification" @ RB
causal context includes R0
```

`RA || RB`.

Both replicas therefore recognize a true concurrent scalar conflict. The materialized title is selected by the canonical revision-ID tie-break. No timestamp is consulted.

If B had first incorporated `RA` and then produced `RB`, then `RA < RB` and `RB` MUST win regardless of the ID byte ordering.

## 10. Concurrent insertion example

Starting sequence:

```text
A
B
```

Replica X inserts `X1` between A and B.
Replica Y inserts `Y1` between A and B while offline.

Merged state MUST contain:

```text
A
X1/Y1 in deterministic order
Y1/X1 in deterministic order
B
```

Neither insertion may disappear merely because another replica inserted at the same position.

## 11. Lifecycle and deletion

Deletion is a logical state transition, not immediate physical erasure.

A physical delete alone is unsafe because a long-offline replica could later reintroduce an atom that another replica had deleted.

The portable model therefore requires lifecycle metadata sufficient to prevent accidental resurrection during merge.

This metadata is not a user-facing recycle bin and does not imply a recovery feature.

Initial rules:

- deleting an atom marks its lifecycle as deleted;
- ordinary edits to unrelated atoms remain independent;
- descendants/children are not rewritten merely because a parent is deleted;
- physical removal of deleted data or tombstone metadata is permitted only when the implementation can prove that doing so cannot change the result of any valid future merge under the supported replica model.

The exact concurrent delete-versus-edit policy and safe compaction proof remain open and must be specified before production format freeze.

## 12. Merge metadata is not history

A.P.C. distinguishes three things:

1. **current user state** — portable content;
2. **minimal causal/merge metadata** — portable data required to make convergence correct;
3. **history/diagnostic logs** — optional auxiliary data not required to reconstruct current state.

The format MUST NOT preserve an unbounded event log merely because an edit occurred.

If a piece of historical information is not required to authenticate current state, resolve a future merge, or interpret the current portable object, it does not belong in the normative state model.

## 13. Replica independence

A `ReplicaId` is a logical identity for one independently evolving replica. It is not a GitHub username, device clock, branch name or Android device identifier.

The model MUST permit many replicas to edit concurrently.

No merge rule may require election of a permanent leader, a central sequence allocator or a single online authority.

Repository permissions may control who can fetch or push through a particular GitHub transport, but those permissions do not alter the merge algebra.

## 14. Serialization boundary

Logical merge MUST NOT depend on how the `.apc` file happens to be physically laid out.

The native container may be compacted, re-indexed or rewritten without creating a semantic edit if the resulting logical state and cryptographic guarantees are unchanged.

Conversely, changing a logical field MUST produce new logical merge metadata even if an implementation manages to overwrite bytes in place inside the local container.

## 15. Required implementation properties

A conforming core must be testable independently of Android and GitHub.

At minimum it must support tests proving:

- atom round-trip preservation;
- scalar causal precedence;
- deterministic scalar tie-break for genuine concurrency;
- preservation of concurrent ordered insertions;
- field independence;
- merge commutativity;
- merge associativity;
- merge idempotence;
- independence from device/Git timestamps;
- correct rejection of incompatible/corrupt protected state.

These properties are stronger than a collection of example merges and should become property-based tests in the first core prototype.
