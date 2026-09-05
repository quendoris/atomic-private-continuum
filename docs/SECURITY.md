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

## 10. Synchronization protection invariant

End-to-end protection continues to apply when A.P.C. uses partial sync projections, capsules, attachment chunks or multipart publications instead of a complete native container.

The optimization boundary MUST be:

```text
clear logical state inside A.P.C. core
        |
construct mergeable projection
        |
protect projection/chunk
        |
transport adapter
```

A transport adapter MUST NOT require plaintext content or plaintext merge-domain values in order to poll, publish, retry, split, resume or compact transport state.

GitHub filenames, commits, refs and API requests may reveal unavoidable transport metadata such as object count, approximate protected sizes and access timing. They MUST NOT intentionally reveal protected user content or semantic block names.

Splitting one logical publication into several transport files MUST NOT weaken cryptographic binding between the parts and the logical publication. A receiver must authenticate and validate the required parts before exposing the merged state.

Repeated synchronization encryption/decryption is an expected foreground workload. Performance optimization may reduce redundant protection work, but MUST NOT bypass end-to-end protection merely to save CPU, battery or transport bytes.

The concrete authenticated-encryption construction, nonce strategy, key hierarchy and replay/rollback treatment remain open until selected and tested. Reference-model test doubles that model an opaque transport boundary are not cryptographic implementations and MUST NOT be described as providing security.

## 11. Current authenticated-protection implementation

The first real symmetric protection implementation now exists in `crates/apc-crypto/`.

It uses XChaCha20-Poly1305 with a 256-bit key, fresh 192-bit OS-generated nonce for each encryption and mandatory caller-supplied associated-data context.

This choice is recorded in detail in `CRYPTO_PROTECTION.md`. It is a concrete implementation choice for the current Rust core, not yet a frozen portable-format algorithm identifier or complete key hierarchy.

The extended nonce was selected specifically so independent offline replicas do not need a shared counter, wall clock or transport ordering merely to allocate safe nonces. Nonces remain public physical cryptographic inputs and carry no merge/time semantics.

The current protected envelope:

- rejects wrong keys and wrong contexts;
- rejects nonce, ciphertext and authentication-tag modification;
- rejects malformed, truncated or trailing envelope bytes;
- returns no partial plaintext after authentication failure;
- redacts owned key material from `Debug` output;
- zeroizes the owned raw content-key buffer on drop.

A real `LocalScalarSnapshot<Vec<u8>>` is now serialized by the pre-format recovery codec, protected by this AEAD layer, committed through the development filesystem durability backend, reopened, authenticated/decrypted, decoded and restored to the original core state in CI.

The existing outer `APCDEV01` CRC framing is still only a development torn-write/corruption detector. It is not security and may be removed later.

The following remain open and MUST NOT be inferred from the existence of the AEAD primitive:

- password/passphrase KDF and unlock format;
- content-key epoch registry and wrapping;
- Android hardware-backed local key wrapping;
- replica signing/key evolution;
- trust enrollment and revocation;
- replay/rollback protection;
- final canonical AAD structures for native storage, capsules and chunks;
- final portable protected-envelope encoding.

AEAD authenticity does not imply freshness. Replaying an old but valid protected state remains a state/protocol problem.
