# A.P.C. boundaries

This document defines which responsibilities belong to the portable A.P.C. format and core, and which belong to platform or transport layers.

## 1. Format

The A.P.C. format defines portable user data and the metadata required to interpret, verify and deterministically merge it.

The format may define:

- atomic objects and fields;
- stable object and revision identifiers;
- parent/child and ordering relationships;
- merge metadata and causal relationships;
- encrypted content envelopes;
- attachment manifests and chunk references;
- format versioning and forward-compatible extension points.

The format must not depend on:

- Android, desktop or any other host platform;
- Git, GitHub or any other synchronization provider;
- biometric APIs, Android Keystore, StrongBox or equivalent host facilities;
- UI coordinates, screen size or editor layout;
- diagnostic logs, caches or crash reports.

A valid A.P.C. object must remain interpretable by another conforming implementation without the original application.

## 2. Core

The portable A.P.C. core implements format semantics.

Core responsibilities include:

- parsing and writing the format;
- encryption and decryption of portable A.P.C. data;
- deterministic merge;
- validation of object and revision relationships;
- import and export primitives;
- attachment access;
- synchronization transport interfaces.

The core must not require a network connection.

## 3. Platform layer

Platform implementations provide host-specific protection and UX.

Android may provide:

- biometric or device-credential unlock;
- Android Keystore / StrongBox protection for local key material;
- screenshot and overlay hardening;
- application lifecycle integration;
- storage and background synchronization adapters.

These mechanisms protect the local implementation. They do not define A.P.C. portable cryptography or data semantics.

A platform capability may strengthen local protection, but absence of that capability must not make the A.P.C. format unreadable on another supported platform.

## 4. Transport layer

A transport moves encrypted A.P.C. state between replicas.

A transport must not perform semantic merge.

Git/GitHub is the first planned transport. Its role is limited to obtaining remote state and publishing local state. Any merge is performed by the A.P.C. core after decryption.

Other transports may include local network transfer, removable media, self-hosted storage or future providers without changing format semantics.

## 5. Auxiliary data

The following are explicitly outside the portable A.P.C. format unless a later specification states otherwise:

- diagnostic logs;
- crash diagnostics;
- local caches;
- performance telemetry;
- UI history not required for Continuum restoration;
- transport credentials.

If diagnostic logs contain sensitive data, they must be stored separately and encrypted.

## 6. User boundary

A.P.C. protects data while that data remains under A.P.C. control.

The application must allow normal user-directed export and clipboard operations. Exported plaintext is outside the A.P.C. protection boundary.

Security controls must reduce exposure created by the application itself and must not attempt to compensate for arbitrary unsafe actions performed by the user outside the application's trust boundary.
