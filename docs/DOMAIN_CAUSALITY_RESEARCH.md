# A.P.C. merge-domain-local causality research

Status: active research. This document records executable experiments about causal observation boundaries across independent merge domains. It does not freeze transaction semantics or physical storage layout.

Executable cases live in `tests/test_domain_causality_lab.py` and use `reference_model/domain_causality_lab.py`.

## 1. Question

Previous working-state experiments established that a dirty merge domain must seal its current local epoch before a concurrent remote change in that same domain becomes semantically observable.

The unresolved question was broader:

> If a remote change touches one independent merge domain, must every other dirty domain in the continuum also create a causal observation boundary?

For example:

```text
Atom A.body    dirty local typing
Atom B.title   remote update arrives
```

If the two fields are independent merge domains, forcing `A.body` to mint a causal revision merely because `B.title` changed may create large amounts of semantically unnecessary metadata.

## 2. Domain-local candidate

The experimental candidate keeps working/causal state per merge domain.

Conceptually:

```text
Domain A
    local working value
    observed causal frontier A

Domain B
    local working value
    observed causal frontier B
```

A remote observation boundary applies only to domains whose semantic state is actually changed by that incoming projection.

Therefore a clean remote change in `B` does not alter the captured observation frontier of a dirty `A`.

## 3. Same-domain safety is unchanged

Domain-local causality is not permission to ignore relevant remote state.

If `A` is dirty and a concurrent remote revision of `A` becomes observable, `A` must still seal the pre-remote local epoch using the frontier it actually observed before the remote merge.

The executable test rejects a same-domain remote observation without the required pre-observation revision ID.

After the correct boundary the frontier contains both genuinely concurrent revisions.

Thus the earlier causality rule remains intact inside each merge domain.

## 4. Unrelated-domain result

The first test keeps domain `A` dirty while a remote update changes only domain `B`.

Result:

```text
A remains dirty
A creates 0 causal revisions
A working value is unchanged
A captured observation frontier is unchanged
B exposes the remote value
```

When `A` is eventually sealed, its revision uses exactly the frontier captured before the unrelated `B` update.

The remote `B` revision is not inserted into `A`'s semantic causal ancestry merely because both events happened in the same application process.

## 5. Global observation policy counterexample

The suite compares the domain-local rule against an intentionally over-broad policy:

```text
any remote domain changes
        ->
seal every currently dirty domain
```

The user continuously edits domain `A` while 100 successive remote revisions arrive only in independent domain `B`.

Under the global policy, each remote `B` observation forces `A` to seal and the user then continues editing `A`.

Result:

```text
domain-local policy:
A local causal revisions      1

global observation policy:
A local causal revisions    101
```

Both end with the same visible `A` value.

The extra 100 revisions therefore carry no scalar merge-domain semantics in this scenario.

This is a strong argument against continuum-global observation boundaries for independent merge domains.

## 6. Consequence for A.P.C. atomicity

This result fits the existing atomic model:

```text
Sticker
├── title       scalar merge domain
└── children    ordered collection merge domain
```

A concurrent insertion into `children` should not automatically make a pending local `title` edit claim a new title-causal observation merely because both belong to the same atom.

Likewise, a remote edit in another atom should not interrupt or causalize unrelated local typing.

Logical causal scope should follow the merge domain whose conflict semantics require the relationship, not UI timing, network polling or process-wide event order.

## 7. Sync consequence

A protected sync projection already identifies changed merge domains.

That means the future sync/apply path can conceptually perform:

```text
receive protected capsule
        |
authenticate / validate
        |
identify affected merge domains
        |
create observation boundaries only where required
        |
merge / render
```

Unrelated dirty domains can continue accumulating local durable working state without being sealed by every remote capsule.

This should materially reduce portable causal revision count during simultaneous work on large continua.

## 8. Authentication remains separate

Domain-local semantic causality does not imply that signing keys or transport authentication are domain-local.

A replica may still authenticate a publication containing changes to several domains with one per-replica authentication mechanism.

The important separation is:

```text
semantic causal relationship
!=
authentication/publication order
```

A global signing chain must not become a hidden global semantic clock for independent merge domains.

## 9. Boundary: atomic multi-domain edits

The experiment intentionally does not yet generalize the domain-local rule to operations that must be semantically atomic across several domains.

For example, if a future operation means:

```text
change A
change B
```

and partial visibility would violate the operation's semantics, the two domains may require a shared transaction/observation boundary.

That is different from merely receiving unrelated changes at similar times.

The format therefore needs to distinguish:

- independent merge domains;
- explicit atomic multi-domain mutation groups, if such groups are introduced.

No transaction primitive is selected yet.

## 10. Current decision

After this pass:

1. Causal observation boundaries SHOULD be merge-domain-local for logically independent domains.
2. A remote change in one independent domain does not force unrelated dirty domains to mint causal revisions.
3. Same-domain remote observation still requires the existing pre-observation sealing rule.
4. In the 100-update experiment, domain-local causality reduced one dirty domain's local causal revisions from 101 to 1 with the same final visible value.
5. Application/process order and remote polling order must not create semantic cross-domain causality by default.
6. Sync projections can use their changed-domain set to identify which working domains require observation handling.
7. Per-replica authentication may remain broader than one merge domain, but it must not define logical semantic ordering.
8. Explicit atomic multi-domain operations remain a separate unresolved design problem.

## 11. Next experiments

The next pass should attack:

- atomic two-domain mutations versus independent domain-local causality;
- lifecycle plus content, where deleting an atom interacts with edits to its fields;
- ordered-collection location plus atom content, especially move while content is dirty;
- whether causal metadata can be physically indexed per domain without duplicating shared authenticated publication metadata;
- large simulations with thousands of dirty domains and sparse remote projections;
- crash points where only some touched domains have completed their observation boundary;
- capsule finalization that contains several independent domains but one authentication statement.

Checkpoint, finalization, sync-capsule and moved-anchor research continue in parallel.
