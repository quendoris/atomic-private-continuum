# A.P.C. GitHub transport

Status: transport design and experimental target. This document does not define A.P.C. portable semantics.

## 1. Foreground interactive synchronization

The Android client can run GitHub synchronization entirely inside the normal foreground application process. No Android daemon, foreground service, WorkManager job, or background worker is required while the user has A.P.C. open.

The intended lifecycle is:

```text
foreground -> start SyncSession -> poll/publish/merge -> background -> cancel SyncSession
```

On foreground entry the client immediately checks the remote transport revision and catches up before continuing normal foreground polling.

No correctness property depends on synchronization running while the application is backgrounded.

## 2. Detecting remote changes

The client should poll a small branch/ref/commit marker, not the `.apc` payload itself.

Authenticated conditional requests should use `ETag` / `If-None-Match` where the endpoint supports them. The `.apc` file is fetched only after the remote transport revision changes.

GitHub does not provide a direct repository-ref realtime subscription suitable for an arbitrary foreground phone without adding a webhook receiver/relay. Polling is therefore the baseline GitHub-only mechanism.

## 3. Publication

Local durability is independent from GitHub publication.

A local edit is committed durably to the local A.P.C. state immediately. Nearby edits are then coalesced for transport publication. The adapter must not create a GitHub repository revision for every keystroke.

Publication races are normal:

```text
publish against remote R
       |
       +-- success
       |
       +-- remote changed -> fetch newest -> APC.merge -> republish
```

All semantic conflict resolution remains inside A.P.C.

## 4. Latency target

The GitHub adapter targets automatic **seconds-scale** propagation for discrete edits while two clients are simultaneously open.

A first two-device experiment should begin with approximately one-second foreground change detection and adaptive publication batching. Isolated edits may publish shortly after an idle boundary; sustained editing must be batched more aggressively to avoid excessive GitHub mutations.

No fixed latency is guaranteed before measurement.

The experiment must measure edit-to-publish, publish-to-detect, detect-to-render, total latency, payload bytes, request counts, rate-limit headers, simultaneous publication races, and sustained-input behavior.

## 5. GitHub operational limits are transport limits

A.P.C. itself has no small-file architectural limit. GitHub does.

Current GitHub documentation enforces a 100 MiB maximum for a normal Git object and recommends repositories remain substantially smaller than the largest values Git can technically accept. Git LFS supports larger single objects but currently has per-file maximums measured in gigabytes, depending on plan, rather than arbitrary size.

Therefore a native `.apc` file that exceeds the selected GitHub transport's supported size cannot be synchronized through that adapter as a single GitHub-hosted object.

This does **not** change the A.P.C. format limit and must not cause the core to fragment the user's continuum to satisfy GitHub. The GitHub adapter must expose the transport limitation and another transport may be selected for larger data.

The one-file GitHub experiment is therefore valid for files within GitHub's supported envelope. Large-continuum transport remains a separate engineering problem.

## 6. Encryption consequence

For useful Git-based incremental transfer, small logical edits should not randomize the entire ciphertext representation of the native file.

The portable container should use independently authenticated encrypted regions/chunks so unchanged regions can remain byte-stable across revisions where the security design permits it. Whole-container re-encryption after every edit would make Git delta transfer ineffective and would make interactive synchronization cost scale with total continuum size.

The cryptographic construction for those regions is not selected here; this is a transport-derived requirement that must be reconciled with `SECURITY.md` and the eventual container specification.
