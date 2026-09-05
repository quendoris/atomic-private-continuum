# A.P.C. core implementation

Status: **implementation started; public API and portable encoding are not frozen**.

This document defines how the real portable core is allowed to grow from the validated architecture and the executable reference model. It is an implementation contract, not a replacement for `REQUIREMENTS.md`, `BOUNDARIES.md`, `LOGIC.md` or `ARCHITECTURE_STATE.md`.

## 1. Implementation language

The first portable core is implemented in Rust.

This is an implementation choice, not a portable-format semantic. The reasons are practical:

- one native core can be reused by Android and desktop clients;
- deterministic state logic can live outside either UI toolkit;
- memory safety is valuable for parsers, cryptographic framing and large binary data;
- Rust provides predictable native performance without requiring the portable model to depend on a managed runtime;
- bindings can be added later without changing the logical model.

Android remains the first application target. The binding mechanism is intentionally not selected in this first pass; JNI, UniFFI or another narrow FFI layer may be evaluated after the core surface is less fluid.

The core forbids Rust `unsafe` code by default. A future exception would require a demonstrated need, a narrowly isolated boundary and dedicated tests/audit; it must not enter merely for convenience.

## 2. Repository roles

The repository now contains two deliberately different executable layers:

```text
reference_model/          Python research/oracle models
crates/apc-core/          real portable core implementation
```

The Python reference model remains valuable after Rust implementation begins. It is used to:

- test hypotheses cheaply;
- construct adversarial counterexamples;
- provide an intentionally explicit correctness oracle for selected semantics;
- compare candidate compact representations before they become core commitments.

The Rust core must not copy every research object merely because it exists in `reference_model/`.

A research candidate enters `apc-core` only when its boundary is sufficiently understood to implement without freezing unresolved neighboring semantics.

## 3. First implemented slice

The first core slice intentionally contains only:

```text
opaque typed logical IDs
        |
direct-frontier scalar causal state
        |
deterministic state merge
        |
validation and algebra tests
```

Implemented identifier types currently include:

- `ContinuumId`;
- `AtomId`;
- `ReplicaId`;
- `RevisionId`.

They are distinct Rust types even when their current byte representation is identical.

The first implementation uses 256-bit opaque byte identities, consistent with the current logical design. Their byte magnitude has no temporal meaning. Canonical byte order is available only for explicitly specified deterministic tie-breaks.

The initial scalar register stores immutable revisions with direct causal frontier parents. It implements the currently validated scalar rule:

1. causal descendant dominates ancestor;
2. genuinely concurrent frontier revisions remain concurrent candidates;
3. the materialized concurrent winner is selected by canonical `RevisionId` byte order;
4. arrival order is irrelevant;
5. duplicate delivery of an identical revision is harmless;
6. reuse of one `RevisionId` for a different statement is invalid;
7. missing causal parents and causal cycles are rejected at this complete-state boundary.

This in-memory representation is **not** the portable binary format and is **not** the final checkpoint/coverage representation.

## 4. Important non-commitments

Starting the core does not close the remaining research questions.

The following are deliberately not implemented as frozen production semantics yet:

- final ordered-sequence / moved-anchor structure;
- hierarchy cycle-resolution semantics;
- lifecycle delete/restore/tombstone compaction policy;
- general strong multi-domain atomic mutation;
- final causal-membership/checkpoint encoding;
- native `.apc` binary layout;
- local crash-safe storage layout;
- concrete AEAD, nonce strategy or key hierarchy;
- concrete per-replica signing/key-evolution primitive;
- transport adapter API details;
- Android/desktop bindings.

Code must keep these seams replaceable. Convenience in the first implementation is not permission to smuggle one candidate into the portable format.

## 5. Core dependency rule

The dependency direction is:

```text
portable semantic primitives
        |
portable state model
        |
validation / merge
        |
portable storage + protection
        |
sync projection interfaces
        |
platform / transport bindings outside core semantics
```

The core must not import GitHub concepts, Android lifecycle concepts, UI coordinates or platform keystore identities into its logical types.

Transport revision identifiers, platform session state and portable logical revision identities remain separate types and layers.

## 6. Validation rule

Untrusted or incomplete state must fail closed at the semantic boundary.

The first scalar implementation therefore rejects:

- unknown direct parents when importing a complete register;
- causal cycles;
- conflicting statements reusing the same `RevisionId`.

A future baseline-aware capsule importer may legitimately accept a revision whose parent body is absent when that dependency is covered by an authenticated retained baseline/checkpoint. That behavior belongs to a different import boundary and must be explicit. The ordinary complete-state register must not silently guess that a missing parent is safe.

## 7. Test obligations

Every merge primitive promoted into the real core must have tests for the algebraic properties required by `LOGIC.md` where they apply:

- determinism;
- commutativity;
- associativity;
- idempotence.

It must also test domain-specific invariants and adversarial invalid state.

The initial scalar suite covers:

- causal successor precedence even when its ID sorts below an ancestor;
- deterministic concurrent tie-break;
- a post-merge join revision observing the complete concurrent frontier;
- commutative, associative and idempotent merge on valid states;
- stale-state merge not rolling back a causal descendant;
- rejection of missing parents;
- rejection of conflicting `RevisionId` reuse;
- rejection of causal cycles.

Reference-model differential/property testing should be added as soon as the Rust representation is broad enough to exchange deterministic test fixtures with the Python oracle.

## 8. Performance posture

Correctness comes before low-level optimization in the first core pass.

The initial scalar implementation may use straightforward graph walks. It must not introduce clocks, counters with hidden semantic ordering, lossy ancestry guesses or early format commitments merely to optimize an in-memory prototype.

Once oracle parity is established, benchmarks may identify hot paths and justify indexes, caches or compact representations that preserve exactly the same logical result.

Large attachment paths are a separate streaming problem and must not be benchmarked by loading complete binaries into this scalar model.

## 9. Immediate implementation sequence

The next core work should proceed in this order unless new experiments invalidate a boundary:

1. **Identity + scalar causal primitive** — started in `apc-core`.
2. **Core state shell** — `ContinuumState`, stable atom identity and declared merge-domain containers without freezing unresolved domain implementations.
3. **Working-state boundary** — crash-safe working epoch model separated from portable causal revision finalization.
4. **Finalization boundary** — reserved stable `RevisionId`, immutable finalized statement and exposure bookkeeping.
5. **Portable storage abstraction** — durability contract and crash tests before selecting/finalizing the `.apc` physical encoding.
6. **Cryptographic protection** — select studied primitives and implement real authenticated protection; no fake security API should escape as production behavior.
7. **Sync projection layer** — dirty-domain partial state, protected capsule boundary and baseline/dependency handling.
8. **First transport adapter** — GitHub optimistic publication/retry above the generic protected sync interface.
9. **Android binding + minimal Continuum client** — only after the core can create, persist, reload and deterministically merge a small real continuum.

Sequence, hierarchy and lifecycle work can enter earlier if their research blockers are resolved, but they must not hold the entire core hostage: the implementation should be modular enough to advance stable layers independently.

## 10. Definition of the first meaningful core milestone

The first real milestone is not a UI screenshot.

A.P.C. Core reaches its first meaningful executable milestone when two independent processes can:

1. create the same continuum baseline;
2. make independent valid changes using real core types;
3. persist and reload those states through a crash-safe local boundary;
4. exchange protected portable state without plaintext transport semantics;
5. merge in either order;
6. produce the same logical result;
7. detect corrupt, incomplete or conflicting protected state rather than guessing.

Only then does the first Android editor become an integration client of an existing core rather than the place where core semantics are invented.
