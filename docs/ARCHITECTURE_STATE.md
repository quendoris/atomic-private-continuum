# A.P.C. architecture state

Status: **pre-implementation; architecture research remains active**.

This document is the current handoff/index for A.P.C. It summarizes what is already a project invariant, what has strong executable evidence, what remains a research candidate, and what is still open. It does not replace the normative or specialist documents listed below.

The project is sufficiently constrained to begin a real core implementation, but it is **not** ready for a portable-format freeze. Implementation must preserve open seams where research is not finished.

## 1. Document authority

When documents overlap, use the following order:

1. `REQUIREMENTS.md` — normative observable requirements.
2. `BOUNDARIES.md` — ownership of responsibilities between format, core, platform and transport.
3. Specialist design documents (`LOGIC.md`, `SECURITY.md`, `CONTINUUM.md`, `UX.md`, `KEY_EVOLUTION.md`, `SYNC.md`, `SYNC_CAPSULES.md`, `GITHUB_TRANSPORT.md`, `FORMAT.md`).
4. `*_RESEARCH.md` documents and executable reference-model tests — evidence, counterexamples and candidates; not automatically frozen format semantics.
5. This file — current architectural synthesis and handoff.

If this file conflicts with a higher-authority normative requirement, the requirement wins and this file must be corrected.

Research documents can contain historically useful intermediate conclusions that later experiments supersede. The latest dedicated experiment and its tests take precedence over an older research hypothesis.

## 2. Product and user invariants

The following are already architectural requirements, not research suggestions.

### 2.1 Continuum

Normal launch/resume returns to the last valid working context rather than a dashboard or library. Content durability and local view/session durability are distinct, but both must support returning the user to their work.

A user-visible content change is either durably committed or clearly not committed. Once the durability boundary is acknowledged, process death or power loss must not silently revert the change.

### 2.2 User ownership

A.P.C. data is independently usable without GitHub, Android, a vendor account, telemetry or a recovery service.

Clipboard and export are normal user capabilities. Plaintext deliberately exported from A.P.C. is outside the A.P.C. protection boundary.

A.P.C. contains no advertising and does not require telemetry for normal operation.

### 2.3 Platform direction

Android is the first application target; desktop follows the same portable format and core semantics.

Platform UX, gestures, screen coordinates, biometrics, Android Keystore/StrongBox and lifecycle APIs must not become portable format semantics.

The primary UI direction is a vertically navigated text continuum, not an infinite two-dimensional board.

## 3. Architectural boundaries

A.P.C. is deliberately split into four semantic layers.

```text
portable format
      |
portable core
      |
platform implementation
      |
transport adapter
```

### 3.1 Format

The format defines portable user state plus the metadata required to interpret, authenticate and deterministically merge that state.

It is independent of Android, desktop UI, Git, GitHub, local hardware key stores, transport credentials and diagnostics.

### 3.2 Core

The portable core owns format parsing/writing, portable cryptography, validation, deterministic merge, import/export primitives, attachment access and generic synchronization projection interfaces.

The core does not require a network connection.

### 3.3 Platform

Platform code owns local unlock, host-specific key wrapping, editor/UI behavior, lifecycle integration, storage integration and local hardening.

A platform may strengthen local protection, but it must not redefine portable cryptography or logical merge semantics.

### 3.4 Transport

A transport moves already-protected A.P.C. synchronization material. It does not inspect plaintext content and never performs semantic merge.

GitHub is the first planned transport, not an architectural dependency.

## 4. Native continuum and physical representation

A continuum is one logical state and one user-portable native object. The exported native representation must be self-contained as a `.apc` file.

The physical container may contain indexes, chunk tables, encrypted attachment regions, integrity structures and other internal partitions. One portable file does **not** mean one semantically monolithic value or a requirement to load/rewrite the entire file for every change.

The complete native `.apc` object is **not** the mandatory incremental synchronization unit. Synchronization may carry protected mergeable capsules/chunks that represent only changed or required state. A complete `.apc` may still be used as a bootstrap object when practical.

GitHub limits must never become format limits.

The exact binary encoding, physical index design and crash-safe incremental update layout remain open.

