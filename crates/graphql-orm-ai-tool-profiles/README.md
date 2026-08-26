---
title: "graphql-orm-ai-tool-profiles"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-16
review_by: 2027-02-11
supersedes: []
---

# `graphql-orm-ai-tool-profiles`

`graphql-orm-ai-tool-profiles` compiles reviewed static profiles and automatic,
least-disclosure GraphQL query capabilities against a finished SDL. It emits
closed provider schemas, exact server-owned documents, disclosure contracts,
fingerprints, and versioned subgraph manifests. It is backend-neutral and can
be used without selecting the `graphql-orm-ai` persistence runtime.

It does not enable resolvers, mint authority, perform introspection, execute
GraphQL, grant provider egress, or make a generated resolver an AI tool. Those
are separate runtime decisions and must remain default-deny.

## Install

```toml
[dependencies]
graphql-orm-ai-tool-profiles = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.10.1" }
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

## Automatic typed query capabilities

`AiGraphqlQueryCapabilityCatalog::compile` consumes the complete finished SDL
and its canonical `GraphqlSemanticCatalog`. Compilation is all-or-nothing: each
public `Query` root becomes one stable finite capability, and an undeclared,
stale, ambiguous, unsupported, or excessive root makes readiness fail.
Pass the finished public application SDL (for example, ordinary `schema.sdl()`),
not a Federation transport export containing `_service` or `_entities`. Install
that same public-SDL fingerprint on the exact `GraphqlExecutionTargetRegistry`
target. Router/Federation transport fingerprints remain a separate contract.

```rust,no_run
# use graphql_orm_ai_tool_profiles::{AiGraphqlQueryCapabilityCatalog, AiGraphqlQueryCapabilityLimits, GraphqlExecutionTargetId};
# use graphql_orm_operation_catalog::GraphqlSemanticCatalog;
# fn example(sdl: &str, semantics: &GraphqlSemanticCatalog) -> Result<(), Box<dyn std::error::Error>> {
let capabilities = AiGraphqlQueryCapabilityCatalog::compile(
    "inventory",
    GraphqlExecutionTargetId::parse("inventory.graphql")?,
    sdl,
    semantics,
    AiGraphqlQueryCapabilityLimits::default(),
)?;

