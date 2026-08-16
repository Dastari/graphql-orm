---
title: "graphql-orm-ai-tool-profiles migration guide"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-16
review_by: 2027-02-11
supersedes: []
---

# Migration Guide

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
