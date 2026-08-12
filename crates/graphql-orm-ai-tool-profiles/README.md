---
title: "graphql-orm-ai-tool-profiles"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-11
supersedes: []
---

# `graphql-orm-ai-tool-profiles`

`graphql-orm-ai-tool-profiles` compiles reviewed, least-disclosure GraphQL
tool profiles against a finished SDL into fingerprinted descriptors and
versioned subgraph manifests. It is backend-neutral and can be used by a
subgraph without selecting the `graphql-orm-ai` persistence runtime.

It does not enable resolvers, mint authority, perform introspection, execute
GraphQL, grant provider egress, or make a generated resolver an AI tool. Those
are separate runtime decisions and must remain default-deny.

## Install

```toml
[dependencies]
graphql-orm-ai-tool-profiles = { git = "https://github.com/Dastari/graphql-orm.git", rev = "fac98d99e64c841a34d2d0096cdf928c3f9a7c6f", version = "0.3.0" }
serde_json = "1"
```

There are no package feature flags. The manifest wire version is currently 2
(`AI_GRAPHQL_TOOL_MANIFEST_VERSION`); JSON object keys are canonicalized before
descriptor and manifest hashing, while array order remains meaningful.

This unpublished package has no docs.rs release. The Cargo metadata points to
this Git README; build rustdoc for the checked-out revision with
`cargo doc -p graphql-orm-ai-tool-profiles --no-deps` when API-level detail is
needed.

## Compile a finished-schema-validated custom profile

This example builds a read-only profile for an explicitly described handwritten
query. The profile has one bounded model input, one closed resolver argument,
an exact result projection, matching disclosure shape, and serialized
extension payload. The canonical source is:

- [finished-schema custom-profile example](examples/custom_profile.rs)

It is executed by `cargo test -p graphql-orm-ai-tool-profiles --example custom_profile`.

The manifest is a transportable static contract, not an execution permit.
Before registering or executing it, validate it against the exact active
finished SDL using `validate_against_finished_schema`.

## Generated resolver admission

For a generated resolver, use `add_generated_profile(profile, &catalog,
&policy)` instead of `add_custom_profile`. The owning subgraph supplies the
current `GraphqlOperationCatalog` and an implementation of
`AiGeneratedGraphqlOperationPolicy`. `DenyAllAiGeneratedGraphqlOperationPolicy`
is the fail-closed default. The compiler rejects hidden/stale operations,
subscriptions, and generated operations not classified as reviewed application
operations. Resolver discovery alone never admits a tool.

## Building blocks and limits

| Type / builder | Contract |
| --- | --- |
| `AiGraphqlProfileInput` | closed string, integer, number, boolean, or enum model input; at most 64 inputs per profile |
| `AiGraphqlArgumentValue` | input reference, server-owned constant, closed input object, or fixed-shape list; every input must be used exactly once through argument plans |
| `AiGraphqlSelection` | explicit scalar/object/list projection; every list needs a positive bound; projection depth is at most 8 and each level at most 128 selections |
| `AiDisclosureSchema` | versioned recursive allow-list; unknown response fields and `NeverExport` nodes are rejected; maximum nesting depth is 64 |
| `AiGraphqlToolProfile` | read-only query or explicit supervised mutation; nonempty projection and positive result byte/record bounds are required |
| `AiBrowserResultPreviewPolicy` | optional separate browser preview; byte limit 1..=1 MiB, record limit 1..=100,000, depth 1..=32, never `Secret` |
| `AiGraphqlToolManifestBuilder` | validates a finished SDL locally, compiles custom/generated profiles, orders entries, and fingerprints the versioned manifest |

`AiToolDescriptor::new` defaults to a 64 KiB result limit, 100 records,
`Internal` maximum classification, read-only maturity/risk, no approval, and
idempotency. Profile compilation replaces the result limits and binds a
server-authored document, JSON Schema, result-projection fingerprint, finished
SDL fingerprint, and disclosure fingerprint.

## Registration and serialization

Use `extension_payload` only after a successful build; it verifies manifest
version, fingerprint, and entry consistency before returning JSON. On the
consumer side use `AiGraphqlToolManifest::from_extension_payload`, aggregate
active manifests with `AiGraphqlToolManifestSet::aggregate`, and register them
through an `AiGraphqlToolManifestCatalog` implementation that remains
default-deny. `register_into` never bypasses current-principal,
owner/scope/tool/row/field, approval, egress, or output-bound checks.

## Errors, security, and migration

Errors are `AiError` with stable public codes such as
`AI_INVALID_CONFIGURATION`, `AI_EGRESS_DENIED`, and `AI_TOOL_EXECUTION_FAILED`.
Treat invalid/stale fingerprints, SDL drift, duplicate roots, unknown fields,
and missing bounds as fail-closed configuration errors. Browser preview is
closed unless its explicit policy is present, and that policy supplies only
ceilings—not read authority.

See [the changelog](CHANGELOG.md) and [migration guide](MIGRATION.md) for the
wire-version and fingerprint transition, [operation catalog](../graphql-orm-operation-catalog/README.md)
for generated discovery metadata, and the [AI runtime documentation](../graphql-orm-ai/README.md)
for execution, persistence, approval, and egress responsibilities.