## 5. Logical state and atomicity

The logical model is state-based. A.P.C. does not require a persistent operation/event log to define current portable state.

Conceptually:

```text
ContinuumState
├── ContinuumId
├── format version
├── root ordered collection
├── atom map
├── causal / merge metadata
└── portable cryptographic metadata
```

An `AtomId` gives a persistent information object stable identity. An atom is the smallest independently addressable persistent object, but it is not necessarily one merge unit.

A visual object may contain several independent merge domains. Examples include scalar fields, ordered collections, lifecycle state, location/parent state and immutable content references.

The central atomicity rule is:

> merge at the narrowest declared semantic domain.

A title write must not erase a concurrent child insertion. A move must not rewrite content identity. A lifecycle change must not require rewriting every content field merely because visibility changes.

For valid states, logical merge must remain deterministic, commutative, associative and idempotent.

## 6. Identifiers, time and causality

Correctness must not depend on wall-clock time, device time, Git timestamps, server timestamps or transport arrival order.

Logical identifiers are opaque identities. Their magnitude does not mean earlier or later.

Current identifier classes include `ContinuumId`, `AtomId`, `ReplicaId`, `RevisionId`, `KeyStateId` and attachment/content identities. The exact final encoding remains open.

Canonical unsigned lexicographic ID order may be used only where a deterministic total tie-break is explicitly required for genuinely concurrent states. Causal precedence always wins before such a tie-break.

A.P.C. uses a partial causal order, not a global timeline.

### 6.1 Direct-frontier causality

The explicit-all-ancestors scalar model remains a correctness oracle but is rejected as a production representation because causal references grow quadratically on a simple chain.

The strongest current ID-only causal candidate stores direct observed frontier parents. In the executable 256-revision linear case, retained causal references fell from `32640` to `255` while the tested scalar frontier/materialization behavior remained equivalent to the explicit oracle.

This is strong evidence for the representation in the tested scalar domain, not yet a proof for every merge-domain type.

### 6.2 Causal scope is merge-domain-local by default

Unrelated domains must not acquire causal relationships merely because their events occurred in one process or one network session.

A remote change in domain B does not force dirty domain A to create a revision unless A's own semantic observation context changes.

The executable 100-remote-update experiment reduced local revisions in the unrelated dirty domain from `101` under a global-observation policy to `1` under domain-local causality with the same final visible scalar value.

Strong multi-domain operations, where they genuinely exist, are a separate problem; domain-local causality does not pretend such operations are independent.

## 7. Local working state, observation and portable finalization

A.P.C. separates three boundaries:

```text
local crash-safe working state
        !=
portable causal state
        !=
transport publication
```

### 7.1 Working epochs

Many locally durable edits may be coalesced inside one working epoch. A pending epoch captures the causal frontier actually observed when the work began.

In the executable scalar experiment, `10000` durable local writes created no portable causal revision until sealing, then produced one revision.

### 7.2 Receipt is not observation

Downloading a protected remote capsule is not the same as semantically observing its content.

If a remote change in the same dirty merge domain is about to become observable, the pre-remote local working epoch must first be sealed with the frontier it actually saw. The remote state is then merged. This prevents publication time from inventing false causality.

### 7.3 Stable causal identity across finalization

A device-local `WorkingEpochId` and a portable `RevisionId` are different concepts.

However, once an unresolved local value participates in portable causal/conflict semantics, its causal/conflict identity must remain stable through finalization. Minting an unrelated fresh `RevisionId` at finalization can change a deterministic concurrent scalar winner and is rejected by the executable counterexample.

The current direction is therefore:

```text
WorkingEpochId          local durable record identity
reserved RevisionId     stable causal/conflict identity when required
private canonicalization / squashing
FinalizedStatement      immutable authenticated portable statement
protected sync material
transport handoff
```

Finalization freezes the authenticated statement fields. Safe private canonicalization should happen before finalization where possible.

### 7.4 Exposure boundary

A causal identity is conservatively considered externally exposed when a representation naming it, or a descendant still depending on it, is handed to transport — not when an acknowledgement later arrives.

