---
title: "graphql-orm-operation-catalog"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-20
review_by: 2027-02-11
supersedes: []
---

# `graphql-orm-operation-catalog`

Version 0.13.0 adds optional replay-window facts to client metadata for
opted-in generated Changed streams.
[Changed replay reference](../../docs/reference/graphql-orm/macros-and-attributes.md#bounded-replay)

Version `0.11.0` adds two opt-in pieces and changes no existing descriptor,
wire format or fingerprint. No data migration is needed.

- `GraphqlResolverCatalog` lists what has no root coordinate: every
  relationship field the schema serves and every entity it resolves by a
  Federation key. See
  [relation and entity resolvers](#relation-and-entity-resolvers).
- `rfc8785_canonical_json` and `rfc8785_json_hash` encode JSON as RFC 8785
  specifies. See [canonical JSON encodings](#canonical-json-encodings).

[What an export discloses](#what-an-export-discloses) states who learns which
names from each export.

Version `0.10.0` (**breaking fingerprints**) removes the Rust module path from
generated-operation fingerprints and from the catalogue order.
`GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM` is `graphql-orm-sha256-len-v2`.
Moving or re-exporting an entity no longer changes any fingerprint; every
operation, catalogue and router-export fingerprint changes once at this
upgrade. See the [migration guide](../../MIGRATION.md#01860-to-01870-breaking-release).

Version 0.9.0 declares Rust 1.97.1 as the minimum supported compiler. CI verifies
the workspace and explicit backend/provider graphs with Rust 1.97.1.

Version `0.8.1` adds `GraphqlClientMetadata`, a separate client-facing
companion document of a semantic catalogue (wire version 1, its own
fingerprint, bound to the catalogue's fingerprint), with
`GraphqlClientEntityMetadata`, `GraphqlChangedStreamMetadata`,
`GraphqlEntityLifetimeMetadata`, `GraphqlLifetimeEncoding`,
`GraphqlRelationshipKeyMetadata`, `GraphqlRelationshipKeyMapping` and
`GraphqlRelationshipNotBatchableReason`. The new types are non-exhaustive and
have constructors. `compose` and `validate_against` check every name against
the bound catalogue; `export_json` writes canonical JSON. The semantic
catalogue, its wire version 3 and every existing fingerprint are unchanged. No
data migration is needed. See
[client-facing schema metadata](../../docs/reference/graphql-orm/client-metadata.md).

Version `0.8.0` (**breaking Rust API**) adds the host-only `generated_input_members` contract to entity
metadata. ORM/macros 0.149.0 supplies exact types for generated members that
remain undisclosed. The sidecar is omitted from serde, Debug output, SDL and
router extensions, so existing public catalogue bytes and fingerprints stay
unchanged. Struct literals initialize the new field; serialized consumers
reattach producer contracts before AI compilation. No data migration is needed.
See the [migration guide](../../MIGRATION.md#01470-to-01490-host-only-generated-input-contracts).

The entire semantic catalogue is a trusted host assertion, including public
fields, deserialized metadata, relationship arguments and generated labels.
Canonical identity and shape checks limit disclosure; a custom resolver reusing
a generated input receives its generated disclosure, and the checks cannot
prove how that resolver interprets the members.

Version `0.7.3` adds:

- `GeneratedGraphqlQueryShape` and `query_shape()` on generated and resolved descriptors, so adapters can tell Keyset roots from bounded Scan roots that share the `KeysetList` category. Shapes take part in no fingerprint or encoding.
- `GeneratedGraphqlOperationCategory::Unavailable` for generated placeholder roots, `GraphqlAuthorizationRequirement::unsatisfiable()` and `GRAPHQL_UNSATISFIABLE_SCOPE`.

The category and shape enums are non-exhaustive, so existing code compiles unchanged, and existing catalogue fingerprints are unchanged. No data migration is needed. See the [root migration guide](../../MIGRATION.md#generated-catalogue-arguments-follow-the-sdl).

Version `0.7.2` pins legacy default descriptions for patch compatibility. ORM/macros 0.98 explicitly supply corrected SDL descriptions through existing constructors; consumers of 0.95/0.96 keep their descriptions and fingerprints when updating only this package. No data migration is needed.

Version `0.7.1` adds pure receiver-purpose bindings and canonical hash functions with golden vectors. Existing catalogue fingerprints and signed encodings are unchanged; no data migration is needed.

Version 0.7.0 adds `GraphqlEntitySemanticMetadata::record_version`, the field
carrying the entity's optimistic-concurrency record version and its
`GraphqlRecordVersionMode` (`managed` or `cas`). It is validated to name a
scalar field of the entity and omitted from the wire and fingerprint when
absent, so semantic wire 3 is unchanged. Struct literals must add the member;
see the [migration guide](../../MIGRATION.md#0860-to-0870-recordversion-and-catalogue-record-versions).

Version 0.6.0 adds `GraphqlSemanticFieldMetadata::filter_input` and semantic
catalogue wire version 3. `GraphqlSemanticFilterInputMetadata` declares exact
helper/value TypeRefs, `GraphqlSemanticFilterInputOperator` binds a canonical
meaning to its wire name and typed operand, and
`GraphqlSemanticFilterOperandObject` closes structured operator operands.
Filterable fields require a validated contract whose meanings exactly equal
`filter_operators`; unfilterable fields use `None`. Constructors normalize
producer collections, while deserialized noncanonical or stale payloads fail
validation. `NotIn`, `Similar` and distinct spatial operators complete producer
coverage. SDL verifies mechanical conformance, never operator authority.
Regenerate wire-3 payloads and fingerprint-bound consumers; wire 2 is rejected.
See the [migration guide](../../MIGRATION.md#0780-to-0800-canonical-typed-filter-contracts).

`graphql-orm-operation-catalog` is the backend-neutral contract for the
generated GraphQL resolver metadata emitted by `GraphQLOperations` and resolved
by `schema_roots!`. It is useful to deployment tooling, drift checks, and
optional router-protocol adapters that should not select a database backend.

It is discovery metadata only. `GraphqlOperationCatalog` does not prove a host
schema contains custom roots, authorize a resolver, or bind a GraphQL document.
`GraphqlSemanticCatalog` additionally carries reviewed classification and
structural export facts, but those facts do not authorize disclosure or replace
row, field, resolver, provider-egress, or database policy.

Version 0.5.0 adds `GraphqlEntitySemanticMetadata::label_fields`, the ordered
fields forming one human-readable record label. They are validated against the
entity's selectable, exportable, non-secret scalar fields and omitted from the
wire and fingerprint when empty. Struct literals must add the member; see the
[migration guide](../../MIGRATION.md#0670-to-0680-runtime-collection-label-fields).

Version 0.4.2 gives generated root results the entity's inherited
classification and structural export boundary. Pages, keysets, scans, counts
and Changed envelopes retain that boundary. Entities with no reachable
exportable field result, including entirely nonexportable fields and closed
relationship cycles, remain in the canonical GraphQL catalogue with
`NeverExport` result metadata. This does not remove or authorize a resolver.

Entity/catalogue equality excludes the host-only input contracts, matching public
serialization and fingerprints. JSON/extension round trips have an empty sidecar;
hosts reattach producer contracts before compiling unreadable generated members.
A non-null hidden member with a default is not representable by this contract;
current producers emit none.

## Install

Find the package manifest in the
[workspace package inventory](../../docs/reference/workspace-packages.md)
and the released version and source revision in the
[workspace release manifest](../../docs/operations/release/process.md#release-identities).

```toml
[dependencies]
graphql-orm-operation-catalog = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>" }
serde_json = "1"
```

The optional `router-protocol` feature adds conversion support for the
project-neutral router protocol package. It does not start, configure, or
authorize a router.

This unpublished package has no docs.rs release. The Cargo metadata points to
this Git README; build rustdoc for the checked-out revision with
`cargo doc -p graphql-orm-operation-catalog --no-deps` when API-level detail is
needed.

## Read a generated catalog

Applications normally obtain a catalog from the function generated by
`schema_roots!`, then serialize only the fields their external format needs.
The canonical runnable example derives an entity, resolves its schema-root
catalog, asserts that derive output exists, and prints a host-owned JSON
envelope with algorithm identifiers and all three fingerprints:

- [operation catalog example](../graphql-orm/examples/operation_catalog.rs)

It is executed by `cargo test -p graphql-orm --example operation_catalog`.

`GraphqlOperationCatalog` and its descriptor types are intentionally not a
wire format. `GraphqlSemanticCatalog` is the separate strict, versioned wire
contract for safe public descriptions and typed API shape. Use
`graphql_orm_semantic_catalog()` from `schema_roots!`, then call
`extension_payload()` or, with `router-protocol`,
`router_protocol_extension()`.

## Compose runtime descriptors

Version 0.4.1 adds construction for schemas that have no static operation
catalogue, such as runtime-composed GraphQL: `validate_semantic_description`
exposes the single description rule shared by every catalogue,
`GraphqlSemanticOperationDescriptor::generated` builds an ORM-generated root
(mutations start `Prohibited`), and `GraphqlSemanticCatalog::compose_descriptors`
composes entities and operations with full validation.

## Compose descriptors in generated code

`GraphqlOperationCatalog::compose` and
`GeneratedGraphqlOperationDescriptor::generated_with_authorization` are public
only because proc-macro expansions in downstream crates need them. Consumer
applications should not hand-author generated descriptors; use
`GraphQLOperations` plus `schema_roots!`. The runnable example also calls
`<OperationCatalogAccount as GraphqlOperationMetadata>::generated_graphql_operations()`
before root composition.

Those descriptors say what the derive emitted. The root catalog adds the
important `is_exposed` decision: generated queries are exposed for participating
entities, while mutation/subscription exposure depends on root composition and
read-only policy.

## Contract and configuration facts

| Contract | Meaning |
| --- | --- |
| `GraphqlOperationKind` | `query`, `mutation`, or `subscription`; also yields conventional root type name |
| `GeneratedGraphqlOperationCategory` | stable generated category such as `list`, `single_read`, `create`, `update_many`, or `subscription` |
| `GraphqlAuthorizationRequirement` | `Public`, `Authenticated`, all-of/any-of scope declarations, or explicit `SubgraphOnly` policy explanation |
| `GraphqlOperationCatalog` | deterministic, root-exposure-resolved operation collection and three fingerprints |
| `GraphqlSemanticCatalog` | strict versioned public entity/field/relationship/root semantic graph |
| `GraphqlSemanticClassification` / `GraphqlSemanticExport` | inherited classification and structural provider-export eligibility; neither authorizes disclosure |
| `GraphqlSemanticResultDisclosure` | explicit classification/export disposition for a custom scalar or enum result, plus a positive bound for an exportable scalar list; unclassified leaves fail safe as `Secret`/`NeverExport` |
| `GraphqlAggregateOperator` / `GraphqlAggregateValueKind` | canonical portable aggregate capabilities used by field and generated-operation metadata |
| `GraphqlSemanticFilterOperator` | closed public filter capabilities, including exact date comparison, range, null, calendar, and relative-date operators |
| `GraphqlSubscriptionObservationDescriptor` | truthful best-effort or replay-then-live subscription semantics plus bounded wait and closed condition capabilities; metadata alone does not register a runtime replay source |
| `AiMutationExecutionPolicy` | closed `Automatic`, `ApprovalRequired`, or default `Prohibited` classification for public mutations; metadata never grants execution or resolver authority |
| discovery fingerprint | algorithm `graphql-orm-sha256-len-v2`; detects generated declaration/exposure drift, not authorization or disclosure |
| authorization fingerprint | algorithm `graphql-orm-authorization-sha256-len-v2`; binds core static authorization separately |
| router-export fingerprint | algorithm `graphql-orm-router-export-sha256-len-v1`; combines the previous two for protocol adapters |

Argument descriptors retain declaration order for discovery fingerprints.
Catalog operations are sorted deterministically. Scope sets are canonicalized
for authorization fingerprinting. None of these SHA-256 values are signatures
or capabilities.

Generated operations also carry an optional `generated_entity_name`: the
canonical public GraphQL entity identity used to resolve generated aggregate
semantics. It is fingerprint-bound and is never a physical table/column name
or a Rust path. Custom operations cannot set it. Root-level result disclosure
may tighten an object result, but selected object-field classification and
`NeverExport` metadata remain authoritative and cannot be weakened.

Mutation classification is declared beside the owning resolver metadata.
Generated entities use the `ai_mutations(...)` category map and handwritten
roots use `ai_execution` on `graphql_orm_custom_operations`. An omitted
classification is `Prohibited`; queries remain separately eligible reads and
subscriptions use the observation contract. Runtime target enablement, current
principal resolution, ordinary resolver authorization, disclosure, and any
one-shot approval are separate mandatory boundaries.

## Relation and entity resolvers

`GraphqlOperationCatalog` and the operations of `GraphqlSemanticCatalog`
describe root fields. A relationship field resolves below an object, and a
Federation entity resolver is reached through `_entities`, so neither is a
root field and neither is in those lists. `GraphqlResolverCatalog` (wire
version `GRAPHQL_RESOLVER_CATALOG_VERSION` 1) lists both:

| Member | Contents |
| --- | --- |
| `relations` | one `GraphqlRelationResolverDescriptor` per served relationship field with a generated resolver: owning type, field, target type, cardinality, argument names, whether a field policy exists, and an authorization reference |
| `entities` | one `GraphqlEntityResolverDescriptor` per type the schema resolves by a `@key`: the type name, each key's field names in `@key` order, and an authorization reference |

Coordinates are those of the served schema, not of the Rust declarations:

- A relationship the semantic catalogue describes is listed only when the
  schema serves it: the owning type has a field of that name, the field
  returns the related type (directly, or as the nodes of a connection for a
  to-many relationship), and it takes exactly the catalogue's arguments with
  the same types. A handwritten field that only shares a relation's name is
  not listed. An entity that declares a relation but mounts no resolver for it
  has no entry.
- A key is listed only when the schema resolves it. A type the subgraph marks
  unresolvable (a Federation reference) is not listed, whatever keys are
  registered for it.
- An entity whose Rust type the schema does not register has no entry at all,
  even when the schema serves an unrelated type with the same name.
- `type_name` and `target` are the names the schema registers. Where a type
  is registered under another name than its semantic catalogue entity
  (`#[graphql(name = "...")]`), the descriptor also carries
  `semantic_entity_name` or `semantic_target`, so it still binds to the
  catalogue. They are omitted when the names are equal.
- A key may name as many fields as an entity can have
  (`GRAPHQL_RESOLVER_MAXIMUM_KEY_FIELDS`).

It is a separate document with its own fingerprint
(`graphql-orm-resolver-catalog-rfc8785-sha256-v1`), bound to one semantic
catalogue by `semantic_catalog_fingerprint`. `schema_roots!` emits
`graphql_orm_resolver_catalog()`, which reads the served coordinates from the
query root's type registry and returns a `Result`: a document that does not
validate is an error, never a panic. A host that never calls it is unaffected.
`GraphqlResolverCatalog::compose(&semantic, sources, &served)` builds one for
a schema composed some other way. `served` is a `GraphqlServedObjects` holding
the object types that schema serves and each field's type and arguments
(`GraphqlServedField`); nothing outside it is listed. A relationship is
listed only when a source names it as a mounted generated resolver; the served
schema confirms that claim and never creates one. A source marked
`unregistered()` says the schema registers no type for that entity.

```rust,ignore
let resolvers = my_service::graphql_orm_resolver_catalog()?;
let children = resolvers.relation("Parent", "children").unwrap();
assert_eq!(children.target, "Child");
std::fs::write("resolver-catalog.json", resolvers.export_json()?)?;
```

`validate_against(&semantic)` checks a decoded document against the catalogue
it claims to describe: every relation must be a relationship field of that
catalogue, with the same target, cardinality, arguments and field-policy flag,
every key and authorization reference must name something the catalogue
publishes, and the entries must agree on which served type each catalogue
entity is. It cannot check that no served relationship is missing, because
the semantic catalogue does not record which of its fields a schema serves.

An authorization requirement is never spelled out. `GraphqlResolverAuthorization`
is one of:

- `same_as_root`: the resolver enforces the static requirement of the named
  exposed generated root of the related or resolved entity (its list root for
  a to-many relationship, its single-read root for a to-one relationship or an
  entity lookup). Read the requirement from that root's descriptor.
- `subgraph_only`: the requirement is not published. This covers a target
  with no matching exposed root, a target that is not a generated entity of
  the schema, a requirement that binds root arguments (scope templates), and a
  traversal that requires more than the root does.

The reference is to a static requirement only. The subgraph also applies the
owning type's authentication mode, field policy, row policy and database
policy, and a consumer must not treat a reference as permission.

### Out of scope for version 1: handwritten resolvers

Version 1 lists only what the ORM generates. The following are not listed,
and their absence says nothing about whether the schema serves them:

- handwritten relationship fields, including a handwritten field with the
  name, return type and arguments of a declared relation;
- a generated relation whose name a `SimpleObject` getter on the same struct
  also serves (on a composed object, a relation field without
  `#[graphql(skip)]`; anywhere, a field renamed to the relation's name). Which of the two the schema serves is
  async-graphql's choice, so the entry is left out. If the struct has a
  `#[graphql(flatten)]` field or a `#[graphql(derived(...))]` getter, the
  derive does not know every name the object serves, and no relation of that
  entity is listed;
- handwritten entity resolvers (`#[graphql(entity)]` in a host's own object).

Nothing proves what a handwritten resolver resolves or enforces. Listing them
is a follow-up that needs an explicit per-field annotation.

### What the catalogue cannot observe

The resolver catalogue reflects how the ORM's generated code mounts resolvers.
It cannot observe dispatch a host writes by hand. A host that writes the
`#[Object]` implementation of a type itself, for example one that flattens the
generated relation wrapper and adds its own method serving the same field
name, decides what that field resolves to, and is responsible for keeping the
object consistent with what the catalogue says about it.

A consumer must therefore not treat an entry as proof of enforcement for a
type whose object implementation the host wrote. For every type, an entry is
descriptive metadata from a trusted host, as the rest of the catalogue is.

## Canonical JSON encodings

Two encodings exist, and each algorithm identifier names the one it uses.

| Encoding | Used by |
| --- | --- |
| Compact `serde_json` output with object keys sorted by UTF-8 bytes | `canonical_json_hash` (receiver purpose version 1), the semantic catalogue fingerprint, the client metadata fingerprint and export |
| RFC 8785 (JCS) | `rfc8785_canonical_json`, `rfc8785_json_hash`, the resolver catalogue fingerprint and export |

The first encoding is not RFC 8785. The two give the same bytes unless a value
holds a number with a fraction or exponent (`1.0` against `1`, `1e+16` against
`10000000000000000`), an integer above 2^53 in magnitude, or object keys whose
UTF-16 and UTF-8 orders differ. The first encoding spells numbers as the
locked `serde_json` release does; a unit test pins that spelling, so a
dependency update that changes it fails the build instead of silently changing
bindings. The semantic catalogue and client metadata hold none of
these, so their bytes already equal their RFC 8785 form; receiver-purpose
variables and arguments can hold fractions, so there the issuer and the
receiver must both use `canonical_json_hash`. Existing values are unchanged:
0.11.0 does not move any existing fingerprint or binding to RFC 8785.

`rfc8785_canonical_json` refuses an integer that is not exactly an IEEE 754
double, because RFC 8785 numbers are doubles and encoding it would change the
value. The check reads the integer's digits, so it also holds for integers of
any length under `serde_json/arbitrary_precision`. Use the encoder for a new
format, and name it in that format's algorithm identifier.

The encoder is given a `serde_json::Value`, so the caller owns two checks that
must happen on the JSON text. `serde_json` keeps the last value of a repeated
member name without reporting it, and it turns a non-finite float into `null`.
A caller that canonicalizes untrusted text must reject repeated names while
parsing, and one that builds values from floats must refuse non-finite ones.

`exact_document_hash` is SHA-256 over the document's exact UTF-8 bytes. It does
not parse or normalise: two documents that differ in whitespace, a comment,
field order or a line ending have different hashes. That is deliberate and
fails closed; an issuer must hash the same bytes the receiver has registered.

## What an export discloses

Whoever receives an export reads every name in it. Each export carries the
following, and none carries a table, column, relationship condition, Rust path
or policy implementation.

| Export | Names it carries | Also in the generated schema |
| --- | --- | --- |
| `GraphqlOperationCatalog::router_protocol_operations()` | exposed generated root fields, their argument names and types, and each root's authorization requirement: **scope names and scope templates**, or the short `SubgraphOnly` explanation | root fields and arguments are in the SDL; fixed scopes are in the Federation SDL as `@requiresScopes`. Scope templates and the `SubgraphOnly` text are not in the SDL |
| `GraphqlSemanticCatalog` extension | public types, fields, arguments, descriptions, relationship targets, classifications; whether a policy exists, never its name | names and descriptions are in the SDL |
| `GraphqlClientMetadata` | public field names of streams, lifetimes and relationship keys | yes |
| `GraphqlResolverCatalog` | served type, field, argument and root names and `@key` field sets; for a type registered under another name, its semantic catalogue entity name; no scope name | yes: types, fields and arguments in the SDL, keys in the Federation SDL. The entity name of a renamed type is in the semantic catalogue, not the SDL |

A router descriptor is therefore operator-facing configuration, not a public
document: it tells its reader which scopes guard which root field. A fixed
scope is already visible to anyone who can read the Federation SDL
(`_service { sdl }`), which Federation requires the router to read. A scope
template is published only through the router export. Give a descriptor to the
router that enforces it and to no wider audience, and do not put anything
secret in a scope name. Unexposed operations are never exported.

Fingerprints are SHA-256 digests of these documents. They reveal nothing
beyond equality, and they are not signatures.

## Errors and migration

The package has no fallible catalog-builder API for ordinary consumers; macro
and schema compilation reject invalid declarations earlier. Treat a changed
fingerprint as a review signal, not automatic permission to deploy. Preserve
the algorithm identifiers with any exported value so a consumer can reject an
unknown contract version.

See the [core macro reference](../../docs/reference/graphql-orm/macros-and-attributes.md),
and [operation assurance architecture](../../docs/architecture/operation-assurance.md).

## Stable entity identity

No fingerprint depends on where an entity is declared. A generated operation
is identified by its entity name, table, backend, root coordinate, arguments,
result type and schema signature, and the catalogue is ordered by kind, root
field, entity name, table and category. Moving an entity to another module or
crate, or re-exporting it, keeps every fingerprint.

`entity_rust_type()` returns the entity's declared identity: an optional
`stable_type_id`, or by default the Rust module path and type name. It
distinguishes entity groups at composition and appears in diagnostics.
`diagnostic_rust_type()` records the real Rust path. Neither is hashed or
exported. `stable_type_id` is an opaque string literal of 1–1024 printable
ASCII bytes without whitespace or control characters.

The schema signature still records field types, argument types and relation
targets as spelled in source, so changing such a spelling changes fingerprints.

Use `GraphqlOperationCatalog::try_compose` to receive a
`GraphqlOperationCatalogError` if entity groups repeat an identity, including
when a declared ID collides with a default path. The error names both real Rust
types. The infallible `compose` panics with the same diagnostic. Multiple
operations belonging to one entity remain valid. Empty groups have no
operation identity to check. Semantic catalogues and tool/router bindings
consume the resulting operation fingerprints rather than independently producing
Rust identities.

A stable entity identity alone does not guarantee stable fingerprints after a
move. The schema signature retains Rust source spellings: field types, argument
types and relation targets must be spelled identically before and after the
move, including across crates. Prefer imported bare names rather than changing
`crate::types::Status` to `old_crate::types::Status`. Compare
`catalog().fingerprint()` before and after using the generated
`graphql_orm_operation_catalog()` helper or your catalogue accessor.

See [attribute validation](../../docs/reference/graphql-orm/macros-and-attributes.md#stable-entity-identity)
and the [move migration recipe](../../MIGRATION.md#01370-to-01390-optional-stable-entity-identity).
