# A.P.C. synchronization — design draft

Synchronization is optional. The local A.P.C. state remains authoritative for local work and must remain fully usable offline.

## 1. Transport role

A synchronization transport moves already protected A.P.C. data between replicas.

It does not:

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
local protected state   remote protected state
          |                       |
          +------ decrypt --------+
                     |
               validate states
                     |
               A.P.C. merge
                     |
                  encrypt
                     |
                  publish
```

Git merge is not A.P.C. merge.

## 3. Publication races

Concurrent publication is expected behavior.

If a replica attempts to publish against an outdated remote revision, the transport adapter must be able to:

1. retrieve the newest remote state;
2. validate and merge it locally with the pending local state;
3. produce a new protected result;
4. retry publication.

The result must not depend on which replica happened to upload first except where a defined deterministic scalar tie-break rule produces that same result on all replicas.

## 4. Replica count

The synchronization model MUST NOT encode an architectural assumption that only a small number of replicas or users exist.

The design should remain semantically valid with thousands of participating replicas. Performance optimizations may introduce practical limits in particular implementations, but those limits must not be hidden correctness assumptions in the format.

## 5. Authorization

For the initial GitHub transport, repository permissions may determine who can retrieve or publish transport state.

Those permissions are not part of A.P.C. portable data semantics.

The portable format should nevertheless avoid design choices that would make future cryptographic principals, capabilities or scoped authorization impossible to add compatibly.

## 6. Logical object and transport representation

A continuum is one logical A.P.C. object.

A user-facing native export may be one `.apc` file.

A transport adapter may use a different opaque representation when required for efficient synchronization of very large continua or attachments. Such a representation must be lossless and reconstruct the same logical A.P.C. state.

The decision whether the GitHub implementation can efficiently use exactly one repository file at all supported scales remains open and must be validated against Git/GitHub behavior before it is made normative.

## 7. No trusted transport clock

Commit timestamps, server timestamps and device clocks must not participate in semantic merge ordering.

Git commit identifiers may identify transport revisions, but they are not atom IDs and do not define causal precedence inside the portable A.P.C. model.

## 8. Failure behavior

A temporary synchronization failure MUST NOT block local editing.

Pending local changes must remain durable locally and may be synchronized later.

A failed or interrupted synchronization must never replace a valid local state with an unvalidated remote state.
