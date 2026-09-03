# A.P.C. synchronization — design draft

Synchronization is optional. The local A.P.C. state remains authoritative for local work and must remain fully usable offline.

## 1. Transport role

A synchronization transport moves an already protected native A.P.C. object between replicas.

It does not:

- interpret user content;
- resolve semantic conflicts;
- decide which edit is newer using server time;
- define A.P.C. identity or authorization semantics;
- become a mandatory dependency of the format.

GitHub is the first planned transport because it provides convenient distribution, version retention and repository access control. These are transport properties, not A.P.C. format semantics.

The initial GitHub transport stores one native A.P.C. file as the synchronized object.

## 2. Merge location

All semantic merge occurs inside the A.P.C. core.

Conceptually:

```text
local .apc             remote .apc
     |                      |
     +------ decrypt --------+
                |
          validate states
                |
          A.P.C. merge
                |
             encrypt
                |
        publish new .apc
```

Git merge is not A.P.C. merge.

GitHub does not merge A.P.C. atoms. It only distributes repository revisions of the encrypted native file.

## 3. Publication races

Concurrent publication is expected behavior.

If a replica attempts to publish against an outdated remote repository revision, the transport adapter must be able to:

1. retrieve the newest remote `.apc` file;
2. validate and merge it locally with the pending local state;
3. produce a new protected native file;
4. retry publication against the new repository revision.

The result must not depend on which replica happened to upload first except where a defined deterministic scalar tie-break rule produces that same result on all replicas.

## 4. Replica count

The synchronization model MUST NOT encode an architectural assumption that only a small number of replicas or users exist.

The design should remain semantically valid with thousands of participating replicas. Performance optimizations may introduce practical limits in particular implementations, but those limits must not be hidden correctness assumptions in the format.

## 5. Authorization

For the initial GitHub transport, repository permissions may determine who can retrieve or publish transport state.

Those permissions are not part of A.P.C. portable data semantics.

The portable format should nevertheless avoid design choices that would make future cryptographic principals, capabilities or scoped authorization impossible to add compatibly.

## 6. One synchronized object

The synchronized repository payload is one native `.apc` file.

Internal atom boundaries, indexes, attachment chunks and cryptographic regions exist inside the format. GitHub is not expected to understand or merge them.

The implementation therefore has to make one-file synchronization efficient enough for the intended scale without leaking format semantics into Git.

Repository history is transport history. It is not the A.P.C. data model and is not required for correct semantic merge.

## 7. No trusted transport clock

Commit timestamps, server timestamps and device clocks must not participate in semantic merge ordering.

Git commit identifiers may identify transport revisions, but they are not atom IDs and do not define causal precedence inside the portable A.P.C. model.

## 8. Failure behavior

A temporary synchronization failure MUST NOT block local editing.

Pending local changes must remain durable locally and may be synchronized later.

A failed or interrupted synchronization must never replace a valid local state with an unvalidated remote state.

## 9. Runtime model: foreground only

The Android implementation does not require a daemon, Android foreground service, WorkManager job, alarm, or persistent background worker for normal interactive synchronization.

While at least one A.P.C. activity is visible/foreground, an in-process `SyncSession` may run as ordinary application work:

```text
application enters foreground
        |
        +-- immediate remote-head check
        |
        +-- start foreground sync loop
        |
        +-- local edits remain immediately durable locally
        |
        +-- publish coalesced local changes
        |
        +-- detect remote revisions
        |      |
        |      +-- fetch .apc only after remote head changes
        |      +-- validate/decrypt/merge
        |      +-- update visible state
        |
application leaves foreground
        |
        +-- cancel polling, timers and network sync work
```

When the application returns to the foreground it performs an immediate catch-up check and resumes the loop.

No correctness property may depend on the loop continuing while the application is in the background. The operating system is free to suspend or kill the process after A.P.C. leaves the foreground.

Desktop implementations should use the same semantic model: synchronization is an ordinary part of the running application process, not a mandatory system daemon.

## 10. GitHub change detection

GitHub does not provide a direct realtime subscription to a repository branch for an arbitrary foreground phone client without introducing an externally reachable webhook receiver or relay.

The first GitHub adapter therefore uses efficient foreground polling.

The polling request MUST NOT download the native `.apc` file on every cycle. It should first query only a small transport revision marker, such as the current branch/ref/commit identity.

Conceptually:

```text
poll remote HEAD/ref
        |
        +-- unchanged -> do nothing
        |
        +-- changed
               |
               +-- fetch encrypted .apc
               +-- merge locally
               +-- render new state
```

Authenticated conditional HTTP requests with `ETag` / `If-None-Match` should be used where supported. An unchanged `304 Not Modified` response avoids unnecessary payload transfer and, under GitHub's documented rules for correctly authenticated conditional REST requests, does not consume the primary REST rate-limit budget.

Polling still has to respect GitHub secondary limits and any endpoint-specific `x-poll-interval` guidance. The adapter must apply backoff on `403`, `429`, `retry-after`, or other explicit rate-limit responses.

## 11. Interactive publication cadence

A.P.C. must not translate every keystroke into a GitHub commit.

Local durability and remote publication are separate boundaries:

```text
user edit
   |
   +-- durable local commit        immediate
   |
   +-- pending sync state          immediate
   |
   +-- GitHub publication          coalesced
```

The foreground adapter should coalesce nearby changes and publish at semantic/idle boundaries. Discrete edits can therefore become visible remotely quickly, while sustained typing is transmitted in batches rather than as hundreds of repository mutations.

The exact cadence is an implementation parameter and must be measured experimentally. Initial experiments should evaluate an adaptive policy roughly in this family:

- immediate or near-immediate publication after a short idle boundary;
- a minimum spacing between mutating GitHub requests;
- longer batching during sustained continuous input;
- immediate remote-head checks after a successful local publication race/retry;
- adaptive slowdown when rate-limit headers or network conditions require it.

GitHub currently recommends serial API requests, at least a short pause between mutative REST requests, and imposes secondary limits on content-generating operations. For that reason, sub-keystroke GitHub publication is explicitly not a design goal.

## 12. Expected interactive latency

GitHub synchronization is intended to feel automatic, not to pretend to be a sub-100-ms realtime collaboration channel.

When two phones have A.P.C. open in the foreground, the expected path is:

```text
phone A local edit
      |
      +-- local durable state
      +-- coalesced publish
                |
              GitHub
                |
      phone B foreground poll notices new revision
                |
      fetch -> merge -> render
```

The first practical target is **seconds-scale propagation** for discrete edits while both applications are open. Exact latency is not specified before measurement because it depends on publication batching, poll cadence, network RTT, GitHub processing time, `.apc` size and merge cost.

A dedicated two-device synchronization experiment must record at least:

- local-edit to successful-publication latency;
- publication to remote-detection latency;
- remote-detection to merged-render latency;
- total end-to-end latency;
- bytes transferred for unchanged polls and changed states;
- GitHub request counts and rate-limit headers;
- behavior under simultaneous publication races;
- behavior under sustained typing rather than isolated edits.

If a future transport needs sub-second collaboration, it may provide a push or direct peer channel without changing A.P.C. merge semantics or the native format. GitHub remains a valid durable synchronization transport even if another adapter later provides a faster live channel.
