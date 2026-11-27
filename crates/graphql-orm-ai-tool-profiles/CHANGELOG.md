---
title: "graphql-orm-ai-tool-profiles changelog"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-11-26
review_by: 2027-02-11
supersedes: []
---

# Changelog

## Unreleased - 0.36.0

- **No action (#551).** `AiCapabilityIndex::subscriptions_checked_against_finished_schema`
  reports whether an index was built in process from a typed subscription
  catalogue, whose compiler refuses any root that takes a `@transientInput`
  argument. The proof does not survive serialization, exactly as for
  mutations, and the fixed broker admits brokered subscriptions only from
  an index that carries it.
  `AiGraphqlSubscriptionCapability::capability_signature` now names the
  compact schema's input types (it previously fell back to positional
  names), and `observation_summary`, `maximum_duration_seconds` and
  `maximum_events` expose the observation bounds and completion conditions
  a wait takes. Capability, catalogue, index and plan fingerprints, plan
  schemas and compilation are unchanged.
  [Migration](MIGRATION.md#0360-brokered-subscription-indexes)

## Unreleased - 0.35.0 (breaking release)

- A capability index tracks whether its mutation catalogue was compiled
  against the finished SDL in this process. The fixed broker uses that check
  to exclude mutations that could accept a transient argument.

- **Action needed** if you construct `AiCapabilityIndexEntry` or
  `AiCapabilityOperationMember` literally, or compare stored index, index-set
  or entry fingerprints. Generated query, mutation and subscription
  capabilities gain `contract_fingerprint()`: the fingerprint of the root's
  model-facing contract (identity, target, semantic operation, plan schemas,
  limits, and the resolved output route, root bound, keyset page argument,
  continuation page-info fields and relationship-argument depth; a
  mutation's also covers its execution classification). It
  leaves out the finished-schema and semantic-catalogue fingerprints, so an
  edit to another capability does not change it. Index entries and entity
  members carry it as `contract_fingerprint`; a static entry uses its
  descriptor fingerprint. Because entry fingerprints cover every entry field,
  index, index-set and entry fingerprints change once. Capability, plan and
  catalogue fingerprints are unchanged.
  [Migration](MIGRATION.md#0340-capability-contract-fingerprints)

## Unreleased - 0.33.0

- **No action (#551).** `AiGraphqlMutationCapability::capability_signature`
  and `AiGraphqlSubscriptionCapability::capability_signature` render the
  compact typed signature of a mutation or subscription, as
  `AiGraphqlQueryCapability::capability_signature` does for a query, so the
  capability broker can describe generated mutations. Fingerprints, plan
  schemas and compilation are unchanged.
  [Migration](MIGRATION.md#0330-mutation-and-subscription-signatures)
- **Action needed (#551)** if you construct `AiCapabilitySearchCandidate`
  literally. It gains `approval`, the entry's reviewed approval rule, so a
  broker can decide rule admission from a held candidate. Deserializing a
  candidate without it yields `never`.

## Unreleased - Continuable keyset pages

- **No action (#551).** Generated keyset roots can compile continuable
  pages: `AiGraphqlQueryCapability::supports_continuation` and
  `compile_compact_page` select the connection's page info and take a
  server-held `after` cursor, and `AiCompiledGraphqlQuery::take_page_info`
  removes the page info from the resolver response before disclosure.
  `AiCapabilityIndexSet::browse_matches` lists top-level entries for
  hierarchical browsing. The `Debug` output of `AiGraphqlQueryPageInfo` and
  of a compiled page redacts the cursor. Existing capability, catalogue,
  index and plan fingerprints, documents and plan schemas are unchanged.
  [Migration](MIGRATION.md#0320-continuable-keyset-pages)

## Unreleased - 0.32.0

- **No action (graphql-orm #908).** An argument declared `@transientInput` in
  the finished schema is request-only data, and tool arguments are retained
  with the run. Every AI path that sees the finished SDL now refuses to carry
  one. `AiGraphqlToolManifest::register_into` and `from_extension_payload` do
  not see the SDL, so a manifest this process did not compile must be
  validated first (see the migration note). The marker covers field arguments
  only, not arguments of executable directives.
  - The GraphQL tool-profile compiler refuses an argument plan that maps it.
    A mutation whose transient argument is required therefore has no profile;
    an optional one is never sent.
  - `AiGraphqlToolManifest::validate_against_finished_schema`, and so
    `AiGraphqlToolManifestSet::aggregate`, check every entry's document
    against the owning SDL and refuse one that passes such an argument, for
    example a manifest written by an older compiler. An entry whose document
    does not execute on the entry's root type is refused. The new
    `register_into_with_finished_schema` validates and then registers.
  - The typed query, mutation and subscription capability catalogues fail to
    compile when an operation admitted for AI execution takes one. For
    mutations this is checked on every `Automatic` or `ApprovalRequired`
    mutation, including one whose result is not exportable.

  [Migration](MIGRATION.md#0320-transient-mutation-arguments)

## Unreleased - 0.31.0

- **No action (#817).** Add content-free endpoint validation reasons to the
  shared AI error contract for provider profile saves and reports. [Migration](MIGRATION.md#0310-provider-endpoint-path-error)

## Unreleased - 0.30.0

- Validate filter roles and index generated-input ownership once per target
  when compiling capability catalogues (#865). Capability fingerprints,
  plan schemas and model-facing output stay byte-identical.
  [Migration](MIGRATION.md#0300-capability-catalogue-compilation)

## Unreleased - 0.29.0

- Generated keyset-list plans now accept the page limit injected by the
  compiler from `maximumItems` (#881). The size, continuation cursor, and
  count flag stay out of the model's arguments. The resolver applies the
  schema's page-limit policy and the input's default count behavior.
  See the [migration guide](MIGRATION.md#0290-generated-keyset-page-limits).

## Unreleased - 0.28.0

Per-call costs of the capability broker that grew with the index, and
admission checks for deserialized indexes (#551). No public API, schema,
fingerprint or rendering change. See the
[migration guide](MIGRATION.md#0280-index-set-and-signature-reuse).

- **Fixed:** `AiCapabilityIndexSet::compile` now applies to every member
  index the checks `AiCapabilityIndex::compile` applies. Its own limits must
  be valid and hold for its entry count, each entry's size and the total
  size, and each entry must be stored under its own ID. A deserialized index
  is not recompiled, so before this change one that broke its own limits, or
  stored an entry under a key other than the entry's ID, was admitted and
  reached the broker. Such an index is now refused with a configuration
  error. A compiled index, or one serialized and deserialized unchanged, is
  admitted as before.

- **Performance:** `AiCapabilityIndexSet::compile` measures each member
  index once and reuses the measurement for every later set formed from the
  same index, and a one-member set builds no capability-owner map. A host
  that supplies one current index gets a one-member set on every broker
  call, so before this change each discover, describe, `describe_type` and
  execute re-serialized every index entry and re-indexed every capability ID.
  Duplicate identities in a deserialized index are still refused, and the
  set's entry and byte limits are unchanged.
- **Performance:** `AiGraphqlQueryCapability::capability_signature` and
  `shared_type_plan_schema` recover the capability's input-type names once
  and keep the small name table. Before this change, every call regenerated
  the whole compact plan schema, which for a wide entity cost more than the
  rest of a broker describe. Signatures and renderings are unchanged.

## Unreleased - 0.27.1

- Replayable subscriptions reuse compiled plan and condition validators
  across closed and compact plans. Plan schemas, accepted conditions and
  fingerprints retain their existing contracts (#543).

## Unreleased - 0.27.0 (breaking release)

The tool-profiles part of the single breaking release, with operation catalogue
0.10.0 and AI 0.151.0: operation fingerprint algorithm v2, capability index
version 3 and compact signatures for broker version 2 (#683, #551). Host steps
are in the root [migration guide](../../MIGRATION.md#01860-to-01870-breaking-release)
and the [migration guide](MIGRATION.md#0261-to-0270-breaking-release).

- **Breaking (fingerprints, #683):** requires operation catalogue 0.10.0, whose generated-operation fingerprint algorithm is `graphql-orm-sha256-len-v2`. Operation contracts and tool manifests record that name. A manifest or contract bound under `graphql-orm-sha256-len-v1` is refused as stale; regenerate it. No Rust API change.
- **Breaking (index, 5B):** `AI_CAPABILITY_INDEX_VERSION` is `3`. `AiCapabilityIndex::compile` groups every generated query root of one entity into one `AiCapabilityKind::GeneratedEntity` entry (shape `AiCapabilityOperationShape::Entity`) whose `operations` list each root (`AiCapabilityOperationMember`). The index and its limits count one entry per entity. Each root keeps its own `GeneratedQuery` entry, identity, capability fingerprint and entry fingerprint: `entry(root_id)` still resolves it, and `operation_entry` and `entity_entry_of` navigate between them. Roots without an entity stay standalone entries. Index and index-set fingerprints change once; the entity capability fingerprint (`AI_CAPABILITY_ENTITY_VERSION`) covers every root's ID and fingerprint.
- **Breaking (API):** new `AiCapabilityKind` and `AiCapabilityOperationShape` variants; new `operations` fields on `AiCapabilityIndexEntry` and `AiCapabilitySearchCandidate` (`AiCapabilityCandidateOperation`). `entries()` lists discoverable entries only, so grouped roots are not listed there.
- **Added:** `AiCapabilityIndexSet::ranked_operation_matches` ranks every executable root on its own metadata, exactly as a standalone entry, for delivery that installs one definition per root.
- **Added:** `AiCapabilityIndexSet::rank_admitted` ranks matches again after host policy, scoring each entity entry on its own metadata and its admitted roots only. `AiCapabilityRankedMatch::operations`, `entity` and `admitted_candidate`, and `AiCapabilityIndexEntry::admitted_result_description` and `admitted_result_record_cost`, expose the same admitted-only view.
- **Performance (3B):** search uses an inverted index built when the index is compiled: a query scores only the posting lists of its terms. Scores are the same fixed per-field weights as before; an entity entry is also found by its roots' operation names and descriptions. No IDF, BM25 or other catalogue-wide statistic contributes, so an entry's score never depends on other entries.
- **Added (2B):** `AiCapabilitySignature` renders a plan schema as a compact typed signature and expands one named input type, `select` or `select.<path>` at a time, in pages (`AI_CAPABILITY_TYPE_PAGE_MAXIMUM`). `AiGraphqlQueryCapability::capability_signature` builds it; `AiSharedTypePlanSchema::compile_named` defines every named GraphQL input type once. Both are model-facing renderings: plans are still validated against the unchanged closed schema and no capability fingerprint changes.
- **Fixed:** runtime live key inputs compile under the ORM's default string identities. With live subscriptions and a node identity, a UUID key member is `UUID!` in both identity modes, and an integer key member is `RuntimeInt64!` by default and `Int!` only under `with_legacy_int_identities()`. The matcher previously recognised live key inputs only when `RecordVersion` was `Int!`, so a UUID-keyed collection under default options failed every query, mutation and subscription catalogue with `InvalidConfiguration("generated input <Object>KeyInput.<field> differs from its canonical contract")`. The identity mode is now read from the `RecordVersion` scalar. No data migration is needed.

## Unreleased - 0.26.1

- Install from a reviewed Git revision using the package inventory and release manifest.

## Unreleased - 0.26.0

- Reuse compiled validators for query, mutation and subscription plans and
  their GraphQL arguments. Plan schemas, validation and fingerprints are
  unchanged.
- Classify compact query plan corrections at the validation site so editing
  diagnostics cannot change the model-facing correction code.

## Unreleased - 0.25.0

Declare Rust 1.97.1 as the minimum supported compiler; verify explicit feature graphs in CI.

## Unreleased - 0.24.0

- Preserve root description, title, examples and default through shared-schema compilation and expansion.

Capability scale (part of #551). Capability, catalogue and index fingerprints are unchanged for existing inputs.

- **Added:** `AiSharedTypePlanSchema` renders a compact plan schema with each repeated subschema defined once in a dictionary and referenced as `{"$ref": "#/types/<Name>"}`. The rendering is lossless (`expand` returns the input) and deterministic. `AiGraphqlQueryCapability::shared_type_plan_schema` names dictionary entries after the GraphQL input types they were generated from.
- **Performance:** `AiCapabilityIndex` computes each entry's search terms once, when the index is compiled, and interns them per index. A search tokenises only its query. Ranking, scores and tie-breaking are unchanged, and the terms are not part of the serialized index or its fingerprint; a deserialized index rebuilds them on first search.
- **Performance:** capabilities of one catalogue share one copy of the finished schema and the semantic catalogue. Each capability used to hold its own copy, which made catalogue memory and compile time quadratic in the number of roots.
- **Changed:** a root whose compact schema exceeds the 1 MiB provider contract is excluded from its query, mutation or subscription catalogue and reported by `excluded_capabilities()` (`AiGraphqlCapabilityExclusion`, `AiGraphqlCapabilityExclusionReason`). It used to fail the whole catalogue. The catalogue fingerprint covers the capabilities that compiled.
- **Added (opt-in):** `AiGraphqlQueryCapabilityOptions::with_strict_query_roots` compiles only the listed public Query roots (#463). The default still offers every exportable root. A listed name that is not an exportable public Query root is an error. The optional `strictQueryRoots` wire member is omitted when unset.
- **Added:** `generated_entity_name` and `generated_category` on `AiGraphqlQueryCapability` and `AiGraphqlMutationCapability`.
- **Added:** the `capability_startup_benchmark` example measures catalogue and index compile time, search latency and peak memory against the number of roots.

## Unreleased - 0.23.0

- Generated aggregate bounds use the canonical `groupLimit` or `group_limit` argument spelling. The host injects the bound from the plan maximum; models cannot supply it.

- **Breaking:** generated input shape drift returns a host error with type/member coordinates. ORM/macros 0.149.0 and catalogue 0.8.0 supply undisclosed input member contracts; optional members remain closed, and required members omit only their capability with host-only diagnostics. These members cannot be opened through custom disclosure. Public catalogue serialization and fingerprints omit the producer sidecar.

- **Changed:** generated inputs require canonical type identities and member types. Order inputs use `OrderDirection` (`ASC`, `DESC`); static and runtime search use their respective complete mode enums; subscription filters use `ChangeAction` (`CREATED`, `UPDATED`, `DELETED`). Runtime order validates its complete member and enum contracts. Link changes carry the target entity's canonical key input, and page inputs have exactly optional integer `limit` and `offset` members. Filter, operand and pagination helpers require generated reach as well as their closed contracts. Runtime projected outputs retain their stored scalar input shapes while projected members stay undisclosed. Link helpers describe to-many join relations. Where logical controls follow one supported field-case convention. Ordinary order inputs retain generator-configured relation-count aliases for readable relations; the aliases remain undisclosed.

- **Breaking:** handwritten input objects are closed wherever reached: custom roots, generated roots and relationship arguments alike. Canonical generated entity and service inputs, and their filter, operand and page helpers, require generated reach and their canonical identity and type contracts. Other inputs expose only fields the host declares with `AiCustomInputDisclosure`, either through an exact field coordinate or a declared custom operation. Undeclared optional fields are neither described nor accepted; an undeclared required field fails compilation. Stale field declarations, absent custom operations and declarations reaching no handwritten input are refused. Canonical aggregate metric inputs apply the generated field and operator rules on every root that uses them.
- **Breaking:** `AiGraphqlQueryCapabilityOptions` carries the declaration (`with_custom_input_disclosure`, `custom_input_disclosure`, optional `customInputDisclosure` wire member), so it is no longer `Copy`, and `maximum_relationship_argument_depth` now takes `&self`. `AiGraphqlMutationCapabilityCatalog::compile_with_options` and `AiGraphqlSubscriptionCapabilityCatalog::compile_with_options` are added; their `compile` methods use default options.
- **Changed (security):** aggregate grouping and metric fields, and aggregate field enum values, now match the exact public field name on both admission and lookup. Aggregate plans and result shapes that repeat a key under the same normalised spelling are now refused.
- **Breaking:** remove the hidden `contains_forbidden_graphql_name` helper. `graphql-orm-ai` now derives the AI control-plane root set from its own GraphQL roots.

Capability catalogue fingerprints reflect admitted capability sets; individual capability fingerprints reflect their provider schemas. No data migration is needed. See the migration guide.

## Unreleased - 0.22.0

- **Added:** `AiCapabilityIndexSet::ranked_matches` returns every entry that matches a validated query, in the deterministic search order and without the result ceiling, as `AiCapabilityRankedMatch` values that carry the exact owning index. Callers that apply host policy filter this ranking before truncating. `AiCapabilityIndexSet::search` is unchanged and is now the ceiling-limited prefix of the same ranking. Refs #551.
- **Tests:** add a public-API regression proving query-plan disclosure follows each field's own coordinate. An exportable `Subscription.ownerId` filter, ordering and selection stay available beside a `NeverExport` relation `Subscription.owner` and a private `Subscription.ownerEmail`, and the provider-facing `where`/`orderBy` inputs list exactly the public scalars. `ownerEmail`, lookalike names, the private relation and `owner.id` stay closed. For selections through an exportable relation, `plan.code` is admitted and the private `plan.codeKey` is refused. Relation filters such as `where.plan` are dropped entirely, so they are asserted only as dropped. Fixes #236.

No profile fingerprint, ranking or data migration change.

## Unreleased - 0.21.1

Re-export receiver-purpose bindings and use the shared exact document hash without changing existing signed encodings or profile fingerprints. No data migration is needed. Fixes #352.

## 0.21.0 (unreleased)

Fixes #327: expose content-free retry refusal reasons while preserving uncertain classification for possible provider effects.

## 0.20.0 (unreleased)

- Adopt operation catalogue 0.7.0. Its optional
  `GraphqlEntitySemanticMetadata::record_version` member does not change profile
  compilation, capabilities or fingerprints.

## 0.19.0 (unreleased)

- Keep the provider size ceiling on the compact projection; a wide closed execution schema may be larger.
- Consolidate the unpublished 0.17.1 input-ownership fixes below into this
  breaking minor adopting operation catalog 0.6.0 and semantic wire 3.
- Require canonical operator/operand/auxiliary contracts at all filter-helper
  boundaries. Reject private record helpers, renamed/wrong object operands,
  unknown operators, defaults, extra properties and ambiguous shared roles.
- Add real producer parity, typed operand and actual derived spatial planning
  regression sources. Canonical JSON scalars retain JSON wire values with private
  64 KiB/4,096-node ceilings, bounded depth/width and UTF-8 keys/strings; pre-clone
  runtime admission and finite local schemas share the canonical operator proof.
  RuntimeJson text and unknown scalar string codecs are unchanged. All native
  verification, including the geometry planning/bounds gate, remains outstanding.

- Preserve independently exportable scalar filters and ordering beside a
  private relation with a shared name prefix (#236).
- Bind generated input types to exact entity/type shapes; reject ambiguous
  owners and omit unknown, private or unclassified relation-derived fields.
- Cover bounded compact and nested plans, canonical input naming, entity-name
  collisions, private controls and fingerprint/operation drift bindings.
- Preserve declared public keyset primary-key ordering independently of the
  ordinary sortable flag; resolve shared page/scalar-filter helpers by typed
  usage rather than lexical entity prefixes.
- Validate runtime order object/operator shapes and restrict their field enums
  to exact public sortable scalars; cover actual bounded compact/nested ordered
  plans and private, non-sortable, unknown and conflicting-owner controls.
- Follow transitive nested input references from canonical operation and
  relationship arguments before admitting shared helpers; fail closed on
  nested custom/helper conflicts while retaining wrapper, cycle and depth rules.
- Treat exact canonical scalar-filter references as operator-helper boundaries,
  preserving public filters for an entity named StringFilter without weakening
  private/nonfilterable fields or separate custom-input ownership checks.
- Affected capability input schemas and fingerprints change; regenerate
  definitions and retained bindings. No database migration.

- **Breaking (Cargo feature/default contribution):** disable async-graphql
  defaults. Profiles uses base parser/errors and contributes no UUID feature.
  This feature-only change preserves profile APIs and fingerprints; the typed
  contract and input-ownership changes above still require regenerated bindings.
- Hosts that relied on incidental email/UI symbols or file-backed
  `UploadValue.content` through ORM's public async_graphql reexport must
  explicitly select the matching upstream flags; see the
  [workspace migration](../../MIGRATION.md#0780-to-0790-explicit-graphql-features)
  for host opt-in examples and byte-backed upload migration. No data migration
  is required.

## 0.17.0 (unreleased)

- Adopt operation catalog 0.5.0 (semantic label fields). Public compile APIs take
  `GraphqlSemanticCatalog`, so the catalog's breaking release makes this a
  breaking minor. Capabilities, manifests and fingerprints are unchanged.

## 0.16.2 (unreleased)

- Compile mixed public/secret generated schemas without exposing private root
  capabilities or aborting unrelated public capability construction.
- Omit provider relations that cannot reach an exportable scalar within depth
  and cycle bounds; preserve operator SDL and all public roots.

## [0.16.1] - 2026-10-20

### Added

- `AiError::classification` returns a distinct, payload-free lower-snake-case
  name for every variant, so durable outcomes and logs can record which
  refusal occurred where `public_code` deliberately groups variants.

## [0.16.0] - 2026-10-13

### Changed

- This pre-1.0 minor is source-breaking for public `AiToolDescriptor` literals:
  initialize the new optional `result_media` field or use existing constructors.
  Missing media preserves serialized descriptors and their fingerprints.

### Added

- Immutable result-media policies bind selected image-reference fields, exact
  object paths, logical resolver, confidentiality and finite limits to tool
  fingerprints. Secret/NeverExport ancestors and URL/path resolver IDs fail.
- `PreTransportToolSourceUnavailable` retains safe no-dispatch classification
  for unavailable protected history after an unstarted reservation is released.

## [Unreleased]

### Added

- Trusted `ToolPreExecutionRejection` carries an exact invocation hash and one
  closed public reason without resolver text, credentials or request content.
  `RejectedBeforeExecution` and `ToolRejectedBeforeExecution` preserve this
  assertion across the authenticated tool bridge. Hash binding is not remote
  authentication: the host must verify the execution owner's no-effect evidence.


## [0.14.0] - 2026-09-26

### Added

- `AiGraphqlToolProfile::automatic_mutation` compiles an exact static application
  mutation to `AutonomousWrite` maturity with approval `None`. Only ordinary
  bounded write risks are accepted; high-impact and secret profiles remain
  outside this constructor. Runtime policy and resolver authorization remain
  mandatory, and the AI runtime can tighten an exact call to one-shot approval.

### Fixed

- Allow an explicitly configured automatic query total of up to 100,000 records
  instead of 10,000, so bounded nested lists can fit without removing their
  individual bounds. Defaults, authorization and runtime disclosure checks remain unchanged.

### Added

- Content-free `AiError::ProviderExecutionLimit`, `ProviderUsageIncomplete`
  and `ProviderUsageInvalid` preserve distinct public failure codes after
  possible provider dispatch. None proves settled usage or authorizes retry.

- `AiError::SessionExecutionUnbound` and `SessionExecutionUnavailable` expose
  distinct safe recovery codes for immutable session routing. No payload or
  provider/account details are included.

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
