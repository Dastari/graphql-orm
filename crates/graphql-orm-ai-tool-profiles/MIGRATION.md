---
title: "graphql-orm-ai-tool-profiles migration guide"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-10-31
review_by: 2027-02-11
supersedes: []
---

# Migration Guide

## 0.19.0 to 0.20.0: operation catalogue 0.7.0

Adopt profiles 0.20.0 with operation catalogue 0.7.0 from one reviewed
revision. Code that builds `GraphqlEntitySemanticMetadata` with a struct literal
adds `record_version: None`. Profiles, capabilities and fingerprints are
unchanged.

## 0.17.0 to 0.19.0: typed filter authority and semantic input ownership

Hosts already on 0.18.0 apply the same typed-contract migration to 0.19.0.
The earlier explicit GraphQL feature contract remains in effect.

The unpublished 0.17.1 corrections are included in 0.19.0. Adopt operation
catalog 0.6.0 from the same workspace revision. Public field literals must add
`filter_input`; every filterable scalar/enum needs a canonical typed contract,
and unfilterable fields use `None`. Its sorted unique operator meanings must
exactly equal `filter_operators`. Operator names, full operand wrappers and
finite closed auxiliary objects must match the actual producer and SDL, with
no defaults or extra properties. Missing authority cannot be inferred from SDL.

Recompile capability catalogues from the complete finished SDL and canonical
semantics, then replace affected capability/provider definitions and retained
fingerprint bindings. A `NeverExport` relation such as `Customer` no longer
removes the distinct exportable/filterable scalar `CustomerId` from its owning
`WhereInput` or sortable `OrderByInput`.

Entity-owned input properties require an exact exportable non-secret scalar
semantic field and the corresponding filter/order capability. Ordinary ordering
requires `sortable`; generated keyset inputs also retain declared public primary
keys whose ordinary sortable flag is false. Runtime ordering validates its
structural members/operator enums and restricts its field enum to exact public
sortable scalar identities. Unknown or relation-derived properties without
that identity remain unavailable; do not restore them through a host allow-list.
Generated input type names resolve to
exact entity/type shapes, and ambiguous ownership fails closed instead of
choosing a longer entity prefix. Shared page and scalar-filter helper ownership
comes from typed references, so entities named `Page` or `StringFilter` do not
capture those helpers. Entity-prefixed custom input ownership requires a
matching canonical operation argument and entity result/generated identity;
ownership also follows nested input references from operation and relationship
arguments through list/non-null wrappers. Unknown owners or conflicting
shared/custom roles, including transitive conflicts, fail closed. Type traversal
is finite and cycle-safe; ordinary input recursion and depth limits still apply.
Canonical generated Where scalar fields use exact typed operator helpers. These
edges do not propagate entity-record ownership into the helper, including when
the Where input is nested in a custom wrapper. The helper's independent direct
or custom nested uses still require ownership assessment; private, unknown and
nonfilterable Where fields stay absent.
Unrelated custom inputs retain their finished-SDL contract.

Private fields/relations, closed schemas, result disclosure and bounds, and
ordinary resolver/current-principal authorization remain enforced. Public Rust
catalogue types and semantic wire version change (2 to 3); old payloads are
rejected. GraphQL SDL, tool-manifest wire version and stored data are unchanged;
no database or schema migration is required. Do not retain an old capability
fingerprint on a regenerated input schema. Never repair forged/stale payloads.

Canonical static JSON operator operands now retain actual JSON wire values;
RuntimeJson remains JSON text and unknown custom scalars remain strings. No
stringification/coercion occurs. Private fixed ceilings are 65,536 serialized
UTF-8 bytes and 4,096 nodes per scalar (containers/null/scalars count once, keys
do not). JSON depth consumes the remaining root-argument input depth; arrays
and objects both use `maximum_list_items` for width, and keys/strings use UTF-8
`maximum_string_bytes`. Runtime checks before plan/value cloning enforce these
bounds even with a retained/staged provider schema. The finite acyclic local
JSON Schema projects depth/width/character limits; serialized-byte/node/UTF-8
checks remain authoritative at runtime. No new public configuration or data
migration. Real-derived geometry planning/bounds source tests remain UNRUN;
geometry semantics and resolver/SQL authorization are separate gates.

