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

The repository now contains deliberately different executable layers:

```text
reference_model/          Python research/oracle models
crates/apc-core/          portable semantic core
crates/apc-crypto/        real authenticated protection primitive
crates/apc-storage-fs/    development Unix durability backend
```

The Python reference model remains valuable after Rust implementation begins. It is used to:

- test hypotheses cheaply;
- construct adversarial counterexamples;
- provide an intentionally explicit correctness oracle for selected semantics;
- compare candidate compact representations before they become core commitments.

The Rust semantic core must not copy every research object merely because it exists in `reference_model/`.

A research candidate enters production-facing Rust code only when its boundary is sufficiently understood to implement without freezing unresolved neighboring semantics.

Concrete cryptography, storage and transport adapters remain separated from merge semantics even when they are implemented in Rust and live in the same workspace.

## 3. Implemented core slices

### 3.1 Typed opaque identities

Implemented identifier types currently include:

- `ContinuumId`;
- `AtomId`;
- `ReplicaId`;
- `RevisionId`;
- `WorkingEpochId`.

They are distinct Rust types even when their current byte representation is identical.

The first implementation uses 256-bit opaque byte identities, consistent with the current logical design. Their byte magnitude has no temporal meaning. Canonical byte order is available only for explicitly specified deterministic tie-breaks.

`WorkingEpochId` is intentionally device-local crash-recovery identity. It is not portable causal identity and never participates in merge ordering.

### 3.2 Direct-frontier scalar causal state

The initial scalar register stores immutable revisions with direct causal frontier parents. It implements the currently validated scalar rule:

1. causal descendant dominates ancestor;
2. genuinely concurrent frontier revisions remain concurrent candidates;
3. the materialized concurrent winner is selected by canonical `RevisionId` byte order;
4. arrival order is irrelevant;
5. duplicate delivery of an identical revision is harmless;
6. reuse of one `RevisionId` for a different statement is invalid;
7. missing causal parents and causal cycles are rejected at this complete-state boundary.

This in-memory representation is **not** the portable binary format and is **not** the final checkpoint/coverage representation.

### 3.3 Continuum and atom state shell

`ContinuumState<A>` and `AtomMap<A>` provide stable continuum/atom identity without freezing unresolved atom-domain internals.

The shell already enforces:

- distinct atom identities coexist;
- shared atom identity delegates merge to the atom payload's `MergeState` implementation;
- different `ContinuumId` values cannot merge;
- local duplicate `AtomId` creation is rejected;
- absence from `AtomMap` is not deletion semantics.

Lifecycle, location, hierarchy and ordered-sequence semantics remain outside this shell until their research blockers are resolved.

### 3.4 Durable working-state boundary

`WorkingScalar<T>` implements the validated separation:

```text
crash-safe local working state
        !=
portable causal revision
```

A working epoch captures the causal frontier actually observable when the epoch begins. Repeated durable value updates do not create portable revisions. Sealing converts the latest pending value into one revision using that captured frontier.

If remote state is about to become semantically observable while local work is pending, the core requires a pre-observation seal. It does not allow the later remote frontier to be substituted as if the older local work had observed it.

The current tests include 10,000 pending value updates coalescing into one causal revision and explicit preservation of true concurrency across a remote observation boundary.

`WorkingSnapshot<T>` preserves pending value, `WorkingEpochId` and original observed frontier together for crash recovery. Restore also rejects a pending frontier that does not match the causal state stored in the same recovery object; malformed recovery state cannot later be sealed into invented ancestry.

### 3.5 Finalization and exposure boundary

`FinalizationLedger<T>` tracks the distinction between:

```text
local causal identity
        |
finalized immutable statement
        |
transport handoff / external exposure
```

Finalization freezes the semantic statement (`RevisionId`, value and direct causal parents) but deliberately contains no signature or replica key-evolution construction yet.

Finalization is idempotent for the same statement and rejects a rewritten statement under an already-finalized `RevisionId`.

Transport handoff requires every locally owned causal identity in the transitive dependency closure to be finalized first. Exposure is recorded at handoff, not acknowledgement.

`FinalizationSnapshot<T>` preserves finalized and exposed bookkeeping for crash recovery.

### 3.6 Local scalar domain state machine

`LocalScalarDomain<T>` composes the working and finalization layers so callers cannot accidentally seal a local revision without registering local ownership, or restore working state while forgetting finalization/exposure state.

Its current path is:

