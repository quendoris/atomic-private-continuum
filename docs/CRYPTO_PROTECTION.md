# A.P.C. authenticated protection — implementation design

Status: **first concrete construction selected for implementation; portable cryptographic encoding not frozen**.

This document narrows the open authenticated-encryption boundary in `SECURITY.md` without changing the separation established by `KEY_EVOLUTION.md`.

It defines the first real protection primitive used by the Rust implementation. It does **not** yet freeze the native `.apc` cryptographic envelope, password/key-unlock flow, replica signing construction, trust enrollment, replay policy or content-key epoch registry.

## 1. Security responsibilities remain separate

The first protection implementation is responsible only for symmetric authenticated protection of one already-constructed plaintext unit.

It is not responsible for:

- user passwords or passphrases;
- Android Keystore / StrongBox wrapping;
- biometric authorization;
- replica signatures or replica key evolution;
- transport authentication to GitHub;
- merge ordering;
- replay/rollback detection;
- membership or authorization policy.

The dependency direction remains:

```text
clear A.P.C. state/projection
        |
canonical protection context
        |
symmetric AEAD protection
        |
opaque protected bytes
        |
storage or transport
```

Signing/authentication evolution and content confidentiality remain independent.

## 2. First AEAD construction

The first Rust implementation uses **XChaCha20-Poly1305** with:

- a 256-bit symmetric key;
- a fresh 192-bit nonce for every encryption;
- a 128-bit authentication tag as defined by the construction;
- authenticated additional data (AAD) for non-secret semantic binding.

This is an implementation selection, not yet an immutable portable-format promise.

### Why XChaCha20-Poly1305 is a strong fit for A.P.C.

A.P.C. has many independent offline writers and cannot rely on a single durable global nonce counter without creating another synchronization/coordination dependency.

The extended 192-bit nonce space permits a fresh nonce to be generated independently for each protected unit with negligible collision probability when a cryptographically secure random source is used. This avoids making nonce allocation depend on:

- wall clocks;
- merge order;
- a globally synchronized counter;
- a replica-global counter that must be transactionally advanced before every encryption;
- transport state.

The primitive is implemented by multiple interoperable libraries, including libsodium and RustCrypto.

RustCrypto's current `chacha20poly1305` crate provides `XChaCha20Poly1305` and has undergone an external security audit with no significant findings reported by the crate maintainers.

### Important standardization caveat

XChaCha20-Poly1305 does not currently have the same final RFC status as RFC 8439 ChaCha20-Poly1305. The construction is documented by an expired IETF draft and deployed by interoperable implementations.

Therefore A.P.C. will not freeze XChaCha20-Poly1305 as the permanent portable algorithm merely because the first implementation uses it.

Before format freeze the project should require at least:

1. byte-for-byte interoperability tests against an independent implementation such as libsodium;
2. review of the final envelope/context binding;
3. review of target-platform library support;
4. a deliberate comparison against any better standardized misuse-resistant construction available at that time.

## 3. Nonce rule

For this construction, one `(key, nonce)` pair MUST NOT be reused for two different plaintext/AAD inputs.

The first implementation generates the 192-bit nonce from the operating-system cryptographic random source for every protection call.

The nonce is public and stored alongside ciphertext.

Nonce bytes:

- MUST NOT encode wall-clock time;
- MUST NOT encode merge precedence;
- MUST NOT be derived from `RevisionId` magnitude;
- MUST NOT be a hidden semantic counter;
- MUST NOT be supplied by a transport adapter.

A deterministic nonce API must not be exposed to ordinary callers. Fixed nonces may exist only in test-only code for known-answer/interoperability fixtures.

## 4. Content keys and epochs

The AEAD layer accepts one 256-bit content-protection key supplied by a higher layer.

The protection primitive does not define that key to be:

- a password hash;
- a replica signing key;
- an Android hardware key;
- a GitHub credential;
- a stable logical identity.

A future content-key epoch registry may map protected objects to epoch keys conceptually as:

```text
content epoch E0 -> key K0 -> historical protected units
content epoch E1 -> key K1 -> later protected units
```

Moving to `E1` does not require rewriting units protected under `E0`.

The first implementation deliberately does not standardize the epoch identifier/registry encoding yet.

## 5. Associated-data binding