An ACK can be lost after successful remote receipt.

Never-exposed dominated private causal nodes form a separate compaction class and may be squashable before finalization. Exposed causal identities require the stronger long-term compaction/checkpoint rules.

## 8. Causal compaction and long-offline replicas

Direct-frontier causality removes quadratic edge growth but does not by itself solve lifetime causal-node/membership retention.

The exact-coverage checkpoint experiment established these boundaries:

- dominated old causal node bodies can leave the hot DAG while exact covered-ID knowledge still permits tested long-offline branches to reconnect;
- logical frontier `RevisionId`s cannot be replaced by arbitrary fresh checkpoint IDs because doing so can change future concurrent tie-break results;
- if old nodes and all verifiable knowledge of their covered IDs are discarded, a branch naming an old parent becomes ambiguous and must not be guessed into an order.

`CheckpointId`, logical `RevisionId`, transport-generation identity and cryptographic commitment identity are distinct concepts.

The current exact-coverage oracle still retains linear historical membership metadata. A production solution remains open: cold exact membership, proof-backed membership, retained causal/transport generations with rebootstrap, or another demonstrated construction.

## 9. Lifecycle, location and content

Stable `AtomId` is used to prevent common actions from becoming artificial multi-domain transactions.

The tested architectural separation is:

```text
AtomId
├── lifecycle domain
├── location / parent domain
└── content domain(s)
```

A remote move can change location while dirty content remains attached to the same atom identity and does not acquire a content causal revision merely because it moved.

Lifecycle and location are different domains. Encoding deletion as `location = None` is rejected because later location activity could accidentally resurrect the atom.

The current interaction lab uses a **research-only delete-wins lifecycle candidate**. It demonstrates that a concurrent content edit or stale/concurrent move need not resurrect a deleted atom, while hidden content can remain retained internally. Delete/restore policy, hidden-content retention horizon and tombstone compaction are not frozen.

## 10. Ordered structures, movement and hierarchy

Ordered collections must preserve concurrent independent insertions, stable member identity and deterministic order without clocks or global renumbering as a correctness requirement.

The final sequence structure is **not selected**. Fugue/FugueMax-style immutable positions plus stable `AtomId` and a causal location register remain strong research candidates, but moved-anchor semantics and production metadata compaction remain open.

Container membership can be modeled as a parent/location domain on the child rather than remove+insert identity replacement. This permits concurrent moves of one child to converge to one parent without duplicating the child in the tested model.

Per-object convergence is insufficient for a hierarchy: the materialized parent graph must also satisfy global structural invariants such as cycle freedom.

### 10.1 Full-history hierarchy fallback is rejected as a production default

Full historical reactivation is useful as an adversarial research oracle but has three demonstrated problems:

- worst-case fallback work can grow linearly with retained placement history;
- rejecting one placement can activate an unrelated concurrent historical alternative that the rejected move never observed;
- literal historical fallback can exhaust all retained placements and fail to produce a valid result without an additional safe fallback.

### 10.2 One causal witness is the strongest current bounded candidate, not frozen semantics

The current bounded hierarchy candidate records the parent state causally observed before a move. If the requested parent becomes globally invalid, resolution tries that causal witness once, then a safe root/orphan fallback if the witness is also invalid.

This gives bounded hierarchy-validity work and has a stronger causal interpretation than selecting an arbitrary surviving concurrent historical placement.

It is **not yet frozen production hierarchy semantics**. More statistical/adversarial work remains, including causal-purity measurements and denser conflict shapes.

Workstation campaign evidence currently includes:

- 16 runs with `100000` atoms and `1000000` independent branch revisions each: one-witness required zero root fallbacks in the generated workloads; mean one-witness resolution was about `6.63 s` in the Python reference model;
- 32 oracle runs with `5000` atoms and `50000` independent branch revisions each: mean one-witness resolution was about `0.101 s`, while full-history mean was about `1.56 s`; the final parent graphs regularly differed.

These measurements are evidence about the current generator/reference implementation, not performance promises for the production core.

## 11. Cross-domain atomic mutation

Independent merge domains remain independent by default.