## 0.17.0 to 0.18.0: explicit GraphQL features

Profiles uses base parser/errors and contributes no UUID feature. This feature
change alone does not change profile APIs, capability manifests or fingerprints;
the typed-contract and input-ownership migration above remains required.

This intentional pre-1.0 feature/default break removes incidental upstream
email/UI/tempfile defaults. Follow the
[workspace host opt-in examples](../../MIGRATION.md#0780-to-0790-explicit-graphql-features),
including `UploadValue.content` byte-backed versus file-backed behavior and
the canonical Auth 0.24 full revision when integrating AI. Features remain
additive; do not add a default-on legacy umbrella. No data migration is required.

## 0.16.2 to 0.17.0: operation catalog 0.5.0

Adopt profiles 0.17.0 with operation catalog 0.5.0 from the same workspace
release. Public compile APIs take `GraphqlSemanticCatalog`, whose entities gain
`label_fields`; pass catalogues built with 0.5.0. Capability compilation,
fingerprints and manifests are unchanged, and labels are not used for
capabilities. No data or schema migration is required.

## 0.16.1 to 0.16.2: generated result disclosure

Adopt operation catalog 0.4.2 and profiles 0.16.2 from the same qualified
workspace release. Keep complete generated SDL and semantic metadata; no local
filtering or relaxed export policy is needed. Regenerate capability manifests
and revalidate their exact schema and semantic fingerprints. Secret or entirely
nonexportable results and closed provider relation projections are excluded
structurally. Resolver and provider-egress policies still decide authority.
No data or schema migration is required.

## 0.16.0 to 0.16.1

This compatible patch adds `AiError::classification`, a distinct payload-free
lower-snake-case name per variant. Existing `public_code` values and
`AiError` variants are unchanged. Adopt AI 0.108.0 from the same workspace
release, which records the classification in planner failure codes. No data
migration is required.

## 0.15.0 to 0.16.0

This is a source-breaking pre-1.0 minor release: the public
`AiToolDescriptor` gains the optional `result_media` field. Recompile consumers;
old struct literals must initialize it to `None` or use the existing
constructors. Serialized descriptors without media retain their previous
representation and fingerprints.

Profiles and descriptors gain an optional `AiToolResultMediaPolicy`. Construct
it from validated field bindings, exact selected object paths and finite limits;
use a logical owner resolver ID, never a URL or path. The declaration is included
in the immutable fingerprint and can only tighten result classification. Missing
policy preserves previous serialization/fingerprints. Consumers using public
struct literals must initialize `result_media` or use existing constructors.

Media metadata is descriptive and grants neither execution nor disclosure.
Adopt AI 0.107.0 from the same workspace release for the trusted byte resolver,
current source hooks, atomic budgets and typed provider transport. This package
requires no data migration; the owning AI package does require module 0.72.0.

## 0.14.0 to 0.15.0

Adopt the AI and tool-profile packages from one reviewed workspace release.
Trusted hosts may emit `ToolExecutionError::RejectedBeforeExecution` only after
verifying no admission, dispatch, persisted work or application effect for the
exact invocation, including earlier work under its idempotency key. The
attestation has no deserialization implementation. The request hash is a
correlation check, not authentication of a remote execution owner; HTTP status,
arbitrary GraphQL extensions, resolver text and missing rows are insufficient.

The new error variants are non-exhaustive API additions. Callers that normalize
errors must preserve trusted exact rejection evidence and must not classify an
ordinary `Execution` or `ResultBudgetExceeded` error as no-effect evidence.
There is no data migration, feature/default change or GraphQL SDL change in
this package. The owning AI runtime separately versions its persistence semantics.


## 0.13.1 to 0.14.0

`AiGraphqlToolProfile::automatic_mutation` is an explicit alternative to the
supervised mutation constructor. It accepts an exact server-authored mutation
and bounded disclosure contract with `LowRiskWrite` or `NonIdempotentWrite`
risk, and compiles it as `AutonomousWrite`/`None`. It rejects high-impact,
secret, query and otherwise invalid profiles, including during deserialization
validation. Existing read-only and supervised constructors are unchanged.

Register and enable the exact resulting fingerprint in current runtime policy;
a manifest is discovery metadata and never authorization. Hosts may use AI
`0.102.0` current policy to require exact one-shot approval for selected
arguments. No data, schema or GraphQL SDL migration is required in this package.
Adopt the profiles and AI packages from one reviewed workspace release.

## 0.13.0 to 0.13.1

The validated upper bound for `AiGraphqlQueryCapabilityLimits::maximum_result_records`
is now 100,000. Existing settings and the 100-record default remain unchanged.
Hosts can opt into larger totals for bounded nested collections; per-list,
byte, depth and runtime disclosure limits remain enforced. A changed setting
changes compiled fingerprints and requires refreshing capability bindings.
No data, schema, GraphQL SDL or wire migration is needed.

## 0.12.0 to 0.13.0

The non-exhaustive `AiError` adds `ProviderExecutionLimit`,
`ProviderUsageIncomplete` and `ProviderUsageInvalid`. Their public codes are
`AI_PROVIDER_EXECUTION_LIMIT`, `AI_PROVIDER_USAGE_INCOMPLETE` and
`AI_PROVIDER_USAGE_INVALID`. Treat these as failures after possible dispatch,
not proof of complete accounting or permission to replay. The AI runtime
preserves them in durable recovery outcomes. No data or schema migration is
needed in this package; AI 0.100.0 versions its durable semantics separately.

## 0.11.0 to 0.12.0

Handle the two additional non-exhaustive `AiError` variants with public codes
`AI_SESSION_EXECUTION_UNBOUND` and `AI_SESSION_EXECUTION_UNAVAILABLE`. They
carry no provider payload or credentials. This package has no data or schema
migration; AI `0.99.0` owns the corresponding session persistence migration.

## 0.10.4 to 0.11.0: typed result-budget failures

Adopt `graphql-orm-ai-tool-profiles` 0.11.0 with `graphql-orm-ai` 0.97.0 from
one reviewed full monorepo revision. Host transports should return
`ToolExecutionError::ResultBudgetExceeded` only when a bounded response is
refused because its byte ceiling was exceeded. Deliberately exhaustive
in-crate matches must handle the new non-exhaustive error variants.

The runtime converts that proof, and its own descriptor result-byte overflow,
to `AiError::ResultBudgetExceeded`. The stable public code is
`AI_RESULT_BUDGET_EXCEEDED`. There is no schema, database, data, GraphQL SDL,
manifest, capability, fingerprint, protected-content, credential, backup,
restore, or persistent AI schema-module migration.

## 0.10.3 to 0.10.4: resolver-first semantic discovery ranking

Adopt `graphql-orm-ai-tool-profiles` 0.10.4 from one reviewed full monorepo
revision. No capability-index wire shape or execution contract changes.
Discovery now tokenizes PascalCase public names, ranks public resolver/root
semantics ahead of nested field vocabulary, and uses explicit result shape only
as a semantic tie-breaker. Hosts should retain exact namespace, kind and
entity/operation filters whenever they know those canonical values.

## 0.10.2 to 0.10.3: proof-bearing pre-dispatch provider failure

Adopt `graphql-orm-ai-tool-profiles` 0.10.3 with `graphql-orm-ai` 0.95.12 from
one reviewed full monorepo revision. Deliberately exhaustive internal matches
must handle `AiError::PreTransportProviderFailed` separately from generic
`ProviderFailed`. Only the provider-call executor may return it after the
adapter reports `RejectedBeforeDispatch` and the unstarted budget reservation
is durably released.

The new variant keeps the stable `AI_PROVIDER_FAILED` public code. There is no
schema, database, data, GraphQL SDL, manifest, capability, fingerprint,
protected-content, credential, or persistent AI schema-module migration.

## 0.10.1 to 0.10.2: bounded relationship-argument projection

Adopt `graphql-orm-ai-tool-profiles` 0.10.2 from one reviewed full monorepo
revision. Existing callers of `AiGraphqlQueryCapabilityCatalog::compile` keep
the complete relationship-argument surface. Hosts with a relationship-rich
schema may opt into `compile_with_options` and select the maximum relationship
depth whose typed arguments should enter provider schemas. A depth of zero
keeps deep scalar selection and collection bounds but requires
`relationshipArguments` to remain empty.

The option changes capability and provider-definition fingerprints. Rebuild
retained bindings after changing it. It grants no authority and does not
change GraphQL SDL, the canonical plan compiler, disclosure checks, result
budgets, target policy, or resolver authorization. There is no database, data,
protected-content, backup, restore, or persistent AI schema-module migration.

## 0.10.0 to 0.10.1: provider-sized selection allow-lists

Adopt `graphql-orm-ai-tool-profiles` 0.10.1 from one reviewed full monorepo
revision. Recompile generated capability catalogues and refresh exact
capability/provider-session bindings because the compact JSON Schema and its
derived capability fingerprints change.

The compact plan JSON is unchanged: callers still submit a `selections` array
of exact public scalar paths plus typed relationship arguments and bounds. The
schema now represents those paths as one closed string enum instead of a
separate described `const` alternative per path. Public descriptions remain
available through the canonical capability index, and relationship argument
descriptions remain in the loaded schema.

There is no database, data, GraphQL SDL, protected-content, backup, restore, or
persistent AI schema-module migration. Resolver authorization, target policy,
disclosure validation, and result budgets remain authoritative.

## 0.9.0 to 0.10.0: shape-aware discovery and record-cost metadata

Adopt `graphql-orm-ai-tool-profiles` 0.10.0 with `graphql-orm-ai` 0.94.0 from
one reviewed full monorepo revision. Recompile every capability index and
refresh retained provider-session bindings: the canonical index contract is
now version `2`, and the added result-record cost metadata changes entry,
index, and index-set fingerprints.

Search callers keep the same request type. Explicit
list/details/search/keyset/aggregate language now ranks the matching mechanical
operation shape before entity, namespace, and lexical relevance while keeping
other lexically relevant shapes eligible. Fixed-broker descriptions add
`resultRecordCost` with compiler-owned `maximumRootRecords`,
`maximumTotalRecords`, and `rootBoundRequired` fields.

There is no database, data, GraphQL SDL, protected-content, backup, restore, or
persistent AI schema-module migration. Discovery remains non-authoritative.

## 0.8.0 to 0.9.0: proof-bearing stateless native-item refusal

Adopt `graphql-orm-ai-tool-profiles` 0.9.0 with `graphql-orm-ai` 0.86.0 from
one reviewed full monorepo revision.

`AiError` gains `StatelessNativeItemRejected`. Only the provider-call executor
may return it, after committing authoritative usage for a completed
StatelessReplay turn and proving that no assistant answer, application tool,
hosted tool, citation, cursor, or unknown provider event was admitted. The
provider adapter must separately prove the refused native item was contained.
Every incomplete or ambiguous turn remains `ProviderFailed`.

The new variant deliberately retains the stable `AI_PROVIDER_FAILED` public
code. Update deliberately exhaustive internal matches so it cannot be folded
back into provider uncertainty. There is no schema, database, data, GraphQL
SDL, manifest, capability, fingerprint, protected-content, credential, or AI
schema-module migration from this package change.

## 0.7.0 to 0.8.0: canonical federated capability-index sets

Adopt `graphql-orm-ai-tool-profiles` 0.8.0 and `graphql-orm-ai` 0.85.0 from one
reviewed full monorepo revision. Continue compiling one exact
`AiCapabilityIndex` beside each owning target after its finished SDL and
semantic catalogue are complete. Combine the active indexes with
`AiCapabilityIndexSet::compile` and use its fingerprint for delivery surfaces
and retained provider-session bindings.

Remove consumer-authored combined-index fingerprints and single-target broker
selection. Capability IDs must be globally unique across the active set; a
collision is now a deployment error. Discovery ranks the complete set, while
loading and execution revalidate the exact member index's target, schema,
semantic catalogue and policy fingerprints. Existing single-index sources can
continue through the compatibility adapter. Validate deployment-specific
`AiCapabilityIndexSetLimits` for target count, aggregate entry count, canonical
entry bytes and global search results when the defaults are not appropriate.

No database, data, GraphQL SDL, table, column, index, constraint, backfill,
protected-content, credential or AI schema-module migration is required.

## 0.6.0 to 0.7.0: proof-bearing pre-transport budget denial

Adopt `graphql-orm-ai-tool-profiles` 0.7.0 with `graphql-orm-ai` 0.84.0 from
one reviewed full monorepo revision.

`AiError` gains `PreTransportBudgetDenied`. Only the provider-execution
boundary may return it, and only after proving provider dispatch was never
attempted and any created reservation was durably released. A budget or rule
limit reached during a provider/tool loop remains `BudgetDenied`; it must not
be reclassified as a certain local refusal. Both variants intentionally expose
the same stable public error code, `AI_BUDGET_DENIED`.

The enum was already non-exhaustive. Update any deliberately exhaustive
internal matches. There is no schema, database, data, GraphQL SDL, manifest,
capability, fingerprint, protected-content, credential, or AI schema-module
migration from this package change.

## 0.5.0 to 0.6.0: compact discovery and query-plan wire v3

Adopt `graphql-orm-ai-tool-profiles` 0.6.0 and `graphql-orm-ai` 0.81.0 from one
reviewed full monorepo revision. Rebuild the finished public SDL,
`GraphqlSemanticCatalog`, generated capability catalogues and canonical
`AiCapabilityIndex`. Refresh exact capability/catalogue/index allowlists and
provider registrations: `AI_GRAPHQL_QUERY_CAPABILITY_VERSION` is now `3`, so
generated capability and provider-session fingerprints intentionally change.

Provider-facing callers now submit the compact shape:

```json
{
  "arguments": {"id": "job-123"},
  "selections": ["id", "status", "labour.id", "stock.quantity"],
  "relationshipArguments": {"labour": {}, "stock": {}},
  "relationshipMaximumItems": {"labour": 25, "stock": 25}
}
```

Use `compact_argument_schema()` and `compile_compact`; do not send the legacy
recursive `fields`/`relationships` schema to a model. The compiler temporarily
accepts an already-persisted closed v2 plan as migration input, but v3
definitions never advertise it. Every plan still receives final authoritative
depth, list, total-record and result-byte validation. Correctable compact-plan
flows may use `compile_compact_correctable` and its bounded failure code.

Compile `AiCapabilityIndex` from the exact target/schema/semantic/catalogue
set only after schema composition. Index limits are independent. Public
descriptions now participate in semantic, capability, entry and index
fingerprints, so a documentation-only semantic description change requires
the same rebuild and retained-session rebind as another executable catalogue
change.

No database, data, GraphQL SDL, table, column, index, constraint, backfill,
protected-content, credential or AI schema-module migration is required.

## 0.4.1 to 0.5.0: explicit collection-bound query plans

Adopt `graphql-orm-ai-tool-profiles` 0.5.0 with `graphql-orm` 0.23.0,
`graphql-orm-macros` 0.23.0, and `graphql-orm-operation-catalog` 0.3.0 at the
same reviewed full Git revision. Rebuild the finished SDL, semantic
catalogue, and automatic query capability set.

No database, GraphQL SDL, table, column, constraint, data, or AI
schema-module migration is required. Capability and plan fingerprints change
because the query-capability version is now `2` and server-fixed lists no
longer advertise `maximumItems`. Raise `maximum_result_records` to at least
the product of nested server-fixed ceilings before compiling a capability
that selects those lists together.

## 0.4.0 to 0.4.1: exact generated relationship arguments

Adopt `graphql-orm-ai-tool-profiles` 0.4.1 together with `graphql-orm` and
`graphql-orm-macros` 0.22.1 at the same reviewed full Git revision. Rebuild the
finished public SDL, canonical semantic catalogue and automatic query
capability set. The corrected relationship semantic argument changes semantic
catalogue and derived capability fingerprints deterministically.

No database, GraphQL SDL, manifest-wire, table, column, constraint, data,
backfill, or AI schema-module migration is required. Existing target policy,
fresh-principal, delegation, resolver authorization and disclosure checks are
unchanged. A consumer-authored catalogue normalization is neither needed nor
supported.

## 0.3.0 to 0.4.0: semantic query and bounded subscription capabilities

The automatic capability APIs are additive. Existing explicit profile and
manifest producers retain their wire version and behavior. Hosts may compile
`AiGraphqlQueryCapabilityCatalog` only after their finished SDL and canonical
semantic catalogue are complete; every public Query root must have exactly one
semantic operation and vice versa. Registration still grants no execution
authority.

Provider-facing query-plan schemas are newly derived contracts rather than a
replacement serialization for static manifests. Exact capability and plan
fingerprints must flow together through provider correlation and execution.
Secret and `NeverExport` fields cannot be restored by host policy because they
are absent from the generated schema.

Handwritten scalar and enum roots now require canonical result disclosure
metadata to become automatic capabilities. Unclassified leaves intentionally
remain available to ordinary GraphQL callers but are treated as
`Secret`/`NeverExport` and omitted from AI capabilities. Add paired
`result_classification` and `result_export` method metadata only after review;
an exportable scalar/enum list also needs `result_maximum_items`. Object roots
continue deriving disclosure from selected semantic fields and an optional
root declaration can only tighten that result.

Generated aggregate plans now bind the owning public entity and exact selected
group/metric identities into plan, result-projection, and disclosure
fingerprints. Hosts persisting or allowlisting those fingerprints must refresh
them from the adopted revision. Runtime responses with a different returned
field/operator identity fail closed.

`schema_roots!` callers may replace a handwritten root repeated in `extra_*`
and `semantic_custom_operations` (plus direct result types in
`semantic_types`) with one `described_*_types` entry. Legacy lists remain
supported, but mixing both forms for the same root is rejected at compile
time.

The bounded subscription compiler is also additive and creates no durable
worker by itself. Only `ReplayThenLive` semantic roots are admitted. Consumers
that persist compiled waiters should use the owning runtime package's schema
and migration contract rather than persisting these discovery values directly.

No database, GraphQL SDL, manifest wire, table, column, constraint, backfill,
or row rewrite is required by this package update.

Review explicit profiles whose `maximum_result_records` was set equal to only
one projected list bound. In 0.4.0 it is the checked total across the complete
GraphQL result: the root result plus sibling object/list expansions and nested
fanout. Increase it only to the reviewed complete projection maximum; retain
the individual list bounds. Descriptor and disclosure wire shapes are
unchanged, but a corrected limit changes the normal descriptor fingerprint and
any exact host allowlist must be updated.

## 0.2.0 to 0.3.0: canonical JSON fingerprints

Update every manifest producer and consumer to the same reviewed monorepo
revision. Manifest wire version 2 recursively sorts JSON object keys before
hashing while retaining array order, scalar representation, schema binding,
entry order, and all nested security contracts. Version 1 payloads remain
unsupported rather than being guessed or silently upgraded.

Tool descriptors containing JSON Schema objects may receive new fingerprints.
Hosts with exact tool-fingerprint allowlists must review and replace those
values when they adopt the new manifest. Do not copy a version 1 fingerprint
onto a version 2 descriptor.

No database or GraphQL schema migration, table change, backfill, or row rewrite
is required. This is a wire/fingerprint and host-policy configuration
migration only.
