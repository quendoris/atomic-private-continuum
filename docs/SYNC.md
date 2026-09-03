# A.P.C. synchronization — design draft

Synchronization is optional. The local A.P.C. state remains authoritative for local work and must remain fully usable offline.

## 1. Transport role

A synchronization transport moves protected A.P.C. synchronization material between replicas.

The transport unit does not have to be the complete native `.apc` container. A transport may carry independently protected mergeable state capsules representing only state that has changed or is required for bootstrap/recovery.

```text
portable A.P.C. state
        |
A.P.C. sync projection
        |
protected mergeable capsules
        |
transport adapter
        +-- GitHub
        +-- LAN
        +-- removable media
        +-- another future transport
```

The transport adapter does not:

- interpret user content;
- resolve semantic conflicts;
- decide which edit is newer using server time;
- define A.P.C. identity or authorization semantics;
- become a mandatory dependency of the format.

GitHub is the first planned transport because it provides convenient distribution, version retention and repository access control. These are transport properties, not A.P.C. format semantics.

## 2. Merge location

All semantic merge occurs inside the A.P.C. core.

Conceptually:

```text
local state             remote capsules/state
     |                         |
     +------ validate/decrypt --+
                  |
             A.P.C. merge
                  |
           durable local state
                  |
       emit new sync projection
                  |
           protect and publish
```

Git merge is not A.P.C. merge.

A transport may carry a complete protected native file for small/simple deployments, but correctness must not depend on that representation.

## 3. State capsules, not an event log

Efficient synchronization MUST NOT require A.P.C. to become an event-sourced format.

A sync capsule is a partial state that can be merged into another valid state. It may contain, for example:

- the current merge-domain state of changed atoms or fields;
- lifecycle/tombstone state required to prevent resurrection;
- causal metadata required to compare the included state correctly;
- newly referenced encrypted attachment chunks;
- format and integrity information needed to validate the capsule.

Repeated edits to the same merge domain may be coalesced before publication. A typing session should normally publish the newest mergeable state for the dirty domain, not one record per keystroke.

Capsule application should inherit the merge algebra of the contained state: duplicate delivery must be harmless, and publication order must not define semantic order.

Diagnostic logs and user-visible history remain separate from synchronization state.

## 4. Adaptive publication

Local durability is independent from remote publication.

Each confirmed local change is made durable locally according to Continuum requirements. A foreground sync session maintains a local dirty set of merge domains that have changed since their last published projection.

Publication is adaptive rather than keystroke-driven:

- short isolated edits may be published shortly after an idle boundary;
- sustained typing is coalesced into larger updates;
- a maximum pending age prevents continuous input from postponing synchronization indefinitely;
- large binary changes may be chunked independently from small structured changes;
- transport-specific payload limits are applied only after the generic sync projection has been produced.

The first Android experiments should target user-visible propagation on the order of seconds. Up to roughly ten seconds is acceptable for sustained editing. Exact debounce, maximum-age and polling values are measurements, not format constants.

## 5. Publication races

Concurrent publication is expected behavior.

If a replica attempts to publish against an outdated transport revision, the adapter must be able to:

1. retrieve the newest remote transport state;
2. retrieve any capsules not yet incorporated locally;
3. validate/decrypt and merge them locally;
4. retain the local pending state;
5. publish the resulting local sync projection against the new transport revision.

The transport service does not perform semantic merge.

## 6. Foreground interactive synchronization

The Android implementation may run its interactive synchronization loop entirely inside the ordinary foreground application process.

No background daemon is required for normal operation.

```text
foreground
   -> start SyncSession
   -> detect / publish / receive / merge
background
   -> stop SyncSession
```

On return to foreground, the client immediately catches up from the last known transport revision.

No correctness property depends on background synchronization.

Desktop implementations should use the same semantic model: synchronization is ordinary work performed by the running application, not a mandatory system daemon.

## 7. Change detection

A transport should expose a small revision marker so a client can determine whether anything changed before fetching payload data.

For GitHub this may be the current branch/ref/commit identity with conditional HTTP requests where supported.

The client should retrieve only newly published transport capsules after detecting a new transport revision. It should not retransmit or redownload the whole native continuum merely because one atom changed.

## 8. Replica count

The synchronization model MUST NOT encode an architectural assumption that only a small number of replicas or users exist.

The design should remain semantically valid with thousands of participating replicas. Performance optimizations may introduce practical limits in particular implementations, but those limits must not be hidden correctness assumptions in the format.

## 9. Authorization

For the initial GitHub transport, repository permissions may determine who can retrieve or publish transport state.

Those permissions are not part of A.P.C. portable data semantics.

The portable format should nevertheless avoid design choices that would make future cryptographic principals, capabilities or scoped authorization impossible to add compatibly.

## 10. No trusted transport clock

Commit timestamps, server timestamps and device clocks must not participate in semantic merge ordering.

Transport revision identifiers may identify transport states, but they are not atom IDs and do not define causal precedence inside the portable A.P.C. model.

## 11. Failure behavior

A temporary synchronization failure MUST NOT block local editing.

Pending local changes must remain durable locally and may be synchronized later.

A failed or interrupted synchronization must never replace a valid local state with an unvalidated remote state.

## 12. Bootstrap and long-offline replicas

Incremental synchronization and initial data transfer are separate problems.

A new replica may be bootstrapped by the selected transport when the data size permits. Very large continua may instead be bootstrapped locally, over LAN, removable media or another high-volume channel and then use a low-bandwidth transport such as GitHub only for subsequent state capsules.

The synchronization model must therefore support a replica beginning from an already valid local baseline and catching up from protected state capsules without requiring the complete continuum to be retransmitted.

Transport compaction/checkpointing must preserve the ability of valid long-offline replicas to catch up, or explicitly require a new bootstrap when the selected transport retention policy can no longer support that replica. This is a transport lifecycle issue, not a change to A.P.C. merge semantics.

## 13. Interactive latency experiment

A dedicated two-device experiment must record at least:

- local-edit to successful-publication latency;
- publication to remote-detection latency;
- remote-detection to merged-render latency;
- total end-to-end latency;
- bytes transferred for unchanged polls and changed capsules;
- request counts and transport rate-limit state;
- behavior under simultaneous publication races;
- behavior under sustained typing rather than isolated edits;
- batching efficiency: logical edits per published capsule and bytes per changed merge domain.
