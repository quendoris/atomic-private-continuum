# A.P.C. Architecture State

## Status

Research phase completed. The project is transitioning from architectural validation to core implementation.

The current objective is not to build a UI prototype first. The priority is preserving validated semantics in the core model.

## Confirmed principles

### Atomic model

Information is represented as independent atomic entities.

An Atom contains separate state domains:

```
Atom
├── content
├── lifecycle
├── location
└── metadata
```

Domains must not create artificial coupling. A location change must not rewrite content. Lifecycle changes must not destroy internal state.

### Identity and causality

- Identifiers are opaque.
- Identifier magnitude has no temporal meaning.
- Trusted clocks are not used for semantic decisions.
- Transport ordering is ignored.
- Merge decisions belong to the local deterministic model.

### Synchronization

Transport is a delivery mechanism only.

The architecture boundary is:

```
Logical state
    ↓
Merge model
    ↓
Encryption and authentication
    ↓
Protected capsule
    ↓
Transport
```

Git hosting, servers, or other transports must not define data semantics.

## Validated research conclusions

### Global causality is rejected

A single global causal timeline creates unnecessary coupling between unrelated domains.

Causality should be scoped to the semantic merge domain.

### Full history resolution is not the production default

Full history provides maximal historical search, but experiments showed important limitations:

- unbounded metadata requirements;
- potentially high resolution cost;
- possible semantic drift toward unrelated historical states;
- not guaranteed total resolution.

### One-witness resolution direction

A conflicting operation should retain the last known valid causal predecessor.

Resolution model:

```
invalid operation
        ↓
causal witness
        ↓
controlled fallback
```

The witness belongs to the operation that created the conflict, not to arbitrary concurrent history.

## Hierarchy model findings

Location and hierarchy require additional validity rules.

Per-object convergence is not enough. The resulting structure must also satisfy global constraints such as cycle freedom.

The resolver must separate:

```
causal correctness
        and
structural validity
```

## Security assumptions

A.P.C. assumes hostile transport.

The portable core provides:

- encrypted storage format;
- authenticated changes;
- deterministic local merge.

The core does not provide magical recovery from lost keys.

Loss of keys means loss of protected data.

## Next implementation phase

The next milestone is A.P.C. Core:

- atom model;
- revision model;
- encrypted format;
- deterministic merge engine;
- reference tests;
- synchronization abstraction.

The UI layer must be built on top of these guarantees, not define them.
