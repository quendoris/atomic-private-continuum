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
