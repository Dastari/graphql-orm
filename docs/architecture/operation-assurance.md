---
title: "Operation Assurance And Step-Up Classification"
kind: architecture
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-11
review_by: 2027-02-01
supersedes: []
---

# Operation Assurance And Step-Up Classification

`graphql-orm` can classify generated and custom GraphQL root fields with a
provider-neutral assurance policy ID. The schema registry, directive metadata,
audit report, and client manifest describe the requirement. Server-side
enforcement remains authoritative and runs before generated resolver database
work.

Authentication evidence, recent-authentication policy, clocks, and step-up
session rotation remain outside the ORM. Enable `auth-agql` to use the optional
one-way evaluator backed by `agql-auth`, or implement
`AssuranceRequirementEvaluator` for another authoritative authentication
runtime.

An evaluator decides from an `AssuranceCheck`: the root's kind and field name,
and the data of the current request. It receives no field context, so the same
decision can be made before any resolver runs. In a resolver the check also
sees data an extension passed to `run_with_data`; the runtime pre-pass sees
only request, session and schema data, so install `AssuranceEnforcement` and
evaluator inputs there. The evaluator must have no side effects, because
runtime GraphQL evaluates each query and mutation root twice and runs it only
if both evaluations admit it.

## Compatibility Default

`AssuranceSchemaConfig::legacy()` has no interactive mutation default and does
not fail on unclassified mutations. Installing no `AssuranceEnforcement` is
also a compatibility no-op. Queries and subscriptions never inherit a recent-
authentication requirement.

A configured default covers every mutation that reaches enforcement. That
includes a mutation the registry does not contain, which is treated as
interactive. Strict classification instead rejects an unregistered or
unclassified mutation as misconfigured. A registry knows only the operations it
was built from, so register every root the schema serves.

Strict adoption is explicit:

```rust
let config = AssuranceSchemaConfig::legacy()
    .with_default_interactive_mutation_policy("interactive.recent-auth")?
    .with_strict_mutation_classification(true);

let mut builder = OperationAssuranceRegistry::builder(
    graphql_orm_operation_catalog(),
    config,
);
```

The default applies only to mutations classified as `Interactive`. Generated
operations begin as interactive. Mark machine, service, and safety teardown
operations explicitly, then give each mutation a requirement or exemption:

```rust
builder.set_actor_class(
    GraphqlOperationKind::Mutation,
    "runImport",
    AssuranceActorClass::Machine,
)?;
builder.exempt(
    GraphqlOperationKind::Mutation,
    "runImport",
    "machine credential has no interactive session",
)?;
```

An explicit assurance requirement may still be assigned to any actor class.
An API/service-token principal cannot satisfy user-session recent
authentication through the `auth-agql` evaluator, so such a requirement fails
as `FORBIDDEN`. Exemption is a classification decision, not authorization;
normal principal, scope, entity, row, field, and database policies still run.

## Custom Resolver Fields

Register custom fields with a stable operation identity and attach the generic
guard to the resolver:

```rust
builder.register_custom(
    "custom:rotate-credential:v1",
    GraphqlOperationKind::Mutation,
    "rotateCredential",
    AssuranceActorClass::Interactive,
)?;
builder.require(
    GraphqlOperationKind::Mutation,
    "rotateCredential",
    "interactive.recent-auth",
)?;

#[graphql(guard = "DeclaredAssuranceGuard::new(GraphqlOperationKind::Mutation)")]
async fn rotate_credential(&self) -> bool {
    true
}
```

`#[graphql_orm_custom_operations]` adds this guard, with the root's own kind,
to each described root. A host guard is composed after it as
`DeclaredAssuranceGuard::new(…).and(HostGuard)`, so the host guard cannot
replace, weaken or misclassify it. A guard on the root's `Object` or
`Subscription` attribute is composed the same way into resolvers without
their own guard. A described root takes a single `#[graphql(...)]` attribute.
It cannot use `flatten`, which serves another object's fields without the
resolver's guard, or `derived`, which exposes the resolver under an
uncatalogued name. The root's attribute cannot use `rename_fields`; name
resolvers explicitly instead.

The root must carry exactly one recognised async-graphql root attribute of its
declared kind (`Object` for queries and mutations, `Subscription` for
subscriptions), directly below `#[graphql_orm_custom_operations]`. A missing
attribute, an import alias such as `#[GqlObject]`, a kind mismatch, a second
root attribute, or a root attribute placed above the macro fails to compile, so
the root's guard and `rename_fields` are never silently unread.

