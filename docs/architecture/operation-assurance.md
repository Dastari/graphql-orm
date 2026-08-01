---
title: "Operation Assurance And Step-Up Classification"
kind: architecture
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-01
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

## Compatibility Default

`AssuranceSchemaConfig::legacy()` has no interactive mutation default and does
not fail on unclassified mutations. Installing no `AssuranceEnforcement` is
also a compatibility no-op. Queries and subscriptions never inherit a recent-
authentication requirement.

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

Generated query, mutation, and subscription resolvers call the same enforcement
hook automatically. Without installed enforcement the call is a no-op, which
preserves existing schemas.

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
- generated/custom origin and actor class;
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
