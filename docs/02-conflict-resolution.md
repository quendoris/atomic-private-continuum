# Conflict Resolution

## Principles

A.P.C. does not reconstruct a global history.

Conflicts are resolved using causal locality and deterministic rules.

## Resolver models

### Full history

Searches historical placements or states until a valid result is found.

Advantages:

- maximal historical information.

Disadvantages:

- unbounded metadata requirements;
- possible semantic drift;
- not guaranteed to produce a result.

### One witness

A conflicting operation stores the last known valid causal predecessor.

Resolution:

```
invalid operation
        ↓
causal witness
        ↓
controlled fallback
```

Advantages:

- bounded complexity;
- preserves the intent of the specific operation;
- independent from unrelated concurrent history.

## Current research direction

The reference implementation continues testing hierarchy validity, lifecycle conflicts, move semantics, and causal compaction.
