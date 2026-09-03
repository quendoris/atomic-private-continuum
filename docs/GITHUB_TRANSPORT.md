# A.P.C. GitHub transport

Status: transport design and experimental target. This document does not define A.P.C. portable semantics.

## 1. Foreground interactive synchronization

The Android client can run GitHub synchronization entirely inside the normal foreground application process. No Android daemon, foreground service, WorkManager job, or background worker is required while the user has A.P.C. open.

```text
foreground -> start SyncSession -> poll/publish/merge -> background -> cancel SyncSession
```

On foreground entry the client immediately checks the remote transport revision and catches up before continuing normal foreground polling.

No correctness property depends on synchronization running while the application is backgrounded.

## 2. GitHub transports sync capsules, not necessarily the native container

The local continuum remains one native A.P.C. state/container from the user's point of view.

GitHub does not need to store or retransmit that complete native file for every edit. The adapter may publish independently protected A.P.C. state capsules containing only the mergeable state that changed since the last published projection.

This is a transport optimization, not Git-specific data semantics:

```text
local .apc
   |
A.P.C. core determines dirty merge domains
   |
export generic protected sync capsules
   |
GitHub adapter packs/publishes those capsules
```

The GitHub adapter must not understand text, blocks, lists, conflict policy or lifecycle meaning. It only handles opaque protected capsules and transport bookkeeping.

A complete native `.apc` file may still be used for initial bootstrap when its size is practical.

## 3. State fragments rather than keystroke history

A GitHub sync payload must not become an event log merely because Git is versioned storage.

For sustained edits to one block, the core may repeatedly update the local durable state while the foreground publisher keeps only the newest required mergeable projection of that dirty domain.

Example:

```text
local durable states:
A -> B -> C -> D -> E

before remote publication:
coalesce to the current mergeable domain state E
```

The capsule must retain enough causal/lifecycle metadata for correct merge, but it need not preserve every intermediate keystroke as transport data.

This makes adaptive typing publication natural.

## 4. Detecting remote changes

The client should poll a small branch/ref/commit marker, not payload files themselves.

Authenticated conditional requests should use `ETag` / `If-None-Match` where supported.

If the transport head is unchanged, no capsule data is fetched.

If it changed, the adapter determines which opaque transport files were introduced since the client's last incorporated transport revision and fetches only those files.

GitHub does not provide a direct repository-ref realtime subscription suitable for an arbitrary foreground phone without adding a webhook receiver/relay. Polling is therefore the baseline GitHub-only mechanism.

## 5. Adaptive publication cadence

Local durability is immediate; GitHub publication is coalesced.

The first experimental policy should be adaptive rather than fixed:

- publish shortly after a quiet/idle boundary for discrete edits;
- during sustained typing, keep replacing/coalescing the dirty merge-domain projection locally rather than publishing each intermediate state;
- enforce a maximum pending age so continuous typing still propagates within the intended several-second window;
- permit faster publication when both peers appear active and the transport has capacity;
- back off when network conditions or GitHub rate limits require it.

A user-visible delay of several seconds is normal for this transport. Up to roughly ten seconds during sustained typing is acceptable for the initial target.

The values are implementation parameters and will be selected from two-device measurements.

## 6. Payload sizing and GitHub limits

GitHub limits are transport limits and MUST NOT become A.P.C. format limits.

The adapter therefore uses adaptive payload sizing.

A protected sync projection may be emitted as one capsule when small or partitioned into multiple independently valid transport parts when large.

For normal Git objects the adapter must stay below GitHub's per-object limit with margin rather than target the exact boundary. A practical experimental target can be tens of MiB per transport part, with the actual value selected from measured upload reliability and latency.

Large attachment changes should be represented by independently protected chunks so a single changed binary region does not force unrelated state to be retransmitted.

Conceptually:

```text
one logical sync publication
        |
        +-- capsule 0   structured state
        +-- capsule 1   attachment chunk
        +-- capsule 2   attachment chunk
        +-- ...

all parts remain opaque to GitHub
```

Splitting transport payloads does not split the user's continuum and does not change A.P.C. merge semantics.

## 7. Multi-terabyte local continua

A multi-terabyte A.P.C. continuum can still use GitHub as an incremental synchronization channel when participating replicas already possess a valid baseline.

Example:

```text
phone / workstation A        phone / workstation B
      3 TB local .apc              3 TB local .apc
              \                    /
               \                  /
                 GitHub
          only changed capsules
```

The unchanged multi-terabyte base never needs to cross GitHub merely because a small block changed.

This does not mean GitHub can bootstrap an arbitrary multi-terabyte continuum from nothing. Initial transfer and incremental synchronization are separate concerns. A large initial baseline may be transferred over LAN, removable storage or another bulk transport, after which GitHub can carry subsequent deltas.

## 8. Publication races

GitHub still has one mutable branch/ref head, so simultaneous publishers can race at the transport level even when their capsule filenames are unique.

The adapter handles this with optimistic retry:

```text
read remote head R
create opaque capsule(s)
prepare commit based on R
attempt fast-forward publish
        |
        +-- success
        |
        +-- head changed
              -> read newest head
              -> fetch/incorporate missing capsules locally
              -> retain local pending state
              -> prepare a new transport commit
              -> retry
```

GitHub never performs A.P.C. semantic merge.

The retry only produces a repository state containing all transport payloads that the application has locally reconciled.

## 9. Immutable capsule naming

Transport payload files should use opaque stable identifiers rather than semantic filenames or clocks.

For example:

```text
sync/
  7f...c1.apcs
  a2...19.apcs
  f9...e0.apcs
```

A filename may be derived from or include a cryptographically strong capsule identifier. It must not encode wall-clock ordering or user content.

Git commit SHAs remain transport revision identifiers only. They do not become A.P.C. logical IDs.

## 10. Long-offline replicas and compaction

If GitHub retained every historical capsule forever, repository history would grow without bound. Therefore transport compaction is required eventually.

Compaction must operate on mergeable state rather than simply deleting arbitrary old events.

A future GitHub transport generation may contain a compact current sync projection/checkpoint plus only state needed after that checkpoint. Because A.P.C. merge is state-based, a checkpoint may summarize many earlier publications without preserving every intermediate transport revision.

Compaction safety depends on lifecycle and compact causal metadata, both of which are still active research topics.

A very old replica must either:

- be able to merge with the retained compact state, or
- be told that its transport baseline is outside the retained generation and that a new bootstrap is required.

This policy belongs to the transport lifecycle, not to the user's document semantics.

## 11. Encryption boundary

Every GitHub payload is protected before publication.

Independent capsule/chunk protection is preferable to whole-continuum re-encryption because it allows small edits to produce small transport changes. The exact cryptographic construction is not selected here.

Transport packing must not weaken A.P.C. security boundaries merely to improve Git delta compression.

## 12. Experimental measurements

The first two-device GitHub experiment should measure:

- local durability latency;
- idle-to-publication latency;
- maximum publication age during continuous typing;
- publication-to-remote-detection latency;
- remote-detection-to-render latency;
- capsule count and bytes per logical edit burst;
- percentage of repeated edits coalesced before publication;
- request count and rate-limit state;
- simultaneous publication retries;
- attachment chunk throughput;
- behavior when a publication is partitioned into multiple transport files;
- recovery after one client is offline for minutes, hours and days.
