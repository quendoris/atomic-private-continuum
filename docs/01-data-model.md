# A.P.C. Data Model

## Atomic entities

The fundamental unit is an Atom.

An Atom has:

- stable identity;
- independent state domains;
- immutable history of externally visible changes;
- merge semantics defined by the domain type.

Example domains:

```
Atom
├── content
├── lifecycle
├── location
└── metadata
```

Domains are intentionally separated. Changing a location does not rewrite content. Deleting an object does not require destroying its internal state.

## Revisions

A revision represents a causal state transition.

Important properties:

- identifiers are opaque;
- identifier magnitude has no temporal meaning;
- causality is explicit;
- transport ordering is ignored.

The model is designed so that two replicas can independently evolve and later converge through deterministic merge rules.
