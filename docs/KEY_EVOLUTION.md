# A.P.C. replica identity and key evolution — design draft

This document defines required logical properties for replica authentication and key evolution. It does not standardize a cryptographic primitive yet.

The purpose is to prevent the key design from accidentally imposing a single-user or globally linear history on a system that must support large numbers of concurrent replicas.

## 1. Separation of identities and keys

A stable `ReplicaId` and the replica's current signing/authentication key state are different concepts.

A replica may evolve or replace cryptographic key material without changing its stable logical identity.

A replica identifier MUST NOT be defined as "the current public key" because doing so would turn every key rotation into a new logical replica.

## 2. No global next-key chain

A continuum MUST NOT use one global chain of the form:

```text
K0 -> K1 -> K2 -> K3
```

when independent replicas are allowed to edit concurrently.

Such a chain creates an invalid fork if two replicas legitimately evolve from the same synchronized state:

```text
      K1
     /
K0
     \
      K2
```

Neither valid edit should invalidate the other merely because both were produced offline.

## 3. Independent replica evolution

Each replica evolves its own authentication state independently.

Conceptually:

```text
Replica A: A0 -> A1 -> A2 -> A3 -> ...
Replica B: B0 -> B1 -> B2 -> ...
Replica C: C0 -> C1 -> C2 -> ...
```

A transition in replica A's key state MUST NOT consume, replace or invalidate replica B's current key state.

This remains true if thousands of replicas exist.

## 4. Next-key binding

The useful part of the proposed one-time-key idea is the binding of future authentication state to current authenticated state.

A candidate transition may conceptually bind:

```text
replica_id
current_key_state_id
current_public_key
next_key_state_id
next_public_key
protected logical revision commitment
```

with an authentication proof created by the current private state.

After a transition is durably committed, the previous private state SHOULD become unusable if the selected cryptographic construction can provide that property safely.

The exact mechanism may be a studied forward-secure signature scheme, an authenticated key-evolution construction or another design. A.P.C. MUST NOT invent an ad-hoc primitive merely to imitate a ratchet.

## 5. Concurrent work

Two different replicas may publish authenticated revisions concurrently without a cryptographic conflict:

```text
A17 -> A18
B42 -> B43
```

These are independent transitions.

Their user-data merge is resolved by the A.P.C. logical merge model, not by key-chain ordering.

Authentication answers "is this state contribution valid for this replica?" It does not answer "which concurrent user edit is newer?".

## 6. Same-replica fork

Cloning one live private replica state into two independently active devices creates a real key-evolution fork and MUST NOT be treated as normal collaboration.

Therefore portable onboarding should normally create a new `ReplicaId` and new private authentication state for another device rather than copy an active replica's evolving private state.

If migration of one replica identity between devices is later supported, the protocol must define how the old live instance is retired or otherwise prevented from continuing the same private chain.

This requirement is separate from copying the continuum content-encryption material required to read the same data.

## 7. Trust binding

The portable system needs a cryptographically meaningful way to distinguish recognized replica authentication state from arbitrary forged replica records.

That trust binding is not the same as application-level permissions.

The initial GitHub implementation may continue to rely on repository permissions for transport access. The format does not need a full roles/capabilities system merely to authenticate replica state.

However, the portable structure MUST leave room for future explicit principals, revocation and scoped capabilities without replacing replica identity semantics.

The exact trust-root/enrollment construction remains open.

## 8. Verification after offline periods

A replica may remain offline for a long period while other replicas evolve through many key states.

Verification MUST NOT require access to private historical keys.

The portable state must retain, or be able to derive, enough authenticated public transition information to validate the currently accepted key state from a trusted prior state.

This information should be compactable where cryptographically safe. It is authentication metadata, not an application activity log.

## 9. Signing and content encryption are independent

Forward evolution of replica authentication keys MUST NOT cause bulk re-encryption of continuum content.

Content confidentiality uses a separate key hierarchy.

Likewise, rotating a content-encryption epoch MUST NOT silently redefine replica identity or merge ordering.

## 10. Content-key epochs

If future collaboration semantics require excluding a previously authorized reader from future plaintext, A.P.C. may introduce prospective content-key epochs.

Conceptually:

```text
content epoch E0 -> historical protected content
content epoch E1 -> later protected content
```

Changing epoch does not imply rewriting all historical content.

Historical re-encryption, if ever offered, is an explicit transformation rather than a normal consequence of key rotation.

## 11. Loss behavior

Loss of one replica's current private authentication state may make that replica unable to produce further authenticated revisions.

It MUST NOT automatically make the whole continuum unreadable if the continuum content keys remain available elsewhere.

Loss of all required content key material may make the continuum permanently unreadable, as defined in the security model.

## 12. Properties required before algorithm selection

A concrete key-evolution mechanism cannot become normative until it demonstrates:

- independent evolution for concurrent replicas;
- no dependence on wall-clock time;
- authenticity of the current replica key state;
- resistance to use of retired historical private key state for future authentication, to the extent promised by the chosen primitive;
- verification after long offline periods;
- bounded or safely compactable public transition metadata;
- explicit handling of accidental same-replica forks;
- compatibility with a new-replica onboarding flow;
- separation from content encryption and transport authentication;
- availability of well-reviewed cryptographic primitives and libraries on the target platforms.