If an operation genuinely promises all-or-none semantics across several domains, merely buffering all parts and making them visible together is not enough. Two complete concurrent multi-domain mutations can later tear into a hybrid state if members are independently merged, and overlapping mutation groups can enlarge the conflict component.

Therefore strong cross-domain atomic mutation is a costly semantic primitive, not a batching convenience.

A.P.C. should prefer representations where common actions are one semantic-domain change through stable identity (for example move -> location, delete -> lifecycle, edit -> content).

The general concurrent conflict rule for truly irreducible multi-domain atomic mutations remains open.

## 12. Synchronization architecture

Synchronization is optional and does not change local-first authority.

The generic path is:

```text
local native state
      |
dirty merge domains
      |
clear A.P.C. sync projection inside trusted core/sync layer
      |
protect / authenticate
      |
opaque protected capsule(s) / attachment chunks
      |
transport adapter
```

Sync capsules are partial mergeable state, not operation-log events and not Git commits.

Repeated local edits may be coalesced before publication when future merge behavior is preserved.

Multipart publication must not become semantically visible until all required protected parts have been authenticated, validated and assembled.

Transport adapters operate on opaque protected objects and must not require plaintext merge-domain values for polling, publication, retry, splitting or resume.

### 12.1 Foreground runtime

The normal Android synchronization target is an in-process foreground `SyncSession`.

Entering foreground triggers immediate catch-up. While foreground, the session polls/publishes adaptively. Backgrounding cancels normal polling/timers/network sync work.

No correctness property depends on a daemon, Android foreground service, WorkManager job or background worker.

A several-second propagation delay is acceptable; sustained typing may be coalesced with a maximum pending age. Approximate values such as 1-second idle and 5–8-second maximum age are experiment parameters, not format constants.

### 12.2 GitHub transport

GitHub transports opaque protected sync material. It never performs A.P.C. merge.

The adapter should detect a small remote head/ref marker, fetch only newly required protected material, and use optimistic compare-and-swap / fast-forward publication. If the head changed, the client fetches missing material, merges locally, retains local pending state and retries.

Git commit identity is transport bookkeeping only.

Initial bootstrap and incremental synchronization are separate problems. Very large continua may be bootstrapped through LAN/removable media/another bulk channel and then use GitHub only for incremental protected capsules.

Transport history eventually requires generations/checkpoints/retention policy. A valid very-old replica must either remain mergeable from retained state or be explicitly re-bootstrapped without silently dropping its unsynchronized edits.

## 13. Security and key architecture

Sensitive persistent A.P.C. content is encrypted at rest. Sensitive synchronization material is protected before transport handoff. A provider or transport may be fully untrusted with respect to plaintext.

The following responsibilities are separate:

1. content confidentiality;
2. authenticity/integrity of portable contributions;
3. local platform unlock/key protection;
4. transport authentication/authorization.

Android biometrics/Keystore/StrongBox are local protection layers, not portable content keys or portable semantics.

The concrete AEAD, nonce strategy, key hierarchy, replay/rollback treatment and authentication primitive are still open and must use studied cryptography rather than ad-hoc constructions.

### 13.1 Replica identity and key evolution

A stable `ReplicaId` is separate from the replica's current authentication/signing key state.

One global next-key chain is rejected because valid concurrent replicas would fork it. The required direction is independent per-replica authentication evolution:

```text
Replica A: A0 -> A1 -> A2 ...
Replica B: B0 -> B1 -> B2 ...
```

A same-replica clone/fork is a real key-evolution problem; onboarding another device should normally create a new `ReplicaId` rather than clone one live private replica state.

Signing/authentication evolution and content-key evolution are separate. Signing rotation must not force bulk re-encryption. Prospective content-key epochs may be used in future without rewriting arbitrarily large historical ciphertext as a normal consequence of rotation.

The concrete trust-root/enrollment/revocation mechanism remains open.

### 13.2 Recovery boundary

A.P.C. does not promise a hidden recovery authority. Loss of all required content key material may mean permanent data loss.

Root/unlocked devices are not automatically refused. Reduced local protection should be communicated compactly rather than turning security policy into an obstacle to competent use.