Generated query, mutation, and subscription resolvers call the same enforcement
hook automatically. Without installed enforcement the call is a no-op, which
preserves existing schemas.

Enforcement keys on the exact root coordinate. A resolver that runs below
another root passes its own coordinate to `enforce_operation_assurance`.
Generated Federation entity resolvers, emitted for an entity's
`federation_key`, run below `_entities`: they enforce the entity's get-by-id
query root and any classification registered for `_entities`. A handwritten
`#[graphql(entity)]` resolver gets no assurance check from the ORM. Generated
dependent deletes use their own delete root.

## Runtime GraphQL Roots

Runtime collection roots, `node`, generated and link mutations,
`RuntimeHostField` roots and live subscriptions enforce their own coordinate.
For query and mutation operations the runtime request guard first evaluates
every root the operation executes, after alias, fragment and
`@skip`/`@include` expansion. It does so before request preparation, so a
refused root causes no database read and no host read-authority, write-authority
or resolver callback. The resolver repeats the check as defence in depth and
returns the pre-pass refusal. Subscriptions are checked before registration. `RuntimeGraphqlComposer::register_assurance_operations` adds
module roots as `runtime` origin operations and host fields as `custom`
operations. Coordinates the builder already holds keep their registration.
See [runtime GraphQL](../reference/graphql-orm/runtime-graphql.md#operation-assurance).

## Authoritative Server Enforcement

With `auth-agql`, configure the upstream policy set and injected clock, then
install the evaluator and registry as schema data:

```rust
let evaluator = AgqlAssuranceEvaluator::new(
    Arc::new(assurance_policies),
    Arc::new(SystemClock),
);
let enforcement = AssuranceEnforcement::new(
    Arc::new(builder.build()?),
    Arc::new(evaluator),
);

let schema = schema_builder(database)
    .data(enforcement)
    .finish();
```

The bridge converts the declared policy ID into the upstream
`AssuranceRequirement`, evaluates the current accepted user with the current
clock, and maps denial state to a lowercase GraphQL extension key:

```json
{ "code": "STEP_UP_REQUIRED" }
```

The stable categories are `STEP_UP_REQUIRED`, `UNAUTHENTICATED`, and
`FORBIDDEN`. No human-readable message parsing is required. Provider evidence
is verified by the host before upstream session step-up; the ORM never receives
raw provider tokens or factor secrets.

## Audit, Directives, And Manifest

Use `registry.ensure_complete()` or `registry.audit().assert_complete()` in a
schema test. Only exposed mutations participate in this completeness gate;
queries and subscriptions may remain unclassified.

Completeness covers registered operations only. To prove the registry covers
the served schema, call `registry.ensure_schema_sdl(&schema.sdl())`, or
finish a static schema with `enforcement.finish_schema(builder)`. Under strict
classification the latter fails with `UnregisteredMutations` for any
unregistered Mutation root field. A runtime composer applies the same check in
`finish` when strict enforcement is installed through `data`.

`ASSURANCE_DIRECTIVE_DEFINITIONS` contains the provider-neutral directive
definitions. `registry.schema_metadata()` emits a directive use for every
required or exempt field, for example:

```graphql
@requiresAssurance(policy: "interactive.recent-auth", actor: INTERACTIVE)
@assuranceExempt(reason: "session revocation must remain available", actor: SAFETY_TEARDOWN)
```

This metadata API lets schema tooling include directives without changing
legacy SDL by default. It includes the exact root field coordinate and stable
operation identity.

`registry.manifest()` deterministically sorts operations and includes:

- generated fingerprint or stable custom operation identity;
- query/mutation/subscription kind and exact `Root.field` coordinate;
- generated, runtime, or custom origin and actor class;
- policy ID for requirements;
- explicit reason for exemptions; and
- a format version and SHA-256 fingerprint.

The manifest is advisory client-codegen input. It can prepare step-up UX, but
it cannot authorize an operation, extend a session deadline, or replace the
server evaluator.

## Rollback

Keep the previous exact revision available. To roll back this opt-in feature,
remove `AssuranceEnforcement` from schema data and return to
`AssuranceSchemaConfig::legacy()` or stop building the registry. No database,
GraphQL data, token, or session migration is involved. Clients must treat a
removed/stale manifest as advisory and continue handling authoritative server
denials.
