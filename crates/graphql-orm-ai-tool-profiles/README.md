---
title: "graphql-orm-ai-tool-profiles"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-11-13
review_by: 2027-02-11
supersedes: []
---

# `graphql-orm-ai-tool-profiles`

The next breaking tool-profile release adds continuable keyset pages, compact mutation
and subscription signatures, and capability contract fingerprints.
See the [migration guide](MIGRATION.md).

AI tool profiles 0.38.0 reduces per-plan compiler work by reusing the validated
semantic catalogue. See the [migration guide](MIGRATION.md#0380-capability-plan-compilation).

AI tool profiles 0.37.0 adds a streaming iterator over the grouped root
matches of a ranked entity.
[Migration guide](MIGRATION.md#0370-grouped-root-iteration)

AI tool profiles 0.36.0 records whether an index was built in process from a
typed subscription catalogue, so the fixed broker admits subscriptions only
from one whose compiler refused `@transientInput` roots, and summarizes a
subscription's observation bounds for broker describe.
See the [migration guide](MIGRATION.md#0360-brokered-subscription-indexes).

AI tool profiles 0.32.0 refuses tool profiles, loaded manifests and typed
capabilities that would carry a transient mutation argument
(`@transientInput`).
See the [migration guide](MIGRATION.md#0320-transient-mutation-arguments).

AI tool profiles 0.31.0 adds typed endpoint validation reasons for provider
profile saves and reports, including missing or malformed stored endpoints.
See the [migration guide](MIGRATION.md#0310-provider-endpoint-path-error).

Version 0.30.0 speeds up capability catalogue compilation for targets with
many roots by sharing verified input metadata across capabilities. See the
[migration guide](MIGRATION.md#0300-capability-catalogue-compilation).

Version 0.28.0 stops two broker per-call costs from growing with the index:
an index set formed again from the same index reuses that index's
measurements, and a capability recovers its signature type names once. See
the [migration guide](MIGRATION.md#0280-index-set-and-signature-reuse).

Version 0.27.1 reuses compiled replayable subscription validators for both
plan formats and for admitted condition values. See the
[migration guide](MIGRATION.md#0271-subscription-validator-reuse).

Version 0.26.0 reuses compiled plan and argument validators. See the
[migration guide](MIGRATION.md#0250-to-0260).

Version 0.25.0 declares Rust 1.97.1 as the minimum supported compiler. CI verifies
the workspace and explicit backend/provider graphs with Rust 1.97.1.

`graphql-orm-ai-tool-profiles` compiles reviewed static profiles and automatic,
least-disclosure GraphQL query capabilities against a finished SDL. It emits
closed provider schemas, exact server-owned documents, disclosure contracts,
fingerprints, and versioned subgraph manifests. It is backend-neutral and can
be used without selecting the `graphql-orm-ai` persistence runtime.

It does not enable resolvers, mint authority, perform introspection, execute
GraphQL, grant provider egress, or make a generated resolver an AI tool. Those
are separate runtime decisions and must remain default-deny.

Version `0.27.0` (**breaking release**) requires operation catalogue 0.10.0
and ORM/macros 0.187.0. Operation contracts and tool manifests record the
generated-operation algorithm `graphql-orm-sha256-len-v2`, which no longer
includes the Rust module path; regenerate manifests bound under version 1. The
capability index groups the generated query roots of each entity into one
entity capability, so it holds one entry per entity; each root keeps its own
entry and fingerprint. Search uses an inverted index with unchanged fixed
per-entry weights, `AiCapabilitySignature` renders compact typed signatures
for capability broker version 2, and runtime live key inputs compile under
both ORM identity modes. See the
[migration guide](MIGRATION.md#0261-to-0270-breaking-release).

Version `0.24.0` adds a shared type dictionary rendering of plan schemas,
precomputed capability search terms, per-root exclusion of oversized schemas,
an opt-in strict query-root list and a startup benchmark example. See the
[changelog](CHANGELOG.md).

Version `0.23.0` requires operation catalogue 0.8.0 and ORM/macros 0.149.0
producer metadata. It closes handwritten input objects by default. Every input object
outside canonical generated entity and service inputs and their proved filter,
operand and page helpers is handwritten, wherever it is reached. Generated
recognition requires canonical identities and member types together with
generated operation or relationship reach, including shared helpers. A generated
label asserts the trusted host's resolver choice; it cannot prove that a host
resolver interprets canonical arguments as the ORM does. A handwritten input has no
canonical field coordinates, so its fields are described to the provider and
accepted in a plan only when the host declares them with
`AiCustomInputDisclosure`: an exact input-field coordinate, or a declared custom
operation whose reachable handwritten inputs are all disclosed. Generated
ownership follows canonical identities and contracts. An undeclared required field fails
readiness, and a stale declaration fails compilation. Generated producer contracts
keep undisclosed members closed while retaining readable members. A required
undisclosed member omits only the affected capability and returns host-only
`diagnostics()`; canonical shape drift is a host error naming its type and member.
Mutation and subscription
catalogues gain `compile_with_options` to carry the declaration. Aggregate
grouping and metric fields, and aggregate field enum values, now match the exact
public field name, and repeated normalised plan keys are refused. See the
[migration guide](MIGRATION.md).

Version `0.22.0` adds `AiCapabilityIndexSet::ranked_matches`, the complete capability ranking before any result ceiling, so policy can filter before truncation. It also adds regression coverage that query-plan disclosure follows each field's own coordinate, never a name prefix shared with a private relation. No data migration is needed.

Version `0.21.1` re-exports receiver-purpose bindings and shares exact document hashing with the catalogue. Existing signed encodings and profile fingerprints are unchanged; no data migration is needed.

Version `0.21.0` preserves proven pre-transport failure classification and exposes closed retry refusal reasons. No data migration is needed.

Version 0.20.0 adopts operation catalogue 0.7.0, whose optional entity
`record_version` metadata does not change profile compilation.

Version 0.19.0 requires operation catalog 0.6.0 and validated wire-3 producer
filter contracts. One proof checks exact canonical operator names, operand
TypeRefs and closed auxiliary structures at Where admission, helper discovery,
ownership traversal and final argument validation. Missing contracts, arbitrary
record helpers (including renamed helpers), defaults, unknown/extra properties,
wrong object operands and conflicting roles fail readiness. SDL supplies only
mechanical conformance. Regenerate all affected definitions and bindings.
Canonical static JSON operator operands preserve actual JSON values; RuntimeJson
and unknown custom scalars retain their existing string codecs. Each JSON scalar
is limited to 65,536 serialized UTF-8 bytes and 4,096 nodes (each container, null
or scalar counts once; keys do not). Depth includes its root-argument ancestry;
every array and object is bounded by `maximum_list_items`, and each key/string
by UTF-8 `maximum_string_bytes`. Finite acyclic local schema definitions project
the shape/depth/width; authoritative pre-clone runtime checks also enforce total
nodes, serialized bytes and UTF-8 bytes that JSON Schema character counts cannot.
Geometry validity and resolver/SQL authorization remain separate. The real
producer planning and bounds regression sources await native qualification.

Version 0.19.0 resolves entity-owned input fields by their exact canonical
semantic name. A private `Customer` relation does not hide an independently
exportable, filterable `CustomerId` scalar. Unknown or private field and
relation-derived inputs remain absent, and ambiguous entity input ownership
fails readiness. Rebuild affected capability definitions and retained bindings;
resolver authorization, disclosure and result limits remain mandatory.

Ordinary ordering still requires sortable scalar metadata. A generated keyset
input also admits its declared public primary-key fields when that ordinary
sortable flag is false. Shared pagination and scalar-filter inputs are identified
by their typed usage, not an entity-name prefix. Custom ownership follows nested
input references from canonical operation and relationship arguments, including
list/non-null wrappers; conflicting shared/custom roles fail closed. The finite
type traversal is cycle-safe and retains ordinary input depth/recursion failures.
Exact admitted scalar filter references in generated `WhereInput` objects lead
to operator helpers rather than nested entity-record inputs. Those helpers do
not inherit the result entity's ownership through that edge; separate direct or
custom nested uses still undergo ownership checks. Private, unknown and
nonfilterable Where fields remain excluded.
Runtime ordering preserves its validated `field`, `direction` and `nulls`
structure and restricts the field enum to exact public, non-secret,
sortable scalar identities.

Version 0.18.0 uses async-graphql base parser/errors without adding UUID or
upstream defaults. See the [feature contract](../../docs/reference/graphql-features.md)
and [migration guide](MIGRATION.md) for explicit host feature opt-ins.

Version 0.17.0 adopts operation catalog 0.5.0 (semantic label fields). It is a
breaking minor only because public APIs take catalogue types; capabilities are
unchanged.

Version 0.16.2 keeps mixed public and secret generated schemas ready: the
complete finished SDL and canonical semantic catalogue are validated, while
secret or structurally nonexportable results do not become provider
capabilities. A public result does not offer a relationship selection that
cannot reach an exportable scalar within its depth and cycle bounds. The
GraphQL relationship and operator roots remain exposed under their existing
authorization policies. No downstream catalogue filtering is needed.

## Install

Find the package manifest in the
[workspace package inventory](../../docs/reference/workspace-packages.md)
and the released version and source revision in the
[workspace release manifest](../../docs/operations/release/process.md#release-identities).

```toml
[dependencies]
graphql-orm-ai-tool-profiles = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>" }
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

For generated offset and keyset list roots, use `maximumItems` to request a
page size. The compiler supplies the GraphQL page argument, and the resolver
applies the schema's maximum page limit. Generated keyset plans read from the
start of the connection with the default count behavior.

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

For a relationship-rich schema whose complete typed argument map is larger
than the provider contract, use `compile_with_options` with
`AiGraphqlQueryCapabilityOptions::new(depth)`. The depth limits only
provider-authored relationship arguments: all exact scalar paths and
collection bounds remain available. A zero depth makes
`relationshipArguments` a closed empty object, so the provider can still make
one bounded deep read or use typed root filters without inventing nested
filters. The ordinary `compile` entrypoint keeps the complete argument surface.

## Canonical capability index

After compiling the generated catalogues, combine them with reviewed static
descriptors through `AiCapabilityIndex::compile`. The result is a complete,
deterministic and independently bounded discovery index containing only public
semantic summaries and exact fingerprints. It intentionally contains no JSON
Schema, GraphQL document/SDL, database name, resolver URL, policy expression,
credential, authority or secret/hidden field.

`AiCapabilityIndex::search` provides bounded deterministic semantic discovery
with exact namespace/kind/entity filters and stable ID tie-breaking. Public
resolver descriptions, operation/name/entity identity, execution target and
namespace rank ahead of nested field or relationship vocabulary. Explicit
mechanical result-shape intent is only a tie-breaker among otherwise relevant
candidates, so a weak list-shaped match cannot displace a resolver whose
description names the requested business result. PascalCase operation and
entity names contribute their component terms. Every candidate still requires
positive semantic relevance, and non-matching shapes remain eligible. Search
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
| `AiGraphqlToolProfile` | read-only query, explicit automatic mutation, or supervised mutation; nonempty projection and positive result byte/total-record bounds are required |
| `AiBrowserResultPreviewPolicy` | optional separate browser preview; byte limit 1..=1 MiB, record limit 1..=100,000, depth 1..=32, never `Secret` |
| `AiGraphqlToolManifestBuilder` | validates a finished SDL locally, compiles custom/generated profiles, orders entries, and fingerprints the versioned manifest |

`AiToolDescriptor::new` defaults to a 64 KiB result limit, 100 records,
`Internal` maximum classification, read-only maturity/risk, no approval, and
idempotency. Profile compilation replaces the result limits and binds a
server-authored document, JSON Schema, result-projection fingerprint, finished
SDL fingerprint, and disclosure fingerprint.

`AiGraphqlToolProfile::automatic_mutation` explicitly selects
`AutonomousWrite`/`None` for `LowRiskWrite` or `NonIdempotentWrite` application
mutations. High-impact actions use the supervised constructor. The exact
profile still requires current runtime policy and resolver authorization; a
host can tighten an individual automatic call to one-shot approval through the
AI runtime's current authorization decision. Manifest discovery never enables
execution by itself.

Deployments may explicitly configure automatic query totals up to 100,000 records
so independently bounded nested collections can fit. The default remains 100;
per-list ceilings, response-byte limits and actual-result enforcement still apply.

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

The shared error contract includes `AI_SESSION_EXECUTION_UNBOUND` and
`AI_SESSION_EXECUTION_UNAVAILABLE` for safe session-routing recovery.

## Trusted pre-execution refusals

`ToolPreExecutionRejection` and its closed reason enum describe a trusted
execution owner's exact no-effect refusal. They carry no resolver text or
request content and cannot be deserialized from a response. Use the AI runtime's
registered or remote binding helpers after independently verifying the owner's
authenticated evidence. The binding hash correlates one request/attempt; it
never proves remote authenticity or grants authority. Generic transport and
resolver failures remain uncertain for consequential operations.

`AiToolResultMediaPolicy` binds a logical owner resolver, exact selected opaque
image metadata, classification and finite limits to the tool fingerprint. See
the [AI media contract](../graphql-orm-ai/docs/tool-result-media.md); reference
JSON never supplies image bytes or resource authorization.