Encryption without semantic context is too easy to misuse even when the ciphertext itself is authentic.

Every protection call therefore receives explicit caller-provided context bytes. The AEAD authenticates:

```text
A.P.C. protection domain separator
        +
construction/envelope version binding
        +
caller context
```

The caller context is non-secret but integrity-protected.

Higher-level canonical encodings will eventually bind fields such as:

- continuum identity;
- protection purpose (`local recovery`, `sync capsule`, `attachment chunk`, etc.);
- content-key epoch identity;
- logical object/capsule/chunk identity;
- multipart publication identity and part index/count where applicable;
- relevant portable encoding version.

The first low-level API does not invent these higher-level structures. It requires explicit context and treats it as opaque bytes.

A ciphertext authenticated for one context MUST fail to decrypt under another context.

## 6. First development protected envelope

The initial Rust implementation may use a small pre-format envelope sufficient to test real AEAD behavior:

```text
magic
version
algorithm identifier
192-bit nonce
ciphertext || authentication tag
```

The header is either structurally fixed or included in the authenticated binding so algorithm/version substitution cannot silently reinterpret the ciphertext.

This development envelope is not the native `.apc` format and may change before format freeze.

The existing `APCDEV01` CRC envelope in `apc-storage-fs` remains a torn-write/development corruption detector only. CRC is not authentication and does not replace AEAD.

During the transition, an end-to-end development storage path may legitimately be:

```text
LocalScalarSnapshot
        |
pre-format scalar encoding
        |
AEAD protection
        |
development CRC/recovery framing
        |
filesystem durability protocol
```

The outer CRC can later disappear or change. The security property comes from AEAD, not CRC.

## 7. Failure behavior

Unprotection MUST fail closed when any authenticated input differs, including:

- wrong key;
- wrong context;
- modified nonce;
- modified ciphertext;
- modified authentication tag;
- unsupported/invalid envelope version or algorithm;
- truncation.

Authentication failure MUST NOT return partial plaintext.

Errors should not distinguish which secret-dependent authentication component was wrong when that distinction could become an oracle.

## 8. Replay and rollback are separate problems

AEAD proves that a protected unit was created by a holder of the relevant symmetric key and has not been modified under the authenticated context.

AEAD alone does **not** prove that the unit is the newest valid state.

An attacker or stale transport may replay an older but perfectly authentic ciphertext.

Replay/rollback treatment therefore remains a state/protocol problem involving causal state, authenticated replica statements, retained baselines/checkpoints and eventually authorization/key-epoch policy.

The encryption layer MUST NOT claim rollback protection merely because authentication succeeds.

## 9. Key material in memory

The Rust implementation should:

- avoid `Debug` output containing key bytes;
- zeroize owned raw key material on drop where the selected library/runtime permits;
- avoid unnecessary key copies;
- keep key export explicit rather than accidental through generic serialization;
- never write raw content keys into ordinary diagnostics.

This is memory-hygiene hardening, not a claim that keys or plaintext can be eliminated from RAM while in active use.

## 10. Required tests before promotion

The first AEAD implementation must test at least:

- plaintext round-trip;
- empty and large payloads within the implementation boundary;
- distinct protection calls producing distinct nonces/ciphertexts for the same key/plaintext/context;
- wrong key rejection;
- wrong context rejection;
- nonce tamper rejection;
- ciphertext/tag tamper rejection;
- truncation and trailing/malformed envelope rejection;
- no plaintext returned on authentication failure;
- deterministic parsing of the pre-format header;
- key debug output redaction;
- protected filesystem round-trip of a real A.P.C. recovery snapshot.

Before portable format freeze, add independent interoperability vectors against libsodium or another implementation.

## 11. Immediate implementation scope

The next implementation slice is intentionally small:

1. add a separate Rust protection crate so concrete cryptography does not contaminate merge semantics;
2. implement XChaCha20-Poly1305 with OS-generated nonces and mandatory AAD context;
3. define a pre-format protected envelope with strict fail-closed parsing;
4. integrate one real `LocalScalarSnapshot<Vec<u8>>` storage round-trip through AEAD before the filesystem backend;
5. keep key hierarchy, password KDF, platform wrapping and replica signatures out of this slice.

This gives A.P.C. real authenticated encryption without pretending that the complete key architecture is already solved.
