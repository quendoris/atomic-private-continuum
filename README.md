# A.P.C.

**Atomic Private Continuum**

A.P.C. is a cross-platform format and application core for atomic, encrypted, user-owned information.

Status: core implementation started; architecture research remains active where the portable format is not yet frozen.

## Core properties

- atomic data model;
- encrypted persistent storage;
- deterministic client-side merge without trusted clocks;
- offline-first operation;
- transport-independent synchronization;
- portable data with no mandatory account or server;
- platform-specific protection outside the portable core.

The first implementation targets Android. Desktop and other platforms must remain compatible with the same format and core semantics.

## Implementation

The executable research/oracle models remain under [`reference_model/`](reference_model/).

The real portable core is being implemented in Rust under [`crates/apc-core/`](crates/apc-core/). The current implementation contract and staging rules are documented in [`docs/CORE_IMPLEMENTATION.md`](docs/CORE_IMPLEMENTATION.md).

## Documentation

Architecture, format, security, synchronization, continuity requirements and the current implementation handoff are kept under [`docs/`](docs/).

The current architectural synthesis is [`docs/ARCHITECTURE_STATE.md`](docs/ARCHITECTURE_STATE.md).

## License

A.P.C. is licensed under the **GNU General Public License v3.0 or later** (`GPL-3.0-or-later`). See [`LICENSE`](LICENSE).