## 14. Scale requirements

The semantic model must not impose arbitrary limits on atom count, continuum size, attachment size or participating replica count.

Large attachments must support chunked/lazy access and must not require loading the complete object into RAM.

GitHub transport limits are provider-specific and must be handled by transport packing/chunking rather than by changing the portable format's semantic limits.

The architecture must remain semantically valid for thousands of replicas even if a particular implementation later documents practical performance limits.

## 15. Explicitly rejected assumptions

The following should not be silently reintroduced during implementation:

- wall-clock/device/server/Git timestamps as merge authority;
- opaque ID magnitude as recency;
- a process-wide/global causal timeline for independent merge domains;
- explicit all-ancestor sets as the production causal encoding;
- persistent event sourcing as the required portable state model;
- one global signing/key-evolution chain across concurrent replicas;
- deletion encoded as location absence;
- move implemented as content remove+reinsert with new identity;
- Git/GitHub semantic merge;
- the complete `.apc` file as the mandatory unit of every incremental sync;
- publication of one transport object per keystroke;
- network receipt as semantic observation;
- transport acknowledgement as the first exposure boundary;
- replacing a semantically active `RevisionId` with an arbitrary fresh ID during finalization/compaction;
- full-history hierarchy fallback as the production default;
- assuming atomic delivery automatically gives atomic concurrent merge semantics;
- platform hardware keys as portable format dependencies;
- correctness that depends on background synchronization;
- forced clipboard/export blocking or active counter-intrusion behavior as core security.

## 16. Open decisions that still block a format freeze

The following remain deliberately unresolved:

- final identifier construction/encoding;
- production compact causal membership/checkpoint strategy for very old baselines;
- final ordered-sequence structure and moved-anchor semantics;
- deletion versus edit policy, restore semantics, tombstone stabilization and compaction;
- final hierarchy cycle-resolution semantics and bounded fallback policy;
- truly irreducible cross-domain atomic mutation semantics;
- native `.apc` binary layout, indexes, integrity tree/framing and crash-safe incremental update strategy;
- attachment chunking, lazy verification, deduplication/privacy policy and large-object layout;
- concrete AEAD, nonce/key hierarchy, replay/rollback handling;
- concrete per-replica authentication/key-evolution primitive and trust/enrollment model;
- transport checkpoint/generation retention protocol;
- compatibility negotiation and canonical cross-implementation test vectors;
- optional future A.P.C.-level authorization/capabilities.

These must remain implementation seams rather than being accidentally frozen by convenience code.

## 17. What implementation may begin now

A real core implementation may begin around the already-stable boundaries:

- typed logical identifiers with no clock semantics;
- `ContinuumState` / atom / merge-domain abstractions;
- scalar causal oracle plus pluggable compact-causality representation;
- deterministic validation and merge interfaces;
- local durable working-state abstraction separate from portable revision finalization;
- format/container abstraction without freezing the final byte layout;
- portable-crypto interfaces without inventing final primitives;
- generic clear sync projection -> protected sync object boundary;
- transport adapter interface with no plaintext requirements;
- attachment streaming/chunk abstraction;
- property/adversarial test harness independent of Android and GitHub.

The first implementation must treat unresolved sequence, hierarchy, lifecycle-compaction, checkpoint and cryptographic mechanisms as replaceable modules behind explicit contracts.

UI work can proceed against these contracts, but UI behavior must not be allowed to define format semantics.

## 18. Immediate next work

The next phase is two parallel tracks rather than "research finished":

1. **specification consolidation** — keep normative documents synchronized with validated results and remove stale assumptions;
2. **core implementation** — begin the real implementation around stable contracts while continuing focused experiments on the remaining freeze blockers.

The next hierarchy statistics should add causal-purity measurements: how often a fallback is the causal predecessor actually observed by the rejected move versus an unrelated historical/concurrent alternative. The campaign should continue to report witness-to-root frequency, disagreement counts and adversarial failures rather than only runtime.

The reference model remains a falsification/oracle environment. It is not the production core and must not be allowed to become the format by accident.