```text
begin/update working epoch
        |
seal local revision
        |
register local causal identity
        |
optional finalize
        |
transport handoff marks exposure
```

Remote observation uses the same state machine and automatically registers any pre-observation local revision created during the apply boundary.

This is still one scalar merge-domain implementation, not a general final atom type.

### 3.7 Durability acknowledgement protocol

`DurabilityBackend<S>` and `commit_durable()` define the backend-independent local commit ordering:

```text
write complete candidate
        |
sync candidate data
        |
publish candidate as committed root
        |
sync committed root
        |
ACK success
```

Crash-injection tests verify the contract around every boundary: before publication only the old root must remain visible; after unsynchronized publication either old or new complete state may survive; after the committed-root barrier the new state must survive; after `commit_durable()` returns success the old state must not reappear.

### 3.8 Development filesystem backend and recovery path

`apc-storage-fs` is the first concrete implementation of the durability contract. It is intentionally a development Unix backend, not the native `.apc` format.

It persists immutable candidate objects and publishes one candidate through a small root manifest. Candidate file and directory entries are synchronized before publication; the replacement root manifest is synchronized before rename; the containing directory is synchronized before commit acknowledgement.

The backend is single-writer for now and deliberately keeps local candidate numbering outside logical semantics.

Candidate files use a temporary deterministic `APCDEV01` recovery envelope with payload length and CRC-32 so truncation and accidental corruption fail closed. The CRC is only a development corruption detector and is not security.

A second explicitly pre-format codec serializes `LocalScalarSnapshot<Vec<u8>>` deterministically. It preserves causal revisions, a pending working epoch and its observed frontier, local revision ownership, finalized statements, exposure and handoff bookkeeping as one recovery object. The codec validates the core state before encoding and after decoding.

A concrete filesystem test persists a real local scalar domain containing finalized/exposed state plus a later pending draft, closes the backend, reopens it and restores a domain equal to the original.

A separate subprocess worker is force-killed after candidate write, candidate sync, root publication, committed-root sync and the acknowledgement path. The parent independently reopens the store and checks the recovered state. This validates real process death while preserving the important distinction that process death is not power loss.

The detailed durability contract and physical-development status are recorded in `DURABILITY.md`.

### 3.9 Real authenticated protection

`apc-crypto` now provides the first real cryptographic protection boundary.

The current implementation uses XChaCha20-Poly1305 with:

- a 256-bit content-protection key;
- a fresh OS-generated 192-bit nonce for each encryption;
- mandatory caller-provided associated-data context;
- strict pre-format envelope parsing;
- owned raw key bytes zeroized on drop and redacted from `Debug`.

The extended nonce avoids introducing a global nonce counter, wall clock or transport-order dependency across independent offline replicas.

The low-level protection API deliberately does not implement passwords, Android key wrapping, replica signatures, transport credentials, replay/rollback policy or merge semantics.

Tests reject wrong keys, wrong contexts, nonce/ciphertext/tag modification, malformed headers, truncation and trailing bytes. Authentication failure returns no partial plaintext.

The algorithm/envelope is not yet frozen as the permanent portable format. `CRYPTO_PROTECTION.md` records the rationale, standardization caveat and requirement for independent interoperability testing before format freeze.

A full development local recovery path is now executable:

```text
LocalScalarDomain
        |
LocalScalarSnapshot
        |
pre-format deterministic scalar encoding
        |
XChaCha20-Poly1305 authenticated protection
        |
development filesystem recovery framing
        |
durable candidate/root commit
        |
close + reopen
        |
authenticate/decrypt
        |
decode + core validation
        |
restored LocalScalarDomain
```

The integration test also verifies that sensitive draft plaintext is not present in the protected bytes written into the filesystem payload and that the same bytes cannot be authenticated under a different recovery context.

## 4. Important non-commitments

Starting the core does not close the remaining research questions.

The following are deliberately not implemented as frozen production semantics yet:

- final ordered-sequence / moved-anchor structure;
- hierarchy cycle-resolution semantics;
- lifecycle delete/restore/tombstone compaction policy;
- general strong multi-domain atomic mutation;
- final causal-membership/checkpoint encoding;
- native `.apc` binary layout;
- production local crash-safe physical storage layout;
- final portable AEAD/envelope algorithm commitment and content-key epoch registry;
- password/passphrase KDF and portable unlock/wrapping format;
- Android hardware-backed local key wrapping;
- concrete per-replica signing/key-evolution primitive;
- replay/rollback policy;
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

