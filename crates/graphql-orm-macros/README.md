---
title: "graphql-orm-macros"
kind: reference
status: active
owner: graphql-orm-macros-maintainers
last_reviewed: 2026-11-11
review_by: 2027-02-01
supersedes: []
---

# `graphql-orm-macros`

Version 0.126.0 serves the generated `NodeId` and `RecordVersion` on a
`#[graphql(complex)]` entity with typed computed fields and no relations,
through `compose_complex_object` and `#[graphql_complex_object]`. A handwritten
composed resolver that would shadow either field is now a compile error; see the
[root migration guide](../../MIGRATION.md#01250-to-01260-nodeid-on-complex-objects-with-computed-fields).

Version 0.125.0 records PostgreSQL change-journal entries in the database
instead of the runtime role, and journaled writes require the recorder
trigger. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01250-postgresql-records-change-journal-entries-in-the-database).

Version 0.124.0 evaluates operation assurance for runtime GraphQL query and
mutation roots before request preparation, so a refused root reaches neither
the database nor host read authorization. Assurance evaluators now receive an
`AssuranceCheck`. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01240-assurance-evaluators-take-a-request-check).

Version 0.123.0 makes runtime GraphQL reject date-time and JSON input whose
canonical form cannot be read back, and return every stored value the reader
can decode without the input limits. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01230-runtime-date-time-and-json-input-must-be-readable).

Version 0.122.0 extends operation assurance to runtime GraphQL roots,
unregistered mutations under a default policy, Federation entity resolvers and
described custom roots. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01220-operation-assurance-on-runtime-custom-and-federation-roots).

Version 0.121.0 makes schema stage application refuse destructive steps
unless `allow_destructive` is set, matching planned migrations. Upgrade
ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01210-schema-stages-refuse-destructive-steps-by-default).

Version 0.120.0 fails closed when a GraphQL request reaches a PostgreSQL RLS
entity without `DbAuthContext`, reports runtime roles that bypass RLS, and
extends RLS to search documents and the change journal (GHSA-hr7v-hr83-2jj4,
GHSA-7gw2-974x-qhrh). Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01200-postgresql-rls-enforcement-and-orm-managed-table-coverage).

