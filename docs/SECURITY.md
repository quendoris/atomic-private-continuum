# A.P.C. security model — design draft

A.P.C. is designed to protect user information as strongly as the architecture can reasonably provide without obstructing normal competent use.

This document separates portable cryptographic guarantees from platform hardening.

## 1. Security boundary

A.P.C. is responsible for data while that data remains under A.P.C. control.

If the user deliberately exports plaintext to another application or service, that copy is outside the A.P.C. protection boundary.

A.P.C. must not attempt to compensate for arbitrary unsafe behavior outside that boundary by disabling ordinary editing, clipboard or export capabilities.

## 2. Persistent plaintext

Sensitive A.P.C. content MUST NOT be intentionally persisted in plaintext.

This requirement applies to:

- primary user content;
- portable metadata whose disclosure would reveal protected content;
- synchronization payloads;
- sensitive auxiliary logs where logging is enabled.

Temporary plaintext required for editing and rendering will necessarily exist in process memory. The implementation should minimize unnecessary copies and lifetime, but MUST NOT claim that plaintext can be cryptographically eliminated while it is actively displayed or edited.

## 3. Network boundary

All sensitive A.P.C. payloads MUST be encrypted before being handed to a transport.

The security model MUST assume that:

- the network is untrusted;
- GitHub or another storage provider is untrusted with respect to plaintext;
- repository contents may become public;
- old synchronized versions may remain available indefinitely.

Transport confidentiality therefore cannot be the only encryption layer.

## 4. Cryptographic separation

The architecture MUST keep these responsibilities distinct:

1. content confidentiality;
2. authenticity/integrity of changes;
3. local device unlock and local key protection;
4. transport authentication.

A key or mechanism used for one responsibility MUST NOT silently become the semantic definition of another responsibility.

In particular, Android biometrics and hardware-backed keystores protect local access but do not define the portable A.P.C. encryption format.

## 5. Multi-replica key evolution

A single global linear next-key chain is unsuitable for concurrent replicas because two replicas may legitimately evolve from the same prior synchronized state.

Any forward-secure signing or key-evolution design MUST therefore support concurrent replicas without turning valid parallel work into a cryptographic fork.

A promising direction is independent per-replica signing evolution under portable space-level trust metadata, but no concrete construction is standardized yet.

Before selection, the construction must specify at least:

- how a replica is introduced;
- how its current signing state is authenticated;
- how concurrent replicas evolve independently;
- how stale or compromised historical signing material is prevented from authorizing future changes;
- how a replica can be revoked if future authorization is added;
- how verification works after long offline periods;
- how key loss affects that replica and the continuum as a whole.

No custom cryptographic primitive should be invented merely to satisfy this requirement if a well-studied construction exists.

## 6. Content-key evolution

Signing-key evolution and content encryption are separate concerns.

Rotation of signing keys MUST NOT require rewriting existing user content.

If future membership changes require new content keys, the design SHOULD permit new cryptographic epochs to apply prospectively without re-encrypting arbitrarily large historical data unless the user explicitly requests such a transformation.

## 7. Android local protection

The Android implementation SHOULD use platform security facilities when available, including hardware-backed keystore support for local wrapping or authorization keys.

Biometric authentication or device credentials may authorize local use of protected key material.

Platform facilities are additional protection layers, not portable dependencies.

If expected platform protection is degraded, for example because of device configuration, the application SHOULD display a compact dismissible notice. The notice MUST NOT block normal operation solely on that basis.

## 8. Hardening policy

Security hardening is acceptable when it reduces application-created exposure without materially damaging normal workflows.

Examples that may be appropriate:

- avoiding sensitive logs;
- preventing accidental plaintext persistence;
- minimizing exported Android components;
- protecting recent-app previews where practical;
- reducing overlay/tapjacking exposure;
- encrypting diagnostics separately.

The following are not baseline requirements:

- masquerading as another application;
- honeypot data;
- active counter-intrusion behavior;
- mandatory custom keyboards for ordinary text entry;
- complete clipboard prohibition;
- refusal to run on user-controlled devices solely because the bootloader is unlocked;
- elaborate isolated-process cryptography without a demonstrated threat reduction.

## 9. Recovery

A.P.C. MUST NOT imply recoverability that the cryptographic architecture does not actually provide.

If all required keys are lost, protected data may be permanently inaccessible.

There is no mandatory vendor account, e-mail recovery path or hidden master recovery key.
