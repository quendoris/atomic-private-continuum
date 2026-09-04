# Sync Architecture

## Transport independence

GitHub, local networks, removable storage, and future transports are delivery mechanisms only.

The transport layer never decides merge semantics.

Pipeline:

```
Logical state
    ↓
Merge projection
    ↓
Encryption + authentication
    ↓
Protected capsule
    ↓
Transport
```

## Synchronization model

Local durability and remote publication are separate concepts.

A local change may be safely stored before any network operation occurs.

Remote synchronization may batch multiple local changes into one protected capsule.

## Requirements

- encrypted transport objects;
- deterministic merge after arbitrary delivery order;
- duplicate and delayed delivery tolerance;
- no dependency on always-running background services.
