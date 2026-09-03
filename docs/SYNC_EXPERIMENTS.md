# A.P.C. synchronization experiments — round 1

Status: active research. These experiments validate sync semantics and boundaries; they do not freeze transport encoding or cryptographic primitives.

Executable cases live in `tests/test_sync_lab.py` and use `reference_model/sync_lab.py`.

## 1. Goal

The first sync lab tests whether A.P.C. can keep three independent properties separated:

```text
local durability
remote publication cadence
merge correctness
```

A local edit may be committed immediately, several local edits may be coalesced before transport publication, and remote merge must still converge independently of transport arrival order.

## 2. Adaptive publication

The lab contains an intentionally simple publication gate with two boundaries:

- publish after a short idle interval;
- publish after a maximum pending age even if editing is continuous.

The current deterministic experiment uses values such as:

```text
idle boundary      1 s
maximum pending    5-8 s
```

These are experiment parameters, not product constants.

The important property is that continuous typing cannot postpone synchronization forever, while nearby keystrokes do not become separate GitHub mutations.

## 3. Dirty-domain coalescing

Fifty sequential local edits to one merge domain produce one dirty sync-domain projection rather than fifty transport events.

This confirms the intended transport model:

```text
many local durable states
        |
        +-- one current dirty merge domain
                |
           one sync projection
```

The current reference `ScalarRegister` still retains explicit causal ancestry internally. Therefore this experiment proves event-count coalescing, not yet byte-size compaction.

Compact causal metadata remains necessary before production synchronization can obtain the full size benefit.

## 4. Publication acknowledgement race

A publication acknowledgement must not clear a dirty domain if that domain changed again while the publication was in flight.

The lab records the exact register state included in the projection. On acknowledgement, the dirty flag is cleared only if the current merge-domain state still matches the published state.

Thus:

```text
export A
   |
local edit B
   |
ack A
```

leaves the domain dirty and B remains pending.

This is required for invisible foreground synchronization because UI editing cannot stop while GitHub publication is running.

## 5. Duplicate and out-of-order delivery

Two replicas can concurrently edit the same scalar domain, export independent state projections and deliver them in arbitrary order, including duplicates.

The receiver converges using the normal A.P.C. scalar merge rules. Transport arrival order does not define the winner.

Independent dirty domains also merge without interfering with one another.

This supports using GitHub commits and filenames only as transport bookkeeping rather than semantic ordering.

## 6. End-to-end protection boundary

Every transport-facing sync object must already be protected.

The lab makes this an API boundary:

```text
clear SyncProjection
      |
partition / assemble logical publication
      |
protect inside A.P.C. sync boundary
      |
ProtectedSyncPart
      |
transport adapter
```

`MemoryOpaqueTransport` refuses clear sync parts and accepts only `ProtectedSyncPart` objects.

The test protector is deliberately **not cryptography**. It is a non-cryptographic in-memory test double that replaces clear objects with random opaque tokens so the architecture can test the boundary without inventing a production cipher.

No security conclusion may be drawn from that test double. Production synchronization still requires audited authenticated encryption and key management selected under `SECURITY.md`.

The property being tested is narrower and important: transport code must have no API path that requires plaintext user state.

## 7. Multipart atomic visibility

A logical publication may be split into several protected transport parts.

The receiver is not allowed to expose a partial publication as complete. The experiment delivers a three-part publication:

- out of order;
- with a duplicate part;
- with only two of three parts present for part of the test.

No sync projection is returned for semantic merge until all required parts are present and consistent.

This is the first executable form of the rule that GitHub object-size limits may change transport packing but must not split A.P.C. semantics.

## 8. Current result

The sync-capsule direction survives the first executable pass:

1. adaptive batching can bound propagation delay without publishing keystrokes individually;
2. dirty-domain projections can coalesce local edit bursts;
3. acknowledgement can be race-safe while editing continues;
4. duplicate and out-of-order transport delivery does not affect logical convergence;
5. the transport interface can be kept plaintext-blind;
6. multipart transport can remain invisible until the logical publication is complete.

The largest unresolved cost remains causal metadata: the current correctness oracle may place one domain into one capsule while still carrying quadratic explicit ancestry inside that domain.

## 9. Next sync experiments

The next pass should attack:

- compact ID-based causality against the explicit-ancestor oracle;
- overlapping publications from the same replica while an earlier publication is still in flight;
- simultaneous publishers racing on one GitHub head;
- missing multipart data and retry/resume behavior;
- attachment chunk reachability and atomic visibility;
- checkpoint/capsule compaction with a replica offline across several generations;
- adaptive batching under simulated network latency and GitHub backoff;
- protection overhead once a real candidate AEAD/key hierarchy is selected.

Sequence research remains independent and continues in parallel.