The semantic core must not import GitHub concepts, Android lifecycle concepts, UI coordinates or platform keystore identities into its logical types.

Transport revision identifiers, platform session state, cryptographic nonces and portable logical revision identities remain separate types and layers.

## 6. Validation rule

Untrusted or incomplete state must fail closed at the semantic/protection boundary.

The current implementation rejects:

- unknown direct parents when importing a complete scalar register;
- causal cycles;
- conflicting statements reusing the same `RevisionId`;
- reuse of an already-known `RevisionId` when sealing local work;
- remote semantic observation of dirty work without a pre-observation seal;
- inconsistent recovered pending working frontiers;
- finalization of a revision not registered as local;
- mutation of an already-finalized statement;
- transport handoff that depends on an unfinalized local causal identity;
- inconsistent finalization crash snapshots;
- invalid development filesystem root manifests;
- truncated or corrupted development candidate envelopes;
- malformed, truncated, duplicate or structurally invalid pre-format scalar recovery snapshots;
- empty AEAD context;
- wrong AEAD key/context and authenticated-byte modification;
- malformed, truncated or trailing protected-envelope data.

A future baseline-aware capsule importer may legitimately accept a revision whose parent body is absent when that dependency is covered by an authenticated retained baseline/checkpoint. That behavior belongs to a different import boundary and must be explicit. The ordinary complete-state register must not silently guess that a missing parent is safe.

## 7. Test obligations

Every merge primitive promoted into the real core must have tests for the algebraic properties required by `LOGIC.md` where they apply:

- determinism;
- commutativity;
- associativity;
- idempotence.

It must also test domain-specific invariants and adversarial invalid state.

The current Rust suite covers scalar causality, continuum/atom state composition, working-epoch coalescing and observation boundaries, finalization immutability, causal-ancestor handoff requirements, restoration of combined working/finalization snapshots, the abstract durability crash matrix, deterministic recovery encoding, corruption rejection, a real subprocess-kill matrix, authenticated-encryption tamper/context/key rejection and an AEAD-protected real scalar filesystem round-trip.

Reference-model differential/property testing should be added as soon as the Rust representation is broad enough to exchange deterministic test fixtures with the Python oracle.

Independent XChaCha20-Poly1305 interoperability fixtures must be added before portable protection encoding is frozen.

## 8. Performance posture

Correctness comes before low-level optimization in the first core pass.

The initial scalar implementation may use straightforward graph walks. It must not introduce clocks, counters with hidden semantic ordering, lossy ancestry guesses or early format commitments merely to optimize an in-memory prototype.

Repeated AEAD protection/deprotection is expected foreground work and must eventually be benchmarked, but protection must not be bypassed to save CPU, battery or bytes.

Once oracle parity is established, benchmarks may identify hot paths and justify indexes, caches or compact representations that preserve exactly the same logical result.

Large attachment paths are a separate streaming/chunk-protection problem and must not be benchmarked by loading complete binaries into the scalar model.

## 9. Immediate implementation sequence

The next core work should proceed in this order unless new experiments invalidate a boundary:

1. **Identity + scalar causal primitive** — implemented.
2. **Core state shell** — implemented for `ContinuumState` / `AtomMap`.
3. **Working-state boundary** — implemented for scalar domains.
4. **Finalization boundary** — implemented for scalar domains, without selecting replica signing cryptography.
5. **Durability protocol + development filesystem backend** — substantially implemented, including pre-format recovery encoding, real scalar recovery and subprocess-kill testing. Concrete filesystem failure injection, repeated stress and safe orphan reclamation remain.
6. **Authenticated symmetric protection** — first real XChaCha20-Poly1305 implementation and protected recovery integration implemented; final key hierarchy/portable encoding/interoperability remain open.
7. **Sync projection layer** — next stable core direction: remove remaining research-only projection identity ordering, define dirty-domain protected projection boundaries and baseline/dependency handling without freezing unresolved checkpoint encoding.
8. **First transport adapter** — GitHub optimistic publication/retry above the generic protected sync interface.
9. **Android binding + minimal Continuum client** — after the core can create, persist, reload, protect and deterministically merge a small real continuum.

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

The current implementation has now demonstrated most of the single-process/local-storage half of this milestone, including real authenticated protection. The remaining milestone-critical work is primarily protected cross-process projection exchange, deterministic merge through that exchange and the transport-independent capsule/dependency boundary.

Only then does the first Android editor become an integration client of an existing core rather than the place where core semantics are invented.