let capability = capabilities.capabilities().next().ok_or("no Query roots")?;
let compiled = capability.compile_compact(serde_json::json!({
    "arguments": { "id": "item-1" },
    "selections": ["id", "displayName", "children.id"],
    "relationshipArguments": {"children": {}},
    "relationshipMaximumItems": {"children": 10}
}))?;
# let _ = compiled;
# Ok(()) }
```

The model supplies only this closed typed plan. It cannot choose a target,
GraphQL root, document, variable names, hidden fields, unbounded relationship,
or disclosure policy. Secret and `NeverExport` fields are structurally absent.
The compiler binds the exact target, SDL, semantic operation, selection,
variables schema, disclosure shape, limits, and plan fingerprint. Registration
is discovery only; a fresh target/current-principal policy and the ordinary
resolver remain authoritative at execution.

The compact selection schema is one finite string enum of public paths, so
adding deep relationships does not recursively duplicate either the nested
field map or scalar descriptions at every reachable path. Public scalar and
relationship descriptions remain in the canonical discovery index; typed
relationship arguments retain their adjacent descriptions in the loaded
planning schema.
Generated entity `WhereInput` objects still omit recursive `And`/`Or`/`Not`
connectives; all non-recursive typed filter fields remain available.
Handwritten recursive inputs fail readiness rather than being approximated.
Relationship arguments, including the single nullable to-many `OrderByInput`,
continue to match the finished SDL exactly.

## Canonical capability index

After compiling the generated catalogues, combine them with reviewed static
descriptors through `AiCapabilityIndex::compile`. The result is a complete,
deterministic and independently bounded discovery index containing only public
semantic summaries and exact fingerprints. It intentionally contains no JSON
Schema, GraphQL document/SDL, database name, resolver URL, policy expression,
credential, authority or secret/hidden field.

`AiCapabilityIndex::search` provides bounded deterministic discovery with
exact namespace/kind/entity filters and stable ID tie-breaking. Explicit list,
details, search, keyset, or aggregate intent ranks the matching compiler-owned
operation shape first; public entity, execution-target, and namespace relevance
rank next. Every candidate still requires positive lexical relevance, and
non-matching shapes remain eligible. Search
returns exact candidate/index/schema/semantic/target-policy fingerprints but
grants no authority. Each entry also carries conservative compiler-owned root
and total result-record bounds for later planning. The runtime package owns
current-principal rehydration, policy reapplication, short-lived loaded
bindings and ordinary resolver execution.

Opt-in aggregate roots use the same catalogue and a fixed result projection.
Their filters, grouping, metrics, operators, and group limits remain typed and
server bounded. The generated operation's fingerprinted public entity identity
selects the one owning semantic entity. The compiled disclosure shape then
contains only the exact selected `groupBy` fields and metric field/operator
pairs. Runtime validation requires those exact returned identities before
provider egress, so an unrelated entity or a drifted/unselected metric cannot
inherit a less restrictive disclosure classification.

Custom scalar and enum roots participate only when their canonical operation
descriptor explicitly marks the result exportable. Scalar lists additionally
need a positive result-item bound. Unclassified, `Secret`, and `NeverExport`
leaf roots remain in the finished GraphQL schema but are structurally omitted
from query, mutation, and subscription capability catalogues. This is
disclosure metadata only and never grants resolver authority.

## Bounded replayable subscription contracts

`AiGraphqlSubscriptionCapabilityCatalog` is the compiler boundary used by a
durable waiter implementation. It emits capabilities only for roots whose
canonical semantics declare `ReplayThenLive`; described `BestEffort` roots
remain ineligible. A plan selects a
bounded event projection, positive timeout and event ceiling, plus at most one
top-level typed condition admitted by that root. The condition field must also
be selected. `AiCompiledGraphqlSubscription` carries the exact document,
variables, disclosure contract and all schema/catalogue/operation/plan
fingerprints. This package does not maintain subscriptions, persist waiters,
or resume an agent.

## Building blocks and limits

| Type / builder | Contract |
| --- | --- |
| `AiGraphqlProfileInput` | closed string, integer, number, boolean, or enum model input; at most 64 inputs per profile |
| `AiGraphqlArgumentValue` | input reference, server-owned constant, closed input object, or fixed-shape list; every input must be used exactly once through argument plans |
| `AiGraphqlSelection` | explicit scalar/object/list projection; every list needs a positive bound; projection depth is at most 8 and each level at most 128 selections |
| `AiDisclosureSchema` | versioned recursive allow-list; unknown response fields and `NeverExport` nodes are rejected; maximum nesting depth is 64 |
| `AiGraphqlToolProfile` | read-only query or explicit supervised mutation; nonempty projection and positive result byte/total-record bounds are required |
| `AiBrowserResultPreviewPolicy` | optional separate browser preview; byte limit 1..=1 MiB, record limit 1..=100,000, depth 1..=32, never `Secret` |
| `AiGraphqlToolManifestBuilder` | validates a finished SDL locally, compiles custom/generated profiles, orders entries, and fingerprints the versioned manifest |

`AiToolDescriptor::new` defaults to a 64 KiB result limit, 100 records,
`Internal` maximum classification, read-only maturity/risk, no approval, and
idempotency. Profile compilation replaces the result limits and binds a
server-authored document, JSON Schema, result-projection fingerprint, finished
SDL fingerprint, and disclosure fingerprint.

`maximum_result_records` is a total budget for the complete selected GraphQL
result, not the largest individual list. The public root transport envelope is
excluded; a result object or scalar counts once, sibling object/list expansions
add, nested list expansions multiply, and scalar list items count individually.
Compilation and registration reject a disclosure shape whose checked
worst-case total exceeds the descriptor budget. Runtime evaluation counts the
actual returned shape again before disclosure.

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
