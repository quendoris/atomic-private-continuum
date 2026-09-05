# A.P.C. format — design draft

This document records format-level decisions that are sufficiently stable to constrain implementation. Exact binary encoding, cryptographic algorithms and merge structures are intentionally not fixed yet.

## 1. Native object

An A.P.C. continuum is one portable native object from the user's and format's point of view.

The native representation MUST be self-contained and exportable as one `.apc` file. Internal indexing, chunk tables, attachment regions or other structures may exist inside that single file. They do not change the one-object model.

The format must support efficient partial access and incremental durable update without treating the continuum as one semantically monolithic value.

The complete native `.apc` file is **not** the mandatory unit of incremental synchronization. A synchronization transport may carry protected mergeable state capsules, attachment chunks or other transport-independent partial projections defined by A.P.C. sync semantics. A complete `.apc` may still be used for bootstrap when practical.

Transport/provider limits, including GitHub limits, MUST NOT become native-format limits.

## 2. Atoms

The format is composed of independently addressable atoms.

An atom has at minimum:

- a stable atom identifier;
- a type identifier;
- content or references to content;
- structural relationships where applicable;
- merge metadata sufficient for deterministic convergence;
- cryptographic envelope metadata required by the selected format version.

Atom identifiers MUST NOT be derived from wall-clock time.

The exact identifier construction remains open. It must provide collision resistance at the scale expected from arbitrary numbers of devices and users without requiring a central allocator.

## 3. Fields and collections

A visual object such as a sticker is not necessarily one merge unit.

Example logical structure:

```text
sticker
├── title        scalar field
├── body         structured content
└── children     ordered collection
    ├── item A
    ├── item B
    └── item C
```

Different components may have different merge rules.

A scalar replacement and an insertion into an ordered collection are distinct operations over distinct merge domains. A concurrent title replacement must not erase a concurrent child insertion.

## 4. Deterministic convergence

A.P.C. MUST converge without relying on device clocks.

If one state causally descends from another, that relationship may determine precedence.

If two scalar values are genuinely concurrent and incompatible, every implementation MUST select the same result using a stable deterministic rule based on format metadata rather than local arrival order or wall-clock time.

For ordered collections, concurrent independent insertions MUST remain present. Their order MUST also converge deterministically.

The exact data structure used to implement these properties remains open. Candidate structures must be evaluated against scale, metadata growth, deletion behavior, reordering and implementation complexity before selection.

## 5. Deletion

Deletion semantics are not yet fixed.

A deletion design must account for:

- replicas that remain offline for long periods;
- concurrent edits to deleted content;
- safe compaction of deletion metadata;
- deterministic convergence after rejoining.

A simple physical delete is insufficient for concurrent replicas.

## 6. Attachments

Binary attachments are first-class content referenced by atoms.

The logical model MUST support attachments larger than available RAM and MUST permit lazy access.

A large attachment may be represented inside the native object by an encrypted manifest and independently addressable encrypted chunks or regions. The exact internal chunking scheme is not fixed yet.

A PDF, image or other attachment does not change the surrounding atom model. Notes may exist before, after or structurally adjacent to the attachment without embedding editor-specific layout into the portable format.

## 7. Encryption boundary

Persistent sensitive content is encrypted as part of the portable format.

Platform key stores may protect local access to portable key material, but platform-specific key references MUST NOT be required to interpret the portable format.

Content encryption, author/integrity authentication and local platform unlock are separate responsibilities and MUST NOT be conflated.

## 8. Versioning

Every native A.P.C. object MUST identify the format version required to interpret it.

Future versions MUST provide a defined compatibility policy. Unknown optional extensions SHOULD be skippable where doing so does not change the meaning or security of known content.

Critical extensions MUST be distinguishable from optional extensions.

## 9. Not part of the format

The following are not portable format semantics:

- Git commit hashes or branch names;
- GitHub usernames or repository permissions;
- Android Keystore aliases;
- UI coordinates tied to a particular screen;
- diagnostic logs;
- caches;
- dismissed warning state unless explicitly required for Continuum restoration.
