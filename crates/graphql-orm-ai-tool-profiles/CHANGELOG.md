---
title: "graphql-orm-ai-tool-profiles changelog"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-16
review_by: 2027-02-11
supersedes: []
---

# Changelog

## [0.6.0] - 2026-08-16

### Added

- `AiCapabilityIndex` deterministically combines the finished-schema,
  semantic catalogue, generated query/mutation/subscription catalogues,
  reviewed static descriptors and target-policy fingerprint into independently
  count-, entry-byte-, and total-byte-bounded public discovery metadata.
- Generated capabilities expose a non-recursive compact plan schema using
  explicit public scalar paths, typed root/relationship arguments, per-list
  bounds, total-record bounds and result-byte bounds. The staged compiler can
  return closed correction codes for correctable invalid selections.

### Changed

- Generated capability wire version is `3`. Provider-facing definitions use
  the compact plan; v2 closed plans remain accepted only as a migration input
  to the authoritative compiler and are no longer advertised.
- Public ORM entity, field, relationship, argument and operation descriptions
  flow from `GraphqlSemanticCatalog` into the index and its fingerprints.

### Security

- Index entries exclude executable schemas/documents/SDL, private storage
  coordinates, URLs, authorization expressions, credentials and secret or
  `NeverExport` fields. Discovery remains descriptive and grants no authority.
- Compact compilation rejects unknown/hidden/secret/stale public names,
  cross-target drift, relationship depth, collection cardinality, aggregate
  record budget and result-byte overflow.

## [0.5.0] - 2026-08-14

### Added

- Automatic query capabilities honor `GraphqlSemanticCollectionBound`.
  Pageable Many relationships still advertise and inject a trusted page
  argument. Server-fixed object lists omit model `maximumItems`, compile
  without paging arguments, and disclose the authoritative semantic ceiling.

### Security

- Query capability wire version is now `2`. A selectable Many relationship
  without an executable collection bound is rejected during capability
  construction instead of failing only after the model selects it.

## [0.4.1] - 2026-08-13

### Fixed

- Automatic capability compilation now accepts the corrected macro-generated
  relationship argument contract while continuing to compare relationship
  semantic types byte-for-byte with the finished SDL.
- Finite provider schemas omit only the recursive `And`/`Or`/`Not`
  connectives of generated entity `WhereInput` values. Their complete flat
  typed filter fields remain available; handwritten recursive inputs are still
  rejected rather than approximated.

### Security

- List-versus-object relationship tampering remains a closed configuration
  error. Catalogue discovery still grants no execution or disclosure
  authority.

## [0.4.0] - 2026-08-13

### Added

- Automatic query capabilities compile every finished-SDL `Query` root from
  the canonical semantic catalogue into a finite typed plan schema, exact
  server-authored document and variables, selected disclosure shape, stable
  identity, and complete schema/catalogue/operation/plan fingerprints.
- Explicit bounded nested relationship selection and opt-in generated
  aggregate roots no longer require a hand-authored GraphQL document or static
  AI profile. Secret and `NeverExport` fields remain structurally absent.
- Replayable subscription capabilities compile one bounded event projection,
  optional admitted top-level condition, timeout and event ceiling for a
  separate durable waiter implementation. Best-effort subscriptions remain
  described but receive no durable capability.
- `GraphqlOperationContract::with_semantic_operation_kind` binds query or
  subscription documents to an exact canonical semantic root; the existing
  query convenience API remains source compatible.
- Custom scalar/enum roots use an explicit fingerprinted result-disclosure
  contract; unclassified, secret, and non-exportable roots remain structurally
  absent from automatic query, mutation, and subscription capabilities.
- Aggregate disclosure is derived from the owning generated entity and exact
  selected grouping and metric field/operator identities, which are validated
  again against runtime results.
- `schema_roots!` described-root lists compose a handwritten root, its semantic
  operations, and direct result-object metadata from one adjacent declaration.

### Security

- Finished SDL and semantic root coverage must match exactly. Capacity,
  provider-schema size, relationship cycles, missing collection bounds,
  unknown selections, stale capability fingerprints and schema drift fail
  readiness or compilation instead of silently omitting authority-relevant
  metadata.
- Result-record ceilings cover the complete GraphQL result tree rather than
  only the largest individual list. Checked sibling addition and nested
  collection multiplication apply at compilation and registration; runtime
  evaluation independently rejects an actual composite result above the same
  exact total.

## [0.3.0] - 2026-08-11

### Fixed

- Manifest and tool-descriptor fingerprints now hash recursively canonicalized
  JSON object keys. Canonical `DescriptorExtension` transport can no longer
  make an unchanged manifest appear stale merely by reordering nested object
  members.

### Changed

- `AI_GRAPHQL_TOOL_MANIFEST_VERSION` is now 2. Producers and consumers must
  move together so the corrected fingerprint semantics cannot be confused
  with version 1.
