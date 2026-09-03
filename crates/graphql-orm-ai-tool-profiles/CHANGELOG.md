---
title: "graphql-orm-ai-tool-profiles changelog"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-09-02
review_by: 2027-02-11
supersedes: []
---

# Changelog

## [0.11.0] - 2026-09-03

### Added

- `ToolExecutionError::ResultBudgetExceeded` lets a bounded host transport
  distinguish an oversized GraphQL response from a resolver or transport
  failure.
- `AiError::ResultBudgetExceeded` carries the stable
  `AI_RESULT_BUDGET_EXCEEDED` public code.

### Security

- The new variants carry no response content, transport destination, schema,
  policy, credential, or resolver detail. They are size proofs only and do
  not weaken the descriptor's result-byte or disclosure checks.

## [0.10.4] - 2026-09-02

### Fixed

- Capability discovery now ranks public resolver descriptions and root
  operation identity ahead of incidental nested-field vocabulary, splits
  PascalCase public names into searchable terms, and uses explicit mechanical
  result shape only to break semantic ties.

### Security

- A weak shape match can no longer displace a resolver-authored semantic match.
  Shape metadata alone still cannot admit an otherwise unrelated capability,
  and exact namespace, kind, entity/operation, authority and execution checks
  remain unchanged.

## [0.10.3] - 2026-09-01

### Added

- `AiError::PreTransportProviderFailed` represents a provider failure whose
  adapter and call executor proved occurred before dispatch. It retains the
  stable `AI_PROVIDER_FAILED` public code.

### Security

- The variant is proof-bearing and must not classify a generic provider error.
  Failures after possible dispatch remain `ProviderFailed` and preserve the
  uncertain-effect recovery boundary.

## [0.10.2] - 2026-08-26

### Added

- `AiGraphqlQueryCapabilityCatalog::compile_with_options` accepts a generic,
  bounded relationship-argument projection depth. A host can keep every deep
  scalar path and collection bound available while omitting typed relationship
  argument objects beyond the selected depth.

### Security

- The option cannot add a path, field, argument, or bound. Omitted
  relationship-argument paths are closed by `additionalProperties: false`,
  while the canonical compiler, disclosure policy, result budgets, target
  binding, and resolver authorization remain authoritative.

## [0.10.1] - 2026-08-26

### Fixed

- Compact query, mutation, and subscription schemas now encode their exact
  scalar selection allow-list as one string enum instead of repeating one
  described `const` schema for every reachable path. Wide relationship graphs
  therefore remain within the provider-schema byte contract without removing
  public paths or weakening plan validation.

### Security

- Selection paths remain a closed compiler-owned allow-list. Unknown, hidden,
  secret, `NeverExport`, stale, cyclic, over-depth, over-cardinality, and
  over-budget plans continue to fail closed; canonical discovery retains the
  public field descriptions omitted from the repeated provider schema.

## [0.10.0] - 2026-08-23

### Added

- Capability indexes now carry conservative compiler-owned maximum root and
  total result-record costs and whether an explicit root bound is required.

### Changed

- Discovery ranks a narrowly inferred mechanical list, details, search,
  keyset, or aggregate shape before entity, execution target, namespace, and
  lexical relevance without discarding relevant mixed-shape results.
- The canonical capability-index contract version is now `2`; index and set
  fingerprints intentionally change.

### Security

- Shape and cost metadata remain descriptive only. Current host policy,
  short-lived load bindings, compiler validation, and resolver authorization
  remain mandatory.

## [0.9.0] - 2026-08-22

### Added

- `AiError::StatelessNativeItemRejected` is the proof-bearing terminal error
  for a completed, authoritatively metered StatelessReplay turn whose refused
  provider-native item was contained and produced no admitted answer or host
  tool effect. It retains the stable `AI_PROVIDER_FAILED` public code.

### Security

- The variant is not a generic rejection category. An incomplete, retained,
  unmetered, content-producing, tool-producing, or uncontained provider turn
  must remain `ProviderFailed` and preserve uncertainty.

## [0.8.0] - 2026-08-21

### Added

- `AiCapabilityIndexSet` canonically combines independently compiled logical
  target indexes for federated discovery. Its deterministic aggregate
  fingerprint binds each target to its exact index, global search preserves
  stable ranking, and every capability resolves to one owning index.
  `AiCapabilityIndexSetLimits` independently bounds targets, aggregate entries
  and entry bytes, and global search results.

### Security

- Empty sets, duplicate targets, cross-target capability-ID collisions and
  invalid member fingerprints fail closed. The set invents no aggregate SDL,
  semantic catalogue or policy identity; execution must revalidate the exact
  owning index and ordinary resolver authority.

## [0.7.0] - 2026-08-21

### Added

- `AiError::PreTransportBudgetDenied` is the closed execution-boundary signal
  for an atomic budget refusal proven to occur before provider dispatch and
  after any created reservation was released. It retains the existing public
  `AI_BUDGET_DENIED` code while preventing generic tool-loop budget limits from
  being mistaken for proof that provider transport never occurred.

### Breaking

- `AiError` gained `PreTransportBudgetDenied`. Although the enum is
  non-exhaustive, in-crate and deliberately exhaustive consumers must handle
  the new variant. `BudgetDenied` remains the generic limit error and carries
  no transport-absence proof.

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
