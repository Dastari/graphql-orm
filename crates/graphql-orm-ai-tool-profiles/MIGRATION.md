---
title: "graphql-orm-ai-tool-profiles migration guide"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-13
review_by: 2027-02-11
supersedes: []
---

# Migration Guide

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
