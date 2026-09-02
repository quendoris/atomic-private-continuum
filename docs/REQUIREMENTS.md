# A.P.C. requirements

This document records current normative requirements. It describes required behavior, not implementation choices.

Keywords **MUST**, **MUST NOT**, **SHOULD** and **MAY** are used normatively.

## 1. Continuity

1. On launch, the application MUST return to the last working location rather than opening a dashboard by default.
2. The restored state MUST include enough information to reproduce the user's working position, including the active continuum, viewport position and editing position where applicable.
3. A user-visible change MUST be either durably committed or clearly not committed. There MUST NOT be an acknowledged state that exists only in volatile memory.
4. Sudden process death or device power loss after a committed change MUST NOT lose that committed change.

## 2. Atomicity

1. Persistent user information MUST be represented as independently addressable atoms rather than one semantically monolithic document value.
2. Every persistent atom MUST have a stable identifier that is independent of device clocks.
3. Scalar fields and ordered collections MUST have separate merge semantics.
4. Concurrent additions to the same ordered collection MUST be preserved unless they are explicitly deleted later.
5. Concurrent incompatible writes to one scalar field MUST converge deterministically on every conforming implementation.

## 3. Time independence

1. Wall-clock timestamps MUST NOT be used to decide merge precedence.
2. Device time MUST NOT be trusted as an ordering authority.
3. Causal relationships and stable identifiers MAY be used to determine precedence and deterministic tie-breaking.
4. Human-readable timestamps MAY exist as user metadata but MUST NOT affect correctness.

## 4. Privacy and storage

1. Sensitive persistent A.P.C. content MUST be encrypted at rest.
2. Sensitive content MUST be encrypted before it is handed to a network transport.
3. A transport provider MUST NOT require access to plaintext in order to synchronize replicas.
4. Loss of all required cryptographic material MAY make data permanently unrecoverable. A.P.C. MUST NOT depend on account recovery, e-mail recovery or a vendor recovery service.
5. The portable format MUST remain independent of platform-specific key stores.

## 5. Portability

1. A.P.C. data MUST be usable without GitHub.
2. A.P.C. data MUST be usable without Android.
3. The format MUST be openly documented sufficiently for an independent implementation to read and write it.
4. Users MUST be able to export their information in the native A.P.C. format.
5. The application SHOULD support export to ordinary interoperable forms such as plain text, structured text and PDF when meaningful for the selected content.

## 6. Scale

1. The data model MUST NOT impose an arbitrary semantic limit on the number of atoms.
2. The collaboration model MUST NOT assume a small fixed number of replicas or users.
3. Large attachments MUST NOT require loading the complete attachment into memory.
4. Attachment size MUST NOT alter the semantic model of surrounding text or annotations.
5. Synchronization and storage implementations SHOULD permit chunked and lazy access to large binary content.

## 7. Synchronization

1. Synchronization MUST be optional.
2. Offline editing MUST remain fully functional.
3. Semantic merge MUST be performed by A.P.C., not by Git or GitHub.
4. GitHub MAY provide repository access control, but GitHub permissions are not part of A.P.C. format semantics.
5. The format SHOULD reserve a compatible extension path for future A.P.C.-level authorization without requiring such a system in the initial implementation.
6. A synchronization race MUST be recoverable by fetching the newer remote state, merging locally and retrying publication.

## 8. Platform protection

1. Platform security facilities MAY protect local A.P.C. key material.
2. Android biometric authentication and device credentials MUST act as local unlock mechanisms rather than becoming portable A.P.C. content keys.
3. Platform hardening MUST NOT change portable format semantics.
4. The application MUST NOT refuse normal operation solely because the user chooses a weaker host environment, such as an unlocked bootloader.
5. Reduced platform protection SHOULD be communicated with a compact dismissible notice rather than an interrupting dialog.

## 9. User control

1. The user MUST be able to copy and export their own data.
2. Security features MUST NOT deliberately obstruct competent use of the application.
3. The application MUST NOT contain advertising.
4. The application MUST NOT require telemetry for normal operation.
5. The application MUST NOT require an account for local use.

## 10. Non-goals of the core

The portable core does not attempt to protect plaintext after the user exports it to another application or service.

The portable core does not attempt to turn A.P.C. into an active counter-intrusion system.

Decoy interfaces, honeypots, forced clipboard blocking, mandatory custom keyboards, aggressive device-integrity refusal and similar mechanisms are not core requirements.