Version 0.119.0 orders PostgreSQL and MSSQL migration foreign keys around the
keys they reference, including cycles and self references. Upgrade runtime and
macros together; regenerate unapplied plans. See the [migration guide](../../MIGRATION.md#01190-foreign-key-dependency-ordering).

Version 0.118.0 makes `order_aggregate` count ordering count only related
rows the request may read, failing closed for policies SQL cannot express, and
rejects `order_aggregate` on relations with a `read_policy`
(GHSA-f7xj-6695-ww2q). See the
[root migration guide](../../MIGRATION.md#01180-authorized-relation-count-ordering).

Version 0.113.0 fixes SQLite internal-prefix introspection so user table
lookalikes remain visible to migrations. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01130-sqlite-internal-name-introspection).

SQLite column/check/FK rebuilds preserve live DDL and row identity or refuse safely.

Version 0.112.0 keeps consumer fixture builds inside the caller’s `CARGO_TARGET_DIR/fixtures`, retaining workspace `target/fixtures` when unset. Upgrade ORM/macros together; no API or data migration is needed.

Version 0.111.0 corrects the MSSQL composite-key SDL regression assertion and adds CI execution coverage. Generated code and runtime behavior are unchanged; upgrade ORM/macros together. No data migration is needed.

Version 0.109.0 refreshes standalone fixture lockfiles for router 0.8.3.
Runtime and macro behavior are unchanged; no data migration is needed.

Version 0.108.0 aligns runtime and macros for composite unique migration steps.
See the [migration guide](../../MIGRATION.md#01080-composite-unique-migrations)
for exhaustive-match updates and duplicate-data handling.

Version 0.107.0 records source/target predicate presence in static relation
metadata. Hand-built `RelationMetadata` literals require
`has_source_condition` and `has_target_condition`; derives populate them.
Both runtime conversion entry points refuse conditional relations because the
runtime IR cannot preserve their predicates. See the root migration guide.

ORM/macros 0.106.0 fixes SQLite migration preparation data loss: unproven
reserved rebuild tables are preserved with a diagnostic, and interrupted
transactional rebuilds recover through SQLite rollback. Upgrade both together.

Version 0.105.0 extends static readability protection to relation declarations.
Hand-built `RelationMetadata` literals require `is_readable`; derives set it.
Default runtime conversion refuses hidden relations, while trusted repository
conversion retains them behind the non-served marker. See the root migration guide.

Skipped delegated relations require proof of an exposed generated getter.
Handwritten complex output and unmounted wrappers fail closed; see the migration
guide for ownership and composition details.

Version 0.104.0 reports a macro-owned diagnostic for empty GraphQL order inputs. Add `#[sortable]` to a persisted field (for example the primary key); generated query and incoming relation contracts then remain consistent. See the [migration guide](../../MIGRATION.md#01030-to-01040-empty-order-input-diagnostic).

Version 0.103.0 retains static field readability in column metadata and refuses static-to-runtime conversion of unreadable columns. Rebuild derives together; hand-built metadata literals need `is_readable`. Runtime schema literals also need `repository_only: false`; the explicit trusted repository conversion sets this marker to true and runtime GraphQL refuses marked schemas. See the root migration guide. No data migration is needed.

Version 0.102.0 enforces append-only create field-write policies and shares committed-output entity/row read checks with ordinary creates. Missing declared providers fail closed; denied output does not undo a committed write. See the root migration guide for client and provider changes.

Version 0.102.0 emits one create-only mutation surface for append-only composite keys, with complete-key readback and Changed events. All key components must be non-nullable, writable and host-supplied; no data migration is needed.

Version 0.99.0 revalidates the complete dependent-delete catalogue after hooks, before each prepared write and before commit. Catalogue drift refuses atomically; upgrade ORM/macros together. No data migration is needed.

Version 0.100.0 moves CacheSync authority termination to an independent jittered timer and adds separate subscriber/event callbacks. Event denials are silent; typed host failures end with continuity loss. Macros remain version-aligned with the runtime. See the root migration guide; no data migration is needed.

Version 0.98.0 rejects unreadable SimpleObject fields without an object skip, masks internal SQLx errors, and corrects generated singular descriptions. See the root migration guide; no data migration is needed. Readable relations require readable local/target keys and condition columns, proven across crates; unresolved columns or missing target metadata fail compilation. Generated Changed events require readable primary keys. `input_only` relation getters leave the SDL, and incompatible search/order declarations are compile errors; see the complete migration inventory.

Version 0.97.0 adds opt-in, bounded GraphQL dependent deletes on SQLite and PostgreSQL with single-column keys. Declare `dependent_deletes` on each participating entity and its declared deleting parents, then supply a bounded registry. No data migration is needed; see the root migration guide.

Version 0.96.0 bounds Changed operations to one root and requires admission on hand-built schemas. An independent jittered timer rechecks subscriber authority (30-second base; configurable 1–300 seconds): definite denial ends with FORBIDDEN, host failures with SUBSCRIPTION_CONTINUITY_LOST. Per-event denials remain silent. No data migration; see the root migration guide for configuration, client handling and timing limits.

Version 0.95.1 stays aligned with the ORM memory-pool fix; generated code is unchanged. No data migration is needed.

Version 0.95.0 adds opt-in runtime NodeId, managed versions and authorized keyed live subscriptions. Rust runtime collection literals need the two new optional fields; see the root migration guide. Existing serialized schemas and derived APIs are unchanged.

Version 0.95.0 also aligns searchable runtime list arguments in the semantic catalog with SDL.

Version 0.94.0 repairs backend test gating and native MSSQL CI coverage, and fixes paginated relation reads of SQL expression projections. No data migration is needed.
Version 0.94.0 makes `GraphQLRelations` without `#[graphql(complex)]` on an entity that derives `GraphQLEntity`
report one compile error that names the fix, instead of three conflicting-implementation errors. An entity with
`schema_only = true` is exempt; a `GraphQLSchemaEntity` with relation fields now needs `schema_only = true`.
Generated code for every other entity is unchanged.
Version 0.94.0 masks source-bearing callback failures without a string `code` in generated
single-row mutations, matching bulk. Authored refusals retain their message and extensions.
See the root migration guide; no data migration is needed.
Version `0.94.0` keeps runtime/macros aligned for opt-in receiver admission. Generated code is unchanged; no data migration is needed.

Version 0.93.0 is the companion of graphql-orm 0.93.0; macro generation is unchanged.

Version 0.93.0 runs write input transforms once per input row, including each
existing row in bulk updates, with independent outputs and atomic refusal.
Ordinary transaction writes report Repository; hook writes report InternalMutationHook.

Version 0.93.0 adds `mutation_shapes`, which selects the generated mutations an entity
exposes; omission changes nothing.

Version 0.93.0 delivers parent-authorized invalidations after child deletes (#290);
public delete provenance contains no child identity. No data migration is needed.

Version 0.92.0 runs post-commit notifications for GraphQL bulk writes and adds
`notify_on_error = "report"` for single-row success after notification failures.

Version 0.92.0 preserves application refusal messages and codes in generated bulk
write payloads, including denied fields. Bulk result SDL adds nullable `errorCode`.

Version 0.92.0 rejects generated policy-denied field writes by default.
Use `FieldPolicy::ignore_denied_field_write` to retain legacy omission for selected fields.
Version 0.92.0 uses RFC 3339 UTC implicit defaults for String timestamp fields;
integer timestamps retain epoch seconds. Existing defaults and rows are preserved.
See the [migration guide](../../MIGRATION.md#0911-to-0920-type-aware-implicit-timestamps).
Version 0.92.0 makes callback-denied to-one relations indistinguishable from absent targets and enforces relation-field read policies.
Version 0.92.0 generates `MutationContext` keyset reads that bind the entity and caller filter into cursors, matching `graphql-orm` 0.92.0.
See [keyset cursor binding](../../docs/reference/graphql-orm/pagination-migration.md#what-a-keyset-cursor-is-bound-to).

Version 0.92.0 adds opt-in authorized static collection filters with `#[relation(filter)]`.
Filtered keyset cursors are bound to caller-visible typed filter input; restart older filtered cursors.
See [related filters](../../docs/reference/graphql-orm/entities-and-relations.md#authorized-collection-filters).
Version 0.92.0 pairs with the runtime read-predicate additions; generated code is unchanged.

Version 0.89.0 is the companion of graphql-orm 0.89.0 (runtime collection search); macro generation
is unchanged.

Version 0.87.0 generates the fixed `RecordVersion` field for managed record versions and emits
`record_version` semantic metadata for managed and CAS-only versions.

Version 0.86.0 generates the `NodeId` field and its resolvable `@key(fields: "NodeId")` entity
resolver for `#[graphql_entity(node_id)]`.

Version 0.85.0 generates an optional `Keys` argument on every `<Entity>Changed` subscription.

Version 0.84.0 generates managed record versions: `#[graphql_orm(version = "managed")]` is advanced
by every generated write, and generated update and delete accept `ExpectedVersion`.

Version 0.82.0 also pairs with the runtime's shareable aggregate result objects.
The aggregate fix does not change macro generation. See the
[aggregate migration](../../MIGRATION.md#0810-to-0820-shareable-aggregate-result-objects).

Version 0.82.0 adds opt-in `int64` static filters with native `i64` operands,
`Signed64` wire coercion and exact typed metadata. Existing `number` filters
remain 32-bit. See the [signed64 migration](../../MIGRATION.md#0810-to-0820-exact-static-signed64-filters).

Version 0.82.0 allows generated upward Changed hooks to cross Rust crate
boundaries for public entities. Repository-only hooks remain private; current
subscriber operation authority is checked independently at every source/hop,
including after serialization. Transaction/provenance semantics are retained. See the
[cross-crate migration](../../MIGRATION.md#0810-to-0820-generated-cross-crate-upward-changed-hooks).

Version 0.81.1 is the companion of the runtime's post-commit record release; the macros
themselves are unchanged.

Version 0.81.0 remains paired with the runtime actor-stamp contract.

Version 0.80.0 emits canonical typed filter contracts for the actual static
filter implementations, including `notIn`, similarity, date structures and
spatial predicates. Runtime metadata likewise describes its actual operator
surface. Upgrade runtime and macros together with operation catalog 0.6.0;
see the [wire-3 migration](../../MIGRATION.md#0780-to-0800-canonical-typed-filter-contracts).
No resolver policy, GraphQL filter SDL or database format changes.

Version 0.79.0 pairs with the feature-minimal runtime. Generated references
still use `graphql_orm::async_graphql`; independent consumers must select the
needed GraphQL capabilities explicitly. See the
[feature contract](../../docs/reference/graphql-features.md).

Version 0.78.0 is the companion of runtime mutation payload relation pages; the macros
themselves are unchanged.

Version 0.77.0 is the companion of the runtime's date-time backups; the macros themselves are
unchanged.

Version 0.76.0 delimits reserved-word and irregular column and table names in generated
DML. See the [migration guide](../../MIGRATION.md#0750-to-0760-delimited-reserved-word-identifiers).

Version 0.75.0 is the companion of the runtime's automatic timestamps; the macros themselves
are unchanged.

Version 0.74.0 bounds generated index names to PostgreSQL's 63-byte identifier
limit and refuses PostgreSQL plans that would truncate a declared name. See the
[migration guide](../../MIGRATION.md#0730-to-0740-bounded-postgresql-identifiers).

Version 0.73.0 is the companion of the runtime's atomic link changes in writes; the macros
themselves are unchanged.

Version 0.71.0 is the companion of the runtime's composite-key backups; the macros themselves
are unchanged.

Version 0.70.1 is the companion of the runtime's foreign-key replacement order fix; the macros
themselves are unchanged.

Version 0.70.0 is the companion of the runtime's uniform refusal of hidden and
missing link targets; the macros themselves are unchanged. See the
[migration guide](../../MIGRATION.md#0690-to-0700-uniform-refusal-of-hidden-and-missing-link-targets).

Version 0.69.0 adds many-to-many runtime relations: ORM-owned link tables,
batched join reads and authorized link writes. See the
[migration guide](../../MIGRATION.md#0680-to-0690-many-to-many-runtime-relations).

Version 0.68.0 adds validated, fingerprinted runtime collection label fields and
publishes them in the semantic catalogue. Rust struct literals of
`RuntimeCollection` need the new member; see the
[migration guide](../../MIGRATION.md#0670-to-0680-runtime-collection-label-fields).

Version 0.67.0 adds opt-in runtime relation filters (correlated `EXISTS` with
the target grant inside) and to-one relation selections in runtime mutation
payloads. See the
[migration guide](../../MIGRATION.md#0660-to-0670-runtime-relation-filters-and-payload-relations).

Version 0.66.0 lets runtime deletes opt in to authorized, ORM-performed
`Cascade`/`SetNull` dependent changes. See the
[migration guide](../../MIGRATION.md#0650-to-0660-opt-in-runtime-cascade-and-set-null-deletes).

Version 0.65.0 adds `#[graphql_policy_object]` before the derives for the
`GraphQLEntity + SimpleObject #[graphql(complex)] + GraphQLRelations` pattern.
It installs backend-specific ORM scalar policy guards, preserves existing
guards and generated relations, and omits unreadable/private scalar fields.
See the [migration guide](../../MIGRATION.md#0641-to-0650-scalar-policies-with-generated-relations).

Version 0.64.1 classifies SQLite foreign-key `RESTRICT` refusals as constraint
violations, matching PostgreSQL. See the
[migration guide](../../MIGRATION.md#0640-to-0641-sqlite-restrict-refusal-classification).

Version 0.62.0 adds optional typed runtime `orderBy` to generated keyset queries.
The scalar-only input, complete primary-key suffix, order-bound cursor and current
field authorization are described in the
[migration guide](../../MIGRATION.md#0610-to-0620-runtime-keyset-ordering).

Generated Changed delivery prepares the selected snapshot before yielding a
stream item. Late scalar refusals and policy errors, including nullable fields,
discard the entire event and private diagnostics without an error frame. A
later authorized event starts with clean response errors.

Version 0.64.0 generates complete composite-key and read-only projection change
subscriptions independently of mutation exposure. See the
[migration guide](../../MIGRATION.md#0630-to-0640-composite-and-read-only-changed).

Version 0.63.0 resolves generated upward propagation using complete relation
from/to tuples and revalidates the current mapping at release. See the
[migration guide](../../MIGRATION.md#0620-to-0630-propagation-relation-tuples).

Version 0.61.0 adds an optional typed `key` argument to generated single-primary-key
change subscriptions, preserving current authorization and action filtering. See the
[migration guide](../../MIGRATION.md#0600-to-0610-subscription-primary-key-selection).

Version 0.60.0 adds explicit generated query-shape selection for controlled
coexistence with recorded legacy roots. Defaults are unchanged; see the
[migration guide](../../MIGRATION.md#0590-to-0600-generated-query-shape-selection).

Version 0.58.0 extends explicit host-owned timestamps to public entities and String
values without replacing source timestamps with the mutation clock. See the
[migration guide](../../MIGRATION.md#0570-to-0580-public-host-owned-timestamps).

GraphQL-private configured ordering fields remain unavailable even when
`read = true` or `order = true` is explicitly set. Repository field policy and
permitted repository pagination retain their independent behavior.

Configured keyset cursors require current read authority for every order field
and complete primary-key suffix, even for cursor-only selections. Checks run
before queries and again against returned rows. Private, skipped or transformed
GraphQL order fields fail closed; repository field policy remains independent.

Version 0.59.0 supports static keyset reads with a complete composite-primary-key
tiebreaker tuple. Schema ownership and write posture are unchanged. See the
[migration guide](../../MIGRATION.md#0580-to-0590-composite-primary-key-keysets).

Version 0.57.0 adds the SQL Server-only `#[graphql_orm(mssql_money)]` field
annotation and identifier-safe exact read projections for native `MONEY`.
It pairs with runtime 0.57.0; see the
[migration guide](../../MIGRATION.md#0560-to-0570-exact-sql-server-money-reads).

Version 0.56.0 adds runtime descriptions, classification and export with a runtime
semantic catalogue, and an exact runtime decimal kind; see the
[workspace migration](../../MIGRATION.md#0551-to-0560-runtime-descriptions-and-exact-decimals).
Macro output is unchanged.

Version 0.55.1 refreshes the backend-coexistence fixture lock for graphql-orm-ai
0.109.0. Source, public API, generated code, SDL and persistence are unchanged.

Version 0.55.0 pairs runtime/macros for typed runtime literal defaults, semantic field
formats and reviewed runtime GraphQL projections; see the
[workspace migration](../../MIGRATION.md#0540-to-0550-typed-literal-defaults-and-semantic-field-formats).

Version 0.54.1 pairs the runtime/macros with the reviewed Auth 0.22.1 signed
role-catalogue correction. Generated code, SDL and persistence are unchanged.
Adopt one coordinated published workspace/Auth source; see the
[workspace migration](../../MIGRATION.md#signed-role-catalogue-transport-bound).

Version 0.54.0 pairs runtime/macros for durable cache refresh coordination.
Generated status-entity subscriptions retain post-commit current authorization;
clients reconcile initial/query races and refetch after reconnect or gaps. See
[cache synchronization](../../docs/reference/graphql-orm/cache-sync.md).

Version 0.53.0 aligns ORM/macros for post-await current-authority revalidation
and owned custom release checks. Policies remain read-only admission callbacks;
resource ownership, deleted-preimage and commit fences remain host-owned. No
generated code, SDL or schema migration is added. See
[the bridge guide](../../docs/reference/graphql-orm/agql-auth-bridge.md).

The current workspace selects published Auth `v0.22.0` at
`997707e060bcf5fabf879a5f98146dc2732b1c99`. Use that tag for direct Auth
dependencies and refresh the lockfile to one source. The
[0.53 migration](../../MIGRATION.md#0520-to-0530-post-await-and-owned-release-checks)
describes validated current-session proofs; earlier version notes below retain
their historical selectors.


Version 0.52.0 adds explicitly selected native Rust 1.98.1 owned replay evidence.
Historical entry points retain exact qualified 1.97.1 behavior. The two recipes
preserve compiler-native hashes and have separate admission; format 1, default-off
features, generated code and dependencies are unchanged. Upgrade ORM/macros
together; see the [migration guide](../../MIGRATION.md#0510-to-0520-explicit-native-rust-1981-replay-recipe).

Version 0.51.0 adds the runtime's default-off `owned-planning-replay` capability.
Upgrade ORM/macros together; generated code is unchanged. Explicit capture retains
complete sensitive schema evidence; offline verification returns passive facts.
See the [replay contract](../../docs/reference/graphql-orm/owned-planning-replay.md).

Version 0.50.2 aligns the paired runtime/macros release with published
`agql-auth` `v0.21.3`, retaining the same reviewed Auth commit. Use that tag
selector for direct Auth dependencies as well; refresh the lockfile and verify
one Auth source. No runtime behavior, generated code or schema migration is
added. See [source alignment](../../MIGRATION.md#0501-to-0502-published-auth-source-alignment).

Version 0.50.1 adds checked original filter intent and conservative offline
runtime-record matching in the core runtime. Upgrade ORM/macros together; no
stored-data or schema migration is required. Matching does not authorize a row,
prove snapshot provenance or establish native RLS for a deleted record. See
[the filter migration guide](../../MIGRATION.md#0490-to-0500-checked-filter-intent-and-offline-matching).

The paired 0.48.3 release rejects unsupported PostgreSQL column constraint-flag
or spatial-metadata-only alterations before managed target DDL/history rows.
Legacy staged planning may initialize empty bookkeeping; direct/owned paths
reject before runtime preparation. Infallible previews remain structural views;
see the root migration guide for the supported boundary.

PostgreSQL migration default alterations are corrected in the paired 0.48.2
runtime/macros release: future omitted-value inserts use the changed default;
existing rows and recorded migration history are not rewritten. See the root
migration guide for guarded repair of previously incomplete applications.


This is the procedural-macro implementation for `graphql-orm`. Applications
normally depend on the runtime crate and import its prelude; that keeps the
macro/runtime versions aligned:

```toml
[dependencies]
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.80.0", default-features = false, features = ["sqlite"] }
```

Direct use is supported for tooling that needs the macro package:

```toml
graphql-orm-macros = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.80.0", default-features = false, features = ["sqlite"] }
```

The direct dependency still requires a compatible `graphql-orm` runtime in the
consumer crate because generated code refers to `::graphql_orm`. The macros do
not connect to a database, run migrations, host GraphQL, or authorize requests.

Version 0.39.1 aligns with the runtime string/binding correction; generated
static fragment rebasing, inputs, GraphQL SDL and timestamp behavior are unchanged.

Version 0.48.1 aligns with the runtime PostgreSQL TEXT default-comparison fix;
macro generation is unchanged.

Version 0.46.0 requires explicit finite scan budgets for generated residual-policy
offset lists and all generated fallback search, including database-RLS-only
deployments without a RowPolicy. Exact counts are returned only when
scanning completes; missing/exhausted budgets fail closed without partial data.
See the [bounded read migration](../../MIGRATION.md#0450-to-0460-bounded-protected-offset-and-search-reads)
for configuration and AI/tool consumer error handling.

Version 0.45.0 rechecks generated subscription authorization for each event,
releases current authorized live rows and requires bounded source provenance.
Denied fields/rows and missing proofs suppress the whole event, including
metadata. Deleted snapshots additionally require explicit default-deny
`RowPolicy::can_read_deleted_preimage` opt-in. A missing provider or ordinary read
permission alone does not authorize them, including under database RLS.
See the [subscription migration](../../MIGRATION.md#0441-to-0450-subscriptions-authorize-each-release)
for deleted preimages, raw event observers, propagation limits and action filters.

## Public macros

| Item | Purpose |
| --- | --- |
| `GraphQLEntity` | GraphQL object/filter/order/input types, row decoding, metadata, and query helpers |
| `GraphQLSchemaEntity` | schema metadata only |
| `RepositoryEntity` | typed repository CRUD and private projections with no GraphQL surface |
| `GraphQLRelations` | batched single/composite-key relation resolvers |
| `graphql_complex_object` | handwritten complex fields composed with generated relations |
| `GraphQLOperations` | generated GraphQL root operation types and operation metadata |
| `schema_roots!` | query/mutation/subscription roots, schema builders, metadata, and resolved catalog |
| `graphql_orm_custom_operations` | semantic metadata emitted beside a handwritten root impl |
| `GraphQLSemanticObject` | classified public field metadata for a handwritten result object |
| `mutation_result!` | a simple GraphQL mutation result object |
| `backend_selected_graphql_entity` | emits cfg-selected entity definitions for a multi-backend consumer |

For a handwritten root annotated by `graphql_orm_custom_operations`, prefer
the matching `described_query_types`, `described_mutation_types`, or
`described_subscription_types` list in `schema_roots!`. One entry composes the
root and automatically includes its operation descriptors and direct
`GraphQLSemanticObject` result metadata. The older `extra_*` plus
`semantic_custom_operations`/`semantic_types` lists remain compatible for
incremental migration, but a described root cannot also appear in them.

`GraphQLOperations` also owns Federation entity keys. An entity declaring
`#[graphql_entity(federation_key)]` receives one `#[graphql(entity)]` resolver
per key on its generated queries object, which is the only construct
async-graphql turns into a resolvable `@key`. Declaring a key without this
derive is a compile error. See
[federation](../../docs/reference/graphql-orm/federation.md).

Custom scalar and enum results are fail-safe by default. To make one eligible
for provider disclosure, declare `result_classification` and `result_export`
together on the resolver method. An exportable scalar/enum list also requires
positive `result_maximum_items`; `secret` may only use `never_export`.

## Feature configuration

`sqlite` is the default. Select `postgres` or `mssql` by disabling default
features. The macro compiler rejects a build with no backend and a consumer
must select an explicit backend when Cargo feature unification enables more
than one. `mysql` exists as a macro feature but the runtime package does not
offer a MySQL backend in this release; do not use it as application support.

The independent `resolver-case-*`, `argument-case-*`, and `field-case-*`
groups each permit at most one feature: `pascal`, `snake`,
`screaming-snake`, `lower`, or `upper`.

## Minimum use

```rust
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(table = "notes", plural = "Notes")]
struct Note {
    #[primary_key]
    id: i64,
    #[filterable(type = "string")]
    #[sortable]
    body: String,
}
```

The generated surface depends on the selected backend and schema policy.
MSSQL and `external_read_only` schemas retain reads but omit generated
mutations and subscriptions. Repository-only entities intentionally report no
generated GraphQL operations.

## Authoritative reference and security

The [macro and attribute reference](../../docs/reference/graphql-orm/macros-and-attributes.md)
is the canonical accepted-syntax, defaults, constraints, naming, relation,
index, projection, search/spatial, schema-policy, and generated-authorization
reference. Attribute metadata is not authorization: resolver auth, scope
metadata, semantic descriptions, and operation fingerprints never replace
application row/field policy or database controls.

Generated write resolvers and repository helpers lower authorization-sensitive
reads and DML onto one driver-neutral mutation transaction. Existing-row
decisions use backend write locks, predicate mutations materialize typed
primary keys before exact-key DML, and absent-key upserts require the runtime's
state-machine isolation mode. Consumers do not provide SQL or lock clauses.

Generated to-many relation resolvers accept one nullable `OrderByInput`
object, while generated root list resolvers retain their nullable list of
non-null ordering objects. The macro derives relation resolver signatures and
semantic relationship descriptors from the same internal contract so public
names, nullability and `Where`/`OrderBy`/`Page` shapes remain byte-equivalent.

Conditional relations accept `source_condition(field = "...", equals = ...)`
or `target_condition(column = "...", equals = ...)` with string, integer,
float, or boolean literals. Source fields are compile-time type checked; target
columns are quoted by the selected backend and all values are bound. These
logical discriminator joins require `emit_fk = false` and are enforced by
single, pageable, DataLoader, and nested bulk-preload paths.

Server-defined computed ordering uses repeatable entity-level
`graphql_orm(order_expression(name = "...", expression = "..."))`
declarations. An expression containing `:named` bind parameters also declares
`parameters = "server_function_path"`; that function returns
`OrderExpressionParameters` from the GraphQL server context. The generated
input exposes only `OrderDirection`; the fixed, validated expression and bind
names remain compile-time server configuration. Raw backend placeholders are
rejected, and generated pagination adds missing primary-key tie-breakers.
Entities that need relationship counts can add
`order_aggregate(name = "...", aggregate = "count")` to an unconditional,
readable relation without a `read_policy`; the generated correlated count uses
only its declared key mapping and target entity table, and counts only target
rows the request may read. Entities that also have handwritten complex fields use
`graphql_orm(compose_complex_object)` and apply `graphql_complex_object` to the
handwritten impl so generated relations are flattened into the same
`ComplexObject` surface.

Generated `DateFilter` fields use sargable half-open calendar ranges and a
recursive validation path shared by GraphQL and programmatic queries. Between
bounds are required; calendar spans and signed relative offsets are bounded.
The generated semantic operator set follows the date grammar rather than the
field's Rust backing type.

See [core runtime documentation](../graphql-orm/README.md),
and the [macro and attribute reference](../../docs/reference/graphql-orm/macros-and-attributes.md).

## Authorization-aware pagination

Version 0.33 adds typed `RowPolicy::read_visibility` decisions, SQL-authorized
list/keyset pagination, and opt-in bounded callback scans on keyset-enabled
entities. Existing callback list offsets/counts remain exact. See the
[pagination guide](../../docs/reference/graphql-orm/pagination-migration.md)
for configuration, generic examples, continuation semantics, and limitations.

`RepositoryEntity` aggregate-field enums use only ordinary Rust derives and no
`#[graphql]` helper attributes. The repository surface guard also rejects an
accidentally retained async-graphql `Enum` derive. Ordinary `GraphQLEntity`
aggregate enums keep their GraphQL derive and naming attributes. See the
[external runtime consumer](../graphql-orm/tests/fixtures/repository-aggregate-consumer/src/lib.rs)
for compiled plain Rust aggregate calls without a direct async-graphql dependency.

Version 0.33.3 gives the repository aggregate compatibility fix a distinct package
identity after the independent 0.33.2 fixture maintenance. Generated behavior is
unchanged from the reviewed aggregate fix.

Version 0.35.1 generates projection identity for complete SQL row visibility and
aligns with the core policy-aware projection fix. Existing generated
aggregate enums/builders and ordinary GraphQL aggregate SDL remain unchanged;
repository-only consumers still need no direct async-graphql dependency.

Repository-only Integer timestamps can use per-field `#[graphql_orm(timestamp = "host")]`.
See [host timestamp inputs, migration compatibility and cancellation limits](../../docs/reference/graphql-orm/repository-only-entities.md#host-managed-integer-timestamps).

Complete SQLite/PostgreSQL text-group pages and SQL-visible ordinary aggregates: see [typed aggregates](../../docs/reference/graphql-orm/typed-aggregates.md).

Version 0.43.1 fixes typed binding for host-supplied integer primary keys and retains the 0.43.0 runtime additions. Released host-managed
timestamps, framework-neutral repository aggregate helpers and static GraphQL SDL
remain unchanged.

Authorization-aware loaders use complete structural partitions in 0.44.0.
External authorization keys use the versioned `auth-v2:` digest; invalidate old
cache identities when upgrading. SQL-value relation keys preserve binary bytes
and SQL types. See the [0.44.0 migration guide](../../MIGRATION.md#0431-to-0440-authorization-and-relation-partition-identities)
for host cache and manually constructed key changes.

Version 0.44.1 sanitizes generated backend failures and legacy mutation error
payloads. Match stable GraphQL public error codes rather than SQL/driver text;
see the [public error migration](../../MIGRATION.md#0440-to-0441-generated-resolver-public-errors).

Database authorization defaults to `DeclaredPoliciesRequired` in 0.47.0. Install
providers for marked policies; sensitive applications should select
`ExplicitPolicyForAllExposedOperations` and enforce tenant/business boundaries.
With `auth-agql`, retain a coherent full accepted principal and wrap sensitive
entity policies with immediate current-authority checks. See the
[current-authority bridge](../../docs/reference/graphql-orm/agql-auth-bridge.md)
and [upgrade guide](../../MIGRATION.md#0460-to-0470-current-authority-and-coherent-accepted-requests).

Version 0.90.0 fixes empty-list filter semantics, generated Changed catalogue
arguments and propagation path authorization. See the workspace
[MIGRATION.md](../../MIGRATION.md#0890-to-0900-restrictive-empty-filters-and-complete-change-provenance) before upgrading.

Version 0.90.1 permits declared subscription scopes on composite and read-only
Changed roots and fixes generated descriptions. It adds no migration steps; see
[the migration guide](../../MIGRATION.md#0900-to-0901-composite-subscription-authorization).


Version 0.91.1 keeps this crate aligned with `graphql-orm` 0.91.1. Generated code matches 0.91.0.

Version 0.91.0 scores SQLite runtime search from each row's own document, so
hidden rows cannot affect visible scores, and fences the runtime search
backfill against a stale schema. See
[the migration guide](../../MIGRATION.md#0901-to-0910-isolated-runtime-search-ranking-and-fenced-backfill).

Version 0.92.0 pairs the runtime and macros for narrow-key paginated relation windows.

Version 0.92.0 pairs the runtime and macros for selection-aware static offset connections.
Version 0.92.0 adds the off-by-default macro `schema-export-only` feature for dedicated
SDL export builds. Runtime builds must leave it disabled; see
[schema export](../../docs/reference/graphql-orm/schema-export.md).

Version 0.93.0 shares generated scalar-filter SQL and semantic metadata builders to reduce consumer compile cost. SQL, binds, SDL and public contracts are unchanged; no data migration is needed.
