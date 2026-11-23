---
title: "graphql-orm"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-19
review_by: 2027-02-01
supersedes: []
---

# `graphql-orm`

Version 0.214.0 lets a mutation hook's nested writes be authorized from the
originating request, as an opt-in, and passes an explicit `PolicyAuthority` to
entity, row and repository field policies. See the
[migration guide](../../MIGRATION.md#02070-to-02140-explicit-authority-for-hooks-and-workers).

Version 0.207.0 renders SQLite closure read predicates in the original form
again (#876) and reduces SQL duplication for directly nested closure read
predicates on PostgreSQL and SQL Server. See the [migration note](../../MIGRATION.md#02060-to-02070-sqlite-closure-rendering-and-nested-closure-read-predicates).

Version 0.206.0 exposes full backup and restore on explicitly typed SQLite
and PostgreSQL handles in multi-backend builds. See the
[migration guide](../../MIGRATION.md#02050-to-02060-backend-typed-backup-runtime).

Version 0.205.0 reduces per-entity compile work for generated Changed
subscriptions by sharing their stream implementation. See the
[migration guide](../../MIGRATION.md#02040-to-02050-generated-changed-subscriptions).

Version 0.204.0 adds an opt-in for databases whose backup and restore are
owned by another product. See the [migration guide](../../MIGRATION.md#02030-to-02040-externally-managed-backup-and-restore).

Version 0.203.0 adds SQL Server prerequisites for durable AI stores. See the
[SQL Server DDL reference](../../docs/reference/graphql-orm/mssql.md#introspection-and-rendered-ddl).

Version 0.202.0 groups additional integration tests and documents the blocking
migration file writer. See the [test selectors](../../docs/development/testing.md).
See the [migration guide](../../MIGRATION.md#02010-to-02020-test-and-repository-hygiene).

Version 0.201.0 returns duplicate entity identities as errors from the
generated resolver catalogue and fallible schema builders. See the
[migration guide](../../MIGRATION.md#02000-to-02010-duplicate-entity-identities-operation-catalogue-0120).

Version 0.200.0 includes the
[standalone closure rendering update](../../MIGRATION.md#01990-to-02000-closure-read-predicates).

Version 0.199.0 adds derive-level opt-outs for generated mutations and Changed
subscriptions. See [omitting generated roots](../../docs/reference/graphql-orm/macros-and-attributes.md#omitting-generated-roots).
Generated mutations, entity getters, and relations return a curated field error
when the schema lacks the database.

Version 0.198.0 supports size checks for AI restore pages.
See the [migration guide](../../MIGRATION.md#01970-to-01980-restore-page-size-checks).

Version 0.197.0 preserves SQL Server driver diagnostics when decoding rows
and centralizes the decision for a missing authorization policy provider.
For handwritten SQL, bind a typed NULL that matches the destination column.
Runtime search supports collection columns named like its internal search
key and score.
See the [migration guide](../../MIGRATION.md#01960-to-01970-database-binding-and-decoding).

Version 0.196.0 adds opt-in [computed-field invalidations](../../docs/reference/graphql-orm/entities-and-relations.md#computed-field-invalidations)
when a committed related-state write should prompt an authorized client to
refetch a computed value. The signal contains the target ID and a public topic.

Version 0.195.0 groups selected ORM integration tests. The
[testing guide](../../docs/development/testing.md) lists the grouped selectors.

In version 0.194.0, `schema_roots!` also emits `graphql_orm_resolver_catalog()`: the generated
relationship fields and Federation entity resolvers of the schema, which no operation
catalogue lists. It is descriptive, names only what the SDL publishes, and
changes no existing catalogue or fingerprint. See
[generated resolver operation metadata](../../docs/reference/graphql-orm/resolver-operation-metadata.md#relation-and-entity-resolvers).

In version 0.193.0, the object `GraphQLEntity` generates can host handwritten
resolver fields: declare `#[graphql_orm(object_fields)]` on the entity and put
`#[graphql_object_fields]` on one impl of it. Entities that do not opt in are
unchanged.
See the [migration guide](../../MIGRATION.md#01920-to-01930-handwritten-fields-on-generated-objects).

Version 0.192.0 adds `propagate_change_action = "updated"` for upward relations.
See the [relation attribute reference](../../docs/reference/graphql-orm/macros-and-attributes.md#relations-and-foreign-keys).

Version 0.191.0 adds [read snapshot transactions](#read-snapshot-transactions)
for policy-aware reads on SQLite, PostgreSQL, and SQL Server.

In version 0.190.0, generated query roots, Changed subscriptions and the
NodeId object field return field errors when required schema data is missing.
The generated `schema_builder(database)` registers the data for its roots.
See the [migration guide](../../MIGRATION.md#01890-to-01900-generated-query-resolvers).

Version 0.189.0 qualifies generated root search filters with the entity table
(#799). See the [migration guide](../../MIGRATION.md#01880-to-01890-root-search-filter-binding).

Version 0.188.0 binds every column a read predicate renders to its own row: through the entity's
table path at the root of a statement, and through an alias no other scope uses inside a relation,
`not_related`, closure or count subquery. A column missing from a target table then fails the read
instead of resolving against an outer row (#796). The generated alias names are
reserved as table names, and a hand-written `ReadVisibilityFilter` may only hold single-row
expressions, with every column written through `__gom_self` where it is evaluated in a subquery.
See the [migration guide](../../MIGRATION.md#01870-to-01880-read-predicate-columns-bound-to-their-row).

Version 0.187.0 is the **breaking release**: every change that breaks hosts or
invalidates stored fingerprints ships in it, and the
[migration guide](../../MIGRATION.md#01860-to-01870-breaking-release) lists the
steps in upgrade order. In short:

- **Schema and storage.** Migration plans no longer drop tables the target
  does not declare; lossy column type changes and integrity-removing steps
  need `allow_destructive`; applying a plan refuses, before any statement
  runs, a new unique index whose key existing rows repeat
  (`UniqueIndexDuplicates`); the PostgreSQL change journal refuses `TRUNCATE`
  and unjournaled cascades.
- **Build and API.** GraphQL naming is selected by the
  `GRAPHQL_ORM_RESOLVER_CASE`, `GRAPHQL_ORM_ARGUMENT_CASE` and
  `GRAPHQL_ORM_FIELD_CASE` build variables instead of Cargo features; a schema
  built by hand from generated roots installs `GeneratedInputValidation`;
  each schema's maximum page limit is fixed at compile time
  (`schema_roots!(max_page_limit: N)`, 100 when omitted); host-side keyset
  pages and `paginate(PageInput)` reads refuse a limit above the maximum
  instead of returning fewer rows; `SqlValue` is non-exhaustive and gains
  `Temporal`; the policy-free relation preload API and direct relation
  data-loader use are removed.
- **Defaults.** Generated GraphQL bulk writes are narrowed to the caller's
  read visibility (`narrow_bulk_writes_to_read_visibility(false)` opts out;
  `mssql_snapshot_bulk_write_targets` stays a separate opt-in); a `Database`
  refuses a client page offset above 10,000 (`max_page_offset(None)` opts
  out); an uncoded host callback error reads as `INTERNAL_ERROR` unless the
  schema's `Database` opts in; `ReadPredicate::related` is three-valued, so
  its negation never admits a NULL key or an UNKNOWN-only relation, and
  `not_related` is `related(..).not()`; runtime GraphQL serves `RecordVersion`
  and integer live keys as `RuntimeInt64` strings.
- **Fingerprints.** With operation catalogue 0.10.0, generated-operation
  fingerprints no longer include the Rust module path: moving an entity keeps
  them, and every fingerprint changes once at this upgrade. `stable_type_id`
  is still accepted and no longer affects fingerprints.
- **Opt-in additions.** SQL Server binds chrono values natively, and pools can
  opt in to native binds for annotated temporal text and date filters with
  `MssqlPoolOptions::typed_temporal_parameters(true)`;
  `#[graphql_entity(key_only_deletes)]` adds a key-only delete signal for
  `Key`/`Keys` subscribers.

Upgrade ORM and macros together.

Version 0.186.0 fixes startup races in the owned PostgreSQL test harness.
See the [migration guide](../../MIGRATION.md#01850-to-01860-owned-postgresql-test-readiness).

Version 0.185.0 renders SQL Server filtered-index strings as Unicode literals,
including ASCII values. Filters introspected as opaque SQL are replayed
verbatim. See the
[migration guide](../../MIGRATION.md#01830-to-01850-sql-server-filtered-index-literals).

Version 0.182.0 fixes SQL identifier quoting: only exactly one well-formed delimited identifier
is passed through unchanged, and anything else is quoted as one literal name (#782); SQL Server
writes a name bare only under its regular-identifier rule (#783).
See the [migration guide](../../MIGRATION.md#01810-to-01820-sql-identifier-quoting).

Version 0.181.0 supports the transitive-closure read predicates on SQL Server, as an unrolled
breadth-first search with the same semantics as the recursive CTE on SQLite and PostgreSQL;
SQL Server accepts bounds up to `READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH` (64). Read predicates
are budgeted at build: two nested closures, 1 MiB of rendered SQL, 256 expression levels, 10 nested
`CASE` expressions, and 32 nested and 128 total relations (a closure counting as 16), plus 75
subqueries, 9 nested and 9 total relations (a closure counting as 1) and 900 weighted levels on SQL
Server. Each statement's relation filters and count orderings may add 64 relations more (5 on SQL
Server), and a generated `orderBy` list holds at most 16 terms. `and`/`or` folds are flattened, and
lists render as balanced pairs.
See [read-predicate transitive closures](../../docs/reference/graphql-orm/pagination-migration.md#read-predicate-transitive-closures).

Version 0.180.0 adds the opt-in `DatabaseBuilder::mssql_snapshot_bulk_write_targets`, off by
default and separate from `narrow_bulk_writes_to_read_visibility`. With both on and
`READ_COMMITTED_SNAPSHOT` on, narrowed SQL Server bulk writes select their targets from
snapshots and lock only those keys, so they no longer lock or wait on hidden rows (#752). The
target set equals the rows matching at the validation snapshot, but the bulk write's transaction
is then not serializable. See the
[migration guide](../../MIGRATION.md#01791-to-01800-opt-in-sql-server-snapshot-bulk-write-targets).

Version 0.179.1 memory-caps test-owned PostgreSQL containers (test harness only; no runtime change).

Version 0.179.0 delimits a SQLite name that starts with U+FEFF, which SQLite would otherwise
read as whitespace, and adds seeded property suites for identifier quoting on every backend
and for canonical JSON defaults ([#725](https://github.com/Dastari/graphql-orm/issues/725)).

Version 0.177.0 adds the opt-in
`DatabaseBuilder::narrow_bulk_writes_to_read_visibility`, which narrows the
locking read of generated GraphQL `updateMany` and `deleteMany` to the SQL
predicate of the row policy's `read_visibility` when no
`write_target_prefilter` is declared, so rows outside the caller's read
visibility are not passed to row callbacks, and on PostgreSQL not locked
(#752). Off by default. The root delete of a `dependent_deletes` entity now
reports its `errorCode` under `mutation_error_codes` (#763). See the
[migration guide](../../MIGRATION.md#01760-to-01770-bulk-write-target-visibility-and-coded-dependent-delete-refusals).

Version 0.176.0 adds transitive-closure read predicates: `ReadPredicate::related_closure`,
`related_closure_all` and `related_closure_within`, rendered as a recursive CTE on SQLite and
PostgreSQL and refused on SQL Server.
See [read-predicate transitive closures](../../docs/reference/graphql-orm/pagination-migration.md#read-predicate-transitive-closures).

Version 0.174.0 adds fail-closed read-policy negation: `ReadPredicate::not_related`,
`ReadPredicate::is_null` and `is_not_null`, and refuses `not()` over a predicate that renders
the deny fallback `1 = 0` (an empty, over-limit or invalid filter list or value).
See [read-predicate negation and NULLs](../../docs/reference/graphql-orm/pagination-migration.md#read-predicate-negation-and-nulls).

Version 0.173.0 declares Rust 1.97.1 as the minimum supported compiler. CI verifies
the workspace and explicit backend/provider graphs with Rust 1.97.1.

Version 0.172.0 adds opt-in client-facing metadata: a build-time client
metadata export (`schema_roots!(client_metadata: true)`) with `Changed` stream
carried fields, lifetime fields and relationship key mappings, an opt-in
`Changed` started signal, an opt-in aggregate source entity, and
`graphql_orm_interpretation_fingerprint`. Defaults are unchanged. See
[client-facing schema metadata](../../docs/reference/graphql-orm/client-metadata.md)
and the [migration guide](../../MIGRATION.md#ormmacros-01720-and-operation-catalogue-081-client-facing-metadata).

Version 0.171.0 adds an opt-in client error contract with one code vocabulary
shared with the router: `SUBSCRIPTION_SELECTION_UNSUPPORTED` for an
unstreamable `Changed` selection
(`DatabaseBuilder::subscription_selection_unsupported_code`),
`extensions.outcome` on generated mutation errors
(`DatabaseBuilder::mutation_error_outcome`, see `MutationOutcome`), and
`errorCode` on the single-row results of entities declared with
`mutation_error_codes`. Nothing changes without them. See the
[error-code reference](../../docs/reference/graphql-orm/error-codes.md#one-vocabulary-with-the-router)
and the
[migration guide](../../MIGRATION.md#01700-to-01710-opt-in-client-error-contract).

Version 0.170.0 refuses, on generated static schemas, a value that is not an
object where an input object is expected, and on static and runtime schemas a
variable at a position the GraphQL rule does not allow, with
`invalid_graphql_input` before any resolver runs. Spec-valid requests are
unchanged. Clients that send such variable usages, and hosts that build
schemas from generated roots by hand, need action. See the
[migration guide](../../MIGRATION.md#01690-to-01700-input-object-values-on-static-schemas-and-bulk-write-filter-authority).

Version 0.169.0 adds an opt-in client page offset bound on every backend
(`DatabaseBuilder::max_page_offset`, refused with `PAGE_LIMIT_EXCEEDED`) and
fixes SQL Server details: a `DECIMAL(38,38)` value is a decode error instead
of a driver panic, introspection reads indexes, key `String` columns are
indexable `NVARCHAR(450)`, rendered `ALTER COLUMN` keeps the column's
collation, and the restore marker checks its key and constraint. See the
[migration guide](../../MIGRATION.md#01670-to-01690-sql-server-correctness-and-a-client-offset-bound).

Version 0.167.0 changes test code only: the macros unit tests no longer assume
the SQLite backend, so they run under each backend feature. No generated code
or behavior changes.

Version 0.166.0 adds opt-in transaction-bound dependent-effect verification.
`DependentDeleteGuard` compares declarations with the introspected delete plan
and enforces dependent row limits before writes. Generated physical opt-out and
bulk deletes refuse guarded requests. Unchanged hosts keep existing behavior.
See the [migration guide](../../MIGRATION.md#ormmacros-01660-guarded-dependent-deletes).

Version 0.165.0 makes snapshot restore atomic on SQLite and PostgreSQL,
resets restored runtime search indexes, and preserves denied fields on upsert.
MSSQL hosts must install `dbo.__graphql_orm_restore_marker` with
`MssqlPool::install_restore_marker()` and grant runtime roles SELECT before
serving ORM work. Missing or unreadable restore state fails closed with
`RestoreStateUnavailable`, whose reason names the fix. See the
[migration guide](../../MIGRATION.md#01640-to-01650-atomic-restore-search-reset-and-kept-values-on-ignored-denials).

Version 0.164.0 adds opt-in contract settings and one stable code. A GraphQL
page limit above the maximum can be refused with `PAGE_LIMIT_EXCEEDED` instead
of being reduced (`DatabaseBuilder::refuse_oversized_page_limit`), and
`schema_roots!(max_page_limit: N)` publishes the real maximum in the semantic
catalogue and refuses a `Database` configured differently. Runtime GraphQL can
serve `RecordVersion` and integer live keys as `RuntimeInt64` strings
(`with_int64_identity_strings`). A runtime page whose stored sort key is too
long for a cursor is refused with `cursor_key_too_large`. A subscriber that
lagged now ends on the next authority tick instead of at once. Defaults are
unchanged except for that timing. See the
[migration guide](../../MIGRATION.md#01630-to-01640-page-limits-64-bit-identities-cursor-keys-and-subscription-lag).

Version 0.163.0 lowers the compile cost of `GraphQLOperations`. The seven
per-entity wrapper objects (edges, connections, mutation results, change
events, bulk results and authorized scan pages) carry hand-emitted output
impls instead of a `SimpleObject` derive. Generated SDL, catalogues and
fingerprints are byte-identical; no action is needed. See the
[migration guide](../../MIGRATION.md#01590-to-01630-wrapper-object-compile-cost).

Version 0.162.0 adds context-bound authorized pages for handwritten GraphQL
resolvers and query builders. `EntityQuery::fetch_authorized_page` seals its
continuation with a service-owned key under a server-authored
`AuthorizedPageBinding`, refuses a continuation from a different context
before any row is read, and resolves entity, row and field authorization again
on every page. The API is additive and needs no action. See the
[migration guide](../../MIGRATION.md#01620-context-bound-authorized-pages) and the
[reference](../../docs/reference/graphql-orm/pagination-migration.md#context-bound-authorized-pages-for-handwritten-resolvers-01620).

Version 0.159.0 fixes PostgreSQL static tables declared with mixed-case bare
names. Table references follow PostgreSQL's historical folding, so migrations,
reads/writes, RLS and search helpers resolve the same table while declared
metadata and helper digests stay unchanged. Existing installs need no rename. See the
[migration guide](../../MIGRATION.md#01580-to-01590-postgresql-table-name-case).

Version 0.158.0 closes follow-ups from the recent batches. The runtime GraphQL
response budget reserves every selected layer's structure and stored fields
before a projected host object is copied, and the read path no longer copies
records before their bytes are reserved. On SQLite a runtime relation layer
with more than 999 bound values is sent as several statements, and one parent
group above that is refused before anything is sent. Search helper table names
are bounded to PostgreSQL's 63 bytes, which is breaking only for a searchable
base table name longer than 42 bytes or ending in `_` and 16 hexadecimal
digits: its helper table is renamed once. A helper table name that is already
taken is refused before a migration changes anything. A poisoned request error
buffer and an unknown aggregate operator are typed errors instead of panics.
See the
[migration guide](../../MIGRATION.md#01570-to-01580-runtime-budget-search-helper-names-and-typed-refusals).

Version 0.157.0 fixes data-safety defects in migrations, the change journal
and retention. One change is breaking: `export_changes` pages the change
journal by a new monotonic `seq` column (`ChangeWindow::after_sequence`,
`BackupChange::sequence`) instead of comparing random ids, which skipped and
repeated entries, and an existing journal needs its sequence added once.
PostgreSQL type aliases (`varchar(50)`, `int`, `timestamptz`) no longer plan
spurious column changes; a foreign key stays attached across a paired
widening; schema-qualified PostgreSQL entity tables re-plan; a SQLite plan may
remove a child foreign key together with the parent key it referenced; and
PostgreSQL inserts and retention purges work again for row-level security
without an `UPDATE` policy. See the
[migration guide](../../MIGRATION.md#01560-to-01570-migrations-change-journal-and-retention).

Version 0.156.0 fixes query, filter and relation-loading behaviour. Batched
nested relations use the target's `default_sort` when the request gives no
order; one failing key group no longer fails the rest of a shared `DataLoader`
batch; relation targets need no hand-written `BatchLoadEntity`; static filter
lists and patterns are bounded (1,000 operands, 1,000 UTF-16 code units);
placeholder counting and rebasing agree; full-text search finds binary-keyed
entities; projection queries bind relation-count order values; and SQL Server
named instances resolve through SQL Server Browser. Two changes alter results
or refuse requests that were accepted: the nested order and the filter limits.
Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01550-to-01560-queries-filters-and-relation-loading).

Version 0.155.0 (placeholder) makes the runtime GraphQL request guard and
async-graphql agree: variable defaults apply to `@skip`/`@include`, every
argument is validated with the values execution resolves, argument defaults are
checked at composition, and queries and mutations over `execute_stream` refuse
with `request_not_prepared`. Runtime GraphQL refuses operations on PostgreSQL
tables with row-level security without a `DbAuthContext`. Upgrade ORM/macros
together; see the
[root migration guide](../../MIGRATION.md#01520-to-01550-runtime-graphql-guard-and-row-level-security).

ORM 0.152.0 adds a conservative batched row-security probe for recovery and maintenance callers. Upgrade macros to the matching version; no data migration is needed.

Version 0.151.0 corrects SQL Server behaviour, with two breaking changes
for hosts (the error variant and the date text): server errors keep their
number and class (`MssqlServerError`), so constraint violations and deadlocks
are classified and a failed rollback no longer hides the real error; dates
read as RFC 3339; out-of-range `DECIMAL` values are decode errors; wide
relation batches stay under the 2,100-parameter limit; `limit: 0`, bracket
`LIKE` patterns, float and date keys, the upsert batch, foreign-key and
composite-unique DDL and a bare multi-column `default_sort` render correctly;
and connections follow an Azure SQL redirect. Upgrade ORM/macros together;
see the
[root migration guide](../../MIGRATION.md#01490-to-01510-sql-server-correctness).

Version 0.149.0 emits host-only generated input member contracts for AI
capability compilation. Readable members remain available; undisclosed members
stay closed, and a required undisclosed member omits only its capability with a
host diagnostic. Upgrade ORM/macros together and use catalogue 0.8.0. Existing
SDL and public catalogue bytes/fingerprints are unchanged; no data migration is
needed. See the [migration guide](../../MIGRATION.md#01470-to-01490-host-only-generated-input-contracts).

Version 0.147.0 (placeholder) puts every generated write on one authorized
path and gives hooks and workers an explicit authority. `Entity::insert` with a
bare pool no longer bypasses policies and hooks; generated GraphQL bulk writes
are bounded by `PaginationConfig::max_bulk_write_rows` and refused with
`PAGE_LIMIT_EXCEEDED` above it; custom `#[graphql(entity)]` resolvers enforce
operation assurance; `RowPolicy::write_target_prefilter`,
`Database::with_system_access`, `MutationContext::with_system_access`,
transaction extensions, `order_by_fields` with the generated `<Entity>OrderField`
enum and `schema_roots!` `require_policies` are new and opt-in. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01440-to-01470-write-paths-hooks-and-authority).

Version 0.143.0 gives authorization one contract: every generated read path
asks the field policy about a field with or without a policy key;
`#[graphql_orm_custom_operations]` enforces the authorization it declares;
`schema_roots!` rejects a handwritten root field that collides with a generated
operation at compile time, and legacy `extra_*` roots declare their fields
with `#[graphql_orm_root_fields]`; repository field policy refusals keep their
public code; and diagnostics go through `tracing`. Upgrade ORM/macros
together; see the
[root migration guide](../../MIGRATION.md#01410-to-01430-one-authorization-contract).

Version 0.141.0 (placeholder) rejects `#[transform(write = "path")]` on
fields. If the entity compiled, that transform never ran and generated writes
stored the field's supplied values. Replace it with `WriteInputTransform` on
`Database` and remediate existing rows; upgrade runtime and macros together.
See the [migration guide](../../MIGRATION.md#01390-to-01410-write-transforms-fail-closed).

Version 0.139.0 (placeholder) adds optional
`#[graphql_entity(stable_type_id = "old_crate::models::Record")]` to preserve
the entity identity across Rust module/crate moves. Type and relation-target
spellings must also remain unchanged to preserve fingerprints. Default identities and
fingerprint algorithms are unchanged. Upgrade ORM/macros together; see the
[migration recipe](../../MIGRATION.md#01370-to-01390-optional-stable-entity-identity).

Version 0.136.0 (placeholder) keeps runtime and macros aligned. PostgreSQL
backup export and search rebuild fail when RLS would filter rows. Use a role
exempt from RLS; see the [migration guide](../../MIGRATION.md).

Version 0.133.0 refuses reserved runtime schema physical table names (catalog,
pragma, ORM and companion-package prefixes, and the exact names of SQLite
table-valued functions and SQL Server compatibility views), adds `validate_with_reserved_tables` for trusted code
that describes its own `graphql_orm_` tables, refuses NUL in string defaults,
and makes PostgreSQL runtime literal defaults independent of
`standard_conforming_strings`. See the
[root migration guide](../../MIGRATION.md#01320-to-01330-reserved-runtime-table-names).

Version 0.133.0 also makes Rust repository `limit` and `offset` count visible rows
when the row policy decides rows in Rust. `RepositoryQuery`, `tx.query::<T>()`
and `search_db` scan ordered batches until the page is complete, within
`PaginationConfig::max_scanned_rows` (default `10_000`), and fail with
`PAGE_LIMIT_EXCEEDED` instead of returning a short page. Reads without a
per-row Rust check keep one SQL query. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01320-to-01330-repository-limits-and-offsets-count-visible-rows).

Version 0.132.0 hardens PostgreSQL catalog reads: they use catalog-defined
objects regardless of the session's `search_path`. RLS validation pins the
path to `pg_catalog, pg_temp` in a transaction of its own. Schema
introspection, the dependent-delete gate, the runtime migration's checks and
the plan's own `DO` blocks name every catalog relation, function, type and
operator with its schema. A partial index counts as a closed-set predicate
only when everything it depends on belongs to `pg_catalog`. RLS validation
also validates the change journal in whichever schema the connection resolves
it to, and looks for the search-document delete trigger on the table the base
path resolves to. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01310-to-01320-postgresql-catalog-reads).

Version 0.131.0 fixes native search column collisions in repository and generated
GraphQL searches by hiding managed search-table columns from entity SQL.
Server-authored default-sort SQL passes through unchanged. Upgrade ORM/macros
together; see the
[root migration guide](../../MIGRATION.md#01300-to-01310-native-search-column-collisions).

Version 0.130.0 makes four authority defaults fail closed: generated
single-row reads apply the same row visibility as list reads, a field
`read_policy` on a `SimpleObject` entity requires `graphql_policy_object`, an
empty `ScopeEntityPolicy` scope list denies, and only a typed `AuthSubject`
authenticates a request. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01290-to-01300-fail-closed-authority-defaults).

Version 0.129.0 authorizes every field that a `where`, `orderBy`, bulk-write
filter or full-text search source references with the same field read policy
as a selected field, enforces `searchable(policy)` keys at query time, and
makes relation traversal apply the target entity's auth mode and operation
scopes. Native search refuses invalid filters, and host callback failures on
the remaining generated paths are sanitized. Upgrade ORM/macros together; see
the [root migration guide](../../MIGRATION.md#01280-to-01290-field-authority-for-filters-ordering-search-and-relation-targets).

Version 0.128.0 makes Rust repository limits exact or refused. An explicit
`limit(n)` above the database's maximum page limit, or an unlimited
`fetch_all` that matches more rows than that maximum, fails with
`PAGE_LIMIT_EXCEEDED` instead of returning a silently truncated set, so a
`limit(bound + 1)` overflow check works with any bound. Transaction reads honour
the configured maximum. GraphQL page clamping is unchanged. Upgrade ORM/macros
together; see the
[root migration guide](../../MIGRATION.md#01270-to-01280-repository-limits-are-exact-or-refused).

Version 0.128.0 also makes SQL Server connections use full-session TLS with a
validated certificate by default, refuses weaker connection-string settings
unless they are opted into in code, resets pooled sessions after each
transaction, and adds pool timeouts. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01270-to-01280-sql-server-encryption-and-pool-defaults).

Version 0.127.0 keeps a hook's deliberate public refusal (an ORM public `code`
extension, or an `OrmPublicError` returned with `?`) on repository writes,
retention purge, `Database::transaction` and generated GraphQL mutations, and
reports every other hook error to repository callers as a sanitized
`INTERNAL_ERROR`. The text of a carried refusal no longer contains the
internal transport token. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01260-to-01270-repository-callers-keep-public-hook-refusal-codes).

Version 0.126.0 contains the following changes. Upgrade ORM/macros together;
see the [root migration guide](../../MIGRATION.md#01250-to-01260).

- Published Auth `v0.26.0` is selected for the `auth-agql` feature. Hosts
  that pass Auth types must select the same tag. The Auth update itself changes
  no ORM behavior and needs no data migration ([details](../../MIGRATION.md#agql-auth-0260)).
- Row policy applies to the rows generated writes target, write and return.
  The written row must satisfy `can_write_row`, update and upsert payloads
  release the row only to callers who may read it, and GraphQL writes treat
  rows the caller can neither read nor write as missing
  ([details](../../MIGRATION.md#row-policy-on-the-rows-generated-writes-target-write-and-return)).
- PostgreSQL RLS drift validation compares policies as PostgreSQL parses them,
  so a freshly applied target validates clean, and reports undeclared,
  `FOR ALL`, restrictive and role-limited policies and altered helper functions
  as errors. `#[graphql_rls]` `tenant` and `owner` must name a stored column
  ([details](../../MIGRATION.md#rls-drift-validation-compares-postgresqls-rendering)).
- Generated MSSQL writes, including compare-and-swap, work on tables with
  enabled triggers, and MSSQL `rows_affected` reports the statement's own count
  ([details](../../MIGRATION.md#mssql-writes-on-tables-with-triggers)).
- An entity without `default_sort` orders by its primary-key columns instead of
  a literal `id`, and combined `#[graphql(...)]` and `#[serde(...)]` options are
  read in any order. `#[backup]`, `#[transform]` and `#[filterable]` reject
  unknown options
  ([details](../../MIGRATION.md#primary-key-default-ordering-and-strict-attribute-options)).
- Entities whose create or update input has no public member compile: the
  empty input and the roots that need it are omitted. No compiling entity
  changes; see
  [read-only and zero-input entities](../../docs/reference/graphql-orm/macros-and-attributes.md#read-only-and-zero-input-entities).
- `RowPolicy::read_visibility_capability` is new. Generated schema builders
  refuse an exposed Keyset root when the row policy explicitly declares its
  entity `ResidualOnly`, and the new `try_schema_builder` returns that error.
  The default, `Unknown`, keeps existing roots;
  `Database::keyset_read_policy_diagnostics` reports them. Refused GraphQL
  Keyset requests carry a distinct `reason`
  ([details](../../MIGRATION.md#keyset-read-visibility-declarations-and-diagnostics)).
- Each generated operation-catalogue argument is derived from the resolver
  signature that produces the SDL, so a generated router descriptor binds
  unchanged. Managed-version update and delete descriptors gain
  `expectedVersion`. The hidden mutation field of an entity without generated
  writes is now `gom{Entity}WritesUnavailable` and is described with an
  unsatisfiable requirement. The SDL and catalogue fingerprints change
  ([details](../../MIGRATION.md#generated-catalogue-arguments-follow-the-sdl)).
- The generated `NodeId` and `RecordVersion` are served on a
  `#[graphql(complex)]` entity with typed computed fields and no relations,
  through `compose_complex_object` and `#[graphql_complex_object]`. A
  handwritten composed resolver that would shadow either field is now a
  compile error
  ([details](../../MIGRATION.md#nodeid-on-complex-objects-with-computed-fields)).

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

Version 0.102.0 fixes append-only composite-key creates and complete-key Changed events; no data migration is needed. See the root migration guide.

Version 0.99.0 revalidates the complete dependent-delete catalogue after hooks, before each prepared write and before commit. Catalogue drift refuses atomically; upgrade ORM/macros together. No data migration is needed.

Version 0.100.0 moves CacheSync authority termination to an independent jittered timer and adds separate subscriber/event callbacks. Event denials are silent; typed host failures end with continuity loss. Macros remain version-aligned with the runtime. See the root migration guide; no data migration is needed.

Version 0.98.0 rejects unreadable SimpleObject fields without an object skip, masks internal SQLx errors, and corrects generated singular descriptions. See the root migration guide; no data migration is needed. Readable relations require readable local/target keys and condition columns, proven across crates; unresolved columns or missing target metadata fail compilation. Generated Changed events require readable primary keys. `input_only` relation getters leave the SDL, and incompatible search/order declarations are compile errors; see the complete migration inventory.

Version 0.97.0 adds opt-in, bounded GraphQL dependent deletes on SQLite and PostgreSQL with single-column keys. Declare `dependent_deletes` on each participating entity and its declared deleting parents, then supply a bounded registry. No data migration is needed; see the root migration guide.

Version 0.96.0 bounds Changed operations to one root and requires admission on hand-built schemas. An independent jittered timer rechecks subscriber authority (30-second base; configurable 1–300 seconds): definite denial ends with FORBIDDEN, host failures with SUBSCRIPTION_CONTINUITY_LOST. Per-event denials remain silent. No data migration; see the root migration guide for configuration, client handling and timing limits.

Version 0.95.1 keeps in-memory SQLite schema and data across pooled connection replacement and makes release test coordination deterministic. Memory pools remain isolated unless the URL explicitly opts into `cache=shared`. No data migration is needed.

Version 0.95.0 adds opt-in runtime NodeId, managed versions and authorized keyed live subscriptions. Rust runtime collection literals need the two new optional fields; see the root migration guide. Existing serialized schemas and derived APIs are unchanged.

Version 0.95.0 also aligns searchable runtime list arguments in the semantic catalog with SDL.

Version 0.94.0 repairs backend test gating and native MSSQL CI coverage, and fixes paginated relation reads of SQL expression projections. No data migration is needed.
Version 0.94.0 documents the whole `NodeId`, managed-version and generated-relation declaration and pins it
with tests under default and Pascal-case names. `GraphQLRelations` without `#[graphql(complex)]` now reports one
compile error naming the fix; no data migration.
Version 0.94.0 masks source-bearing callback failures without a string `code` in generated
single-row mutations, matching bulk. Authored refusals retain their message and extensions.
See the root migration guide; no data migration is needed.
Version `0.94.0` adds shared exact receiver-purpose bindings and opt-in HTTP admission. Existing signed formats and generated SDL/catalogues are unchanged. No data migration is needed.

Version 0.93.0 documents `MutationContext::defer` as the after-commit callback and pins it with tests;
there is no API or behaviour change.

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
Version 0.92.0 binds keyset cursors to their entity and caller filter on every generated path; restart in-flight keyset paging.
See [keyset cursor binding](../../docs/reference/graphql-orm/pagination-migration.md#what-a-keyset-cursor-is-bound-to).

Version 0.92.0 adds opt-in authorized static collection filters with `#[relation(filter)]`.
Filtered keyset cursors are bound to caller-visible typed filter input; restart older filtered cursors.
See [related filters](../../docs/reference/graphql-orm/entities-and-relations.md#authorized-collection-filters).
Version 0.92.0 adds validated private-column and NULL-preserving negated read predicates.

Version 0.89.0 adds full-text search to runtime collections: an IR declaration, managed storage with
readiness and a resumable backfill, write maintenance, and a ranked runtime GraphQL `search` argument.
See [runtime search](../../docs/reference/graphql-orm/runtime-search.md).

Version 0.87.0 serves every managed record version as the fixed `RecordVersion: Int!` field as
well, and records record versions in the semantic catalogue (graphql-orm-operation-catalog 0.7.0).
See the [migration notes](../../MIGRATION.md#0860-to-0870-recordversion-and-catalogue-record-versions).

Version 0.86.0 adds the opt-in global record identity `#[graphql_entity(node_id)]` (`NodeId: ID!`
plus a resolvable `@key(fields: "NodeId")`), which graphql-orm-router 0.8.0 serves through
`node(NodeId)` and `nodes(NodeIds)`. No migration is needed.

Version 0.85.0 adds an optional `Keys` list to generated `<Entity>Changed` subscriptions, bounded
by `Database::subscription_key_limit()` (default 500). No migration is needed.

Version 0.84.0 adds managed record versions (`#[graphql_orm(version = "managed")]`), advanced by
every generated write, and an optional `ExpectedVersion` on generated update and delete. Bare
`version` fields are unchanged. No migration is needed.

Version 0.82.0 includes shareable aggregate result objects in federation SDL,
allowing two aggregate-enabled subgraphs to compose. No stored-data migration or
aggregate-policy change is required. See the
[aggregate migration](../../MIGRATION.md#0810-to-0820-shareable-aggregate-result-objects).

Version 0.82.0 adds opt-in `int64` static filters with native `i64` operands,
`Signed64` wire coercion and exact typed metadata. Existing `number` filters
remain 32-bit. See the [signed64 migration](../../MIGRATION.md#0810-to-0820-exact-static-signed64-filters).

Version 0.82.0 allows generated upward Changed hooks to cross Rust crate
boundaries for public entities. Repository-only hooks remain private; current
subscriber operation authority is checked independently at every source/hop,
including after serialization. Transaction/provenance semantics are retained. See the
[cross-crate migration](../../MIGRATION.md#0810-to-0820-generated-cross-crate-upward-changed-hooks).

Version 0.81.1 adds `RuntimeReadAuthority::release_mutation_result`, which lets a host
withhold a committed mutation's record (`record: null`, no error) from a caller who may
not view it. The default keeps the previous behaviour; no migration is needed.

Version 0.81.0 adds host-supplied runtime actor stamps (`RuntimeField::auto_actor`), written
from the write grant's actor (`RuntimeWriteGrant::with_actor`). See the
[migration guide](../../MIGRATION.md#0800-to-0810-runtime-actor-stamps).

Version 0.80.0 emits canonical typed filter contracts for the actual static
filter implementations, including `notIn`, similarity, date structures and
spatial predicates. Runtime metadata likewise describes its actual operator
surface. Upgrade runtime and macros together with operation catalog 0.6.0;
see the [wire-3 migration](../../MIGRATION.md#0780-to-0800-canonical-typed-filter-contracts).
No resolver policy, GraphQL filter SDL or database format changes.

Version 0.79.0 disables incidental async-graphql defaults while retaining
loaders, UUID, Decimal and explicit runtime GraphQL. This changes optional
reexport symbols and `UploadValue.content`; see the
[feature contract](../../docs/reference/graphql-features.md) and
[migration guide](../../MIGRATION.md#0780-to-0790-explicit-graphql-features).

Version 0.78.0 lets runtime GraphQL mutation payloads select to-many and many-to-many
relation pages. See the
[migration guide](../../MIGRATION.md#0770-to-0780-to-many-relations-in-mutation-payloads).

Version 0.77.0 backs up runtime `DateTime` columns (`BackupValueKind::DateTime`, now
`#[non_exhaustive]`), restores self-reference cycles, and adds `Database::database_identity()`. See
the [migration guide](../../MIGRATION.md#0760-to-0770-date-time-backups-and-self-reference-cycles).

Version 0.76.0 delimits reserved-word and irregular identifiers in generated SQL, so a
table or column named `order`, `join` or `select` works. See the
[migration guide](../../MIGRATION.md#0750-to-0760-delimited-reserved-word-identifiers).

Version 0.75.0 adds server-managed runtime timestamps (`RuntimeField::auto_timestamp`:
`OnCreate`, `OnCreateAndUpdate`). See the
[migration guide](../../MIGRATION.md#0740-to-0750-automatic-runtime-timestamps).

Version 0.74.0 bounds generated index names to PostgreSQL's 63-byte identifier
limit and refuses PostgreSQL plans that would truncate a declared name. See the
[migration guide](../../MIGRATION.md#0730-to-0740-bounded-postgresql-identifiers).

Version 0.73.0 lets runtime creates and updates carry many-to-many link changes in the same
transaction (`RuntimeMutationRequest::with_link_change`, opt-in GraphQL `links`). See the
[migration guide](../../MIGRATION.md#0720-to-0730-atomic-link-changes-in-writes).

Version 0.71.0 backs up composite-key tables and describes the link tables of many-to-many
relations (`join_table_backup_descriptors`). See the
[migration guide](../../MIGRATION.md#0701-to-0710-composite-key-backups-and-link-tables).

Version 0.70.1 drops a replaced foreign key before adding its replacement, so a changed delete
policy applies on PostgreSQL. See the
[migration guide](../../MIGRATION.md#0700-to-0701-foreign-key-replacement-order).

Version 0.70.0 refuses hidden and missing link targets with a uniform `denied`
before any write; a missing target no longer yields `constraint_violation`. See
the [migration guide](../../MIGRATION.md#0690-to-0700-uniform-refusal-of-hidden-and-missing-link-targets).

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

Version 0.65.0 adds `#[graphql_policy_object]` to enforce current scalar field
policies on `SimpleObject` entities that use generated complex relations. Add
the attribute before the derives; existing relations and guards are preserved.
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
later authorized event starts with clean response errors. After successful
serialization, delivery rechecks current source, target, key and selected-field
authority before yielding the prepared value.

Version 0.64.0 generates complete composite-key and read-only projection change
subscriptions independently of mutation exposure. Read-only and composite
`GraphQLOperations` entities derive both serde traits for their observer events. See the
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

Version 0.57.0 adds exact SQL Server `MONEY` typed reads through
`#[graphql_orm(mssql_money)]`. The generated read projection casts the native
column to `DECIMAL(19,4)` under its original alias before decoding; ordinary
portable decimals retain their precision-18 limit. See the
[SQL Server reference](../../docs/reference/graphql-orm/mssql.md) and
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
`agql-auth` `v0.21.3`, retaining the same reviewed Auth commit. That release
required direct Auth dependencies to use the same tag and resolve one Auth
source in the lockfile. No runtime behavior, generated code or schema migration
is added. See [source alignment](../../MIGRATION.md#0501-to-0502-published-auth-source-alignment).

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

`graphql-orm` turns annotated Rust structs into typed database metadata,
`async-graphql` objects, query/filter/order inputs, and—when requested—CRUD
resolver types. It supports managed SQLite and PostgreSQL schemas plus
read-only or deliberately `ExternalWritable` SQL Server integration for
externally owned tables.

Version 0.48.1 fixes PostgreSQL TEXT literal-default replanning and owned runtime
certification while retaining physical hashes, migration history and timestamp
contracts. See the [migration guide](../../MIGRATION.md).

It is not a database server, migration runner, HTTP framework, authorization
system, or a promise that a generated field is safe to expose. Applications
own connection lifecycle, schema-application decisions, request identity,
field/row policy, and HTTP transport.

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

## Install from Git

Find the package manifest in the
[workspace package inventory](../../docs/reference/workspace-packages.md)
and the released version and source revision in the
[workspace release manifest](../../docs/operations/release/process.md#release-identities).

Packages are distributed from this repository at reviewed full revisions. Pin
the runtime and disable the default SQLite feature when selecting another
backend:

```toml
[dependencies]
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", default-features = false, features = ["sqlite"] }
```

If you depend directly on `agql-auth`, use the same Git URL and tag as the
reviewed workspace revision's [Cargo.toml](../../Cargo.toml). Refresh your
lockfile and verify that it resolves one Auth source. The workspace release
manifest records the locked Auth commit.

This unpublished package has no docs.rs release. Use this Git README and the
repository reference, or build matching local rustdoc with
`cargo doc -p graphql-orm --no-deps`.

| Feature | Enables | Important limit |
| --- | --- | --- |
| `sqlite` (default) | SQLx SQLite, Tokio runtime, migrations, GeoJSON spatial support | spatial predicates run in Rust; no SQLite spatial index is created |
| `postgres` | SQLx PostgreSQL + Rustls, migrations, native PostGIS/FTS paths | requires a PostgreSQL deployment and compatible extensions for spatial use |
| `mssql` | Tiberius SQL Server reads and deliberate external-schema DML | compatibility constructors remain physically read-only; no managed migrations, backup/restore, search maintenance, or runtime-schema row decoding |
| `change-journal` | change-journal API surface | does not itself configure a journal |
| `auth-agql` | one-way `agql-auth` bridge | does not install authentication or grant scopes |
| `receiver-admission` | exact delegated HTTP query admission; enables `auth-agql` | requires registered contracts and authoritative host adapters; [reference](../../docs/reference/receiver-admission.md) |
| `router-protocol` | project-neutral generated operation export | does not run a router |

GraphQL naming is not a feature: the `GRAPHQL_ORM_RESOLVER_CASE`,
`GRAPHQL_ORM_ARGUMENT_CASE` and `GRAPHQL_ORM_FIELD_CASE` environment variables
select it per build; see
[naming configuration](../../docs/reference/graphql-orm/macros-and-attributes.md#naming-configuration).
In a multi-backend dependency graph, select `backend = "sqlite" | "postgres" |
"mssql"` on each entity and in `schema_roots!`.

Companion capabilities remain separate packages rather than core features.
Depending on `graphql-orm` does not compile or link `graphql-orm-ai`,
`graphql-orm-storage`, `graphql-orm-backup`, or `graphql-orm-router`; add only
the companion crates an application uses. For the core crate itself, set
`default-features = false` and enable one database backend plus only the
optional bridges needed by the application.

Generated update, delete, predicate-write, and upsert helpers keep the
authoritative preimage, row/field policy, input transformation, hooks, and DML
on one pinned transaction. Predicate writes materialize the authorized primary
keys and never re-run a broad predicate after authorization. Top-level upserts
select `TransactionMode::StateMachine` automatically; an upsert composed inside
`Database::transaction` must explicitly select that mode so an absent-key
decision is fenced across every backend. See [runtime writes and repository
operations](../../docs/reference/graphql-orm/runtime-and-writes.md#repository-helpers).

## Read snapshot transactions

Use `Database::read_snapshot_transaction` for related reads that need one
consistent database snapshot. The callback receives a `ReadSnapshotContext`
with entity queries, projections and primary-key lookups. Each
uses the same ORM read policies as an ordinary transaction. If the request
uses transaction-local database authorization, call
`read_snapshot_transaction_with_auth` with its `DbAuthContext`.

```rust,ignore
let (account, accounts) = db
    .read_snapshot_transaction(|tx| {
        Box::pin(async move {
            let account = tx.find_by_key::<Account>(&[SqlValue::Int(account_id)]).await?;
            let accounts = tx.query::<Account>().count().await?;
            Ok((account, accounts))
        })
    })
    .await?;
```

- SQLite uses a deferred `BEGIN`. The primary database and every
  file-backed attachment must use WAL and normal locking. A rollback-journal
  reader takes a SHARED lock that can block a concurrent commit, so the
  runner returns `ReadSnapshotError::SqliteWalRequired` for a file outside WAL
  mode and `ReadSnapshotError::SqliteNormalLockingRequired` for exclusive
  locking. Temporary schemas and in-memory attachments are exempt. In WAL
  mode the first read of each file establishes that file's snapshot, and
  writers on other connections can commit while it is open. Shared-cache
  connections are unsupported. See [SQLite WAL](https://www.sqlite.org/wal.html).
- PostgreSQL uses `REPEATABLE READ READ ONLY` with the same transaction-local
  RLS settings as `transaction_with_auth`. It provides a consistent snapshot
  with repeatable-read isolation. A read-only observer can still see a
  result with no equivalent serial order, even when concurrent writers use
  `SERIALIZABLE`. Hosts that need a serializable read can use PostgreSQL's
  `SERIALIZABLE READ ONLY DEFERRABLE` in a separate transaction. It lets
  ordinary data writers proceed and may wait for a safe snapshot before its
  first read. Both modes take normal read locks that can conflict with DDL.
  See [PostgreSQL transaction isolation](https://www.postgresql.org/docs/18/transaction-iso.html)
  and [transaction characteristics](https://www.postgresql.org/docs/18/sql-set-transaction.html).
  The [PostgreSQL SSI paper](https://drkp.net/papers/ssi-vldb12.pdf) gives an example of the read-only anomaly.
- SQL Server uses `SNAPSHOT` after checking the current database setting.
  `ReadSnapshotError::MssqlSnapshotIsolationRequired` means
  `ALLOW_SNAPSHOT_ISOLATION` is disabled or transitioning. Snapshot reads
  can still take schema-stability locks. The shared restore lock allows
  ordinary concurrent writers and excludes restores.

The context exposes typed reads. The pinned driver connection, raw SQL,
writes, savepoints and post-commit hooks are private to the ORM. Same-task
ORM writes through a captured handle return a refusal, and the runner
returns `ReadSnapshotError::WriteRefused` even if the callback catches it.
Nested ORM transactions on that task are refused. Use the context for
every read that must share the snapshot. Driver handles and spawned tasks
have their own scope. Decisions that perform a write belong in
`Database::transaction` with `TransactionMode::StateMachine`.

## Minimum entity

```rust
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    table = "accounts",
    plural = "Accounts",
    schema_policy = "managed",
    auth = "required"
)]
pub struct Account {
    #[primary_key]
    pub id: i64,

    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(description = "Human-readable account label")]
    pub label: String,
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    auth: "required",
    entities: [Account],
}
```

This declares the types and schema builders. Connect a
`Database::<SqliteBackend>`, choose whether to validate/plan/apply schema, and
put the resulting `AppSchema` behind an `async-graphql` HTTP integration. A
complete runnable path belongs in the workspace quickstart; this package
README remains project-neutral.

## Core concepts

- **Entity vs. repository entity:** `GraphQLEntity` creates a GraphQL surface;
  `RepositoryEntity` creates typed persistence APIs with no GraphQL types.
- **Schema policy:** managed SQLite/PostgreSQL schemas may be planned and
  explicitly applied. An externally owned SQL Server table uses
  `external_read_only` by default or deliberate `external_writable` DML;
  neither mode receives generated schema mutation.
- **Generated operations:** `GraphQLOperations` emits potential root fields;
  `schema_roots!` chooses which generated mutations are exposed and creates the
  schema builders plus discovery catalog.
- **Authorization:** generated resolver authentication is `required`,
  `optional`, or `none`; row, field, repository, and database policy remain
  separate authoritative checks.
- **Operation metadata:** generated descriptors and fingerprints are discovery
  and drift evidence. They neither authorize a resolver nor disclose a field.
- **Semantic catalogue:** `graphql_orm_semantic_catalog()` is the canonical,
  versioned public API graph for descriptions, typed fields, relationships,
  capabilities, classification, export disposition, and root coordinates.
  It omits physical and policy internals and remains non-authoritative.
- **Typed aggregates:** every public persisted readable field participates in a
  closed generated aggregate-field enum. Repository code can group and compute
  multiple `COUNT`, `MIN`, `MAX`, and `SUM` expressions in the database;
  `aggregate = true` separately opts a schema into a generated bounded
  aggregate query root.
- **Conditional relations:** externally managed polymorphic references can use
  compile-time source or target discriminator conditions. Every generated
  resolver and batching path enforces the same bound-value predicate, and the
  declaration must disable physical foreign-key emission.
- **Server-defined ordering:** computed expressions and opt-in relation counts
  add direction-only fields to generated order inputs. Named expression binds
  can be supplied by an entity-owned function reading GraphQL server context;
  clients never provide SQL, identifiers, values, or aggregate functions.
  Missing primary-key columns are appended as deterministic pagination
  tie-breakers.
- **Federation entities:** `federation_key` opts an entity into a resolvable
  Apollo Federation v2 `@key`, generating a batched entity resolver that runs
  the same authorization chain as the single-row read. A joined field is
  authorized by the subgraph that owns the entity. The router's descriptor
  contract covers root fields; since router 0.16.0 it also refuses fields whose
  composed `@authenticated` or `@requiresScopes` the caller does not meet, but
  it grants nothing.
- **Checked calendar filters:** generated date predicates use half-open
  calendar ranges and recursively reject invalid spans, offsets, and ranges
  before database work. PostgreSQL uses session `CURRENT_DATE`, SQL Server uses
  server-local `GETDATE()`, and SQLite uses UTC `date('now')`.

## Errors and security boundaries

Generated queries, mutations, relations and Federation lookups use the shared
public error sanitizer. Top-level GraphQL errors expose a safe message and
`extensions.code`; existing mutation payload `error` strings contain only the
safe message. Backend SQL, schema and record diagnostics are not client fields.
Handwritten resolvers exposing low-level repository/loader failures must use
`graphql_error_from_sqlx`, `graphql_error_from_loader`, or `graphql_internal_error`
from `graphql::errors`.

Do not treat entity
descriptions, generated operation metadata, `auth = "required"`, or a tool
manifest as authorization. Install an `AuthSubject`/policy integration and
enforce row/field/database limits appropriate to the application.

## Documentation

- [Core reference index](../../docs/reference/graphql-orm/README.md)
- [Macro and attribute reference](../../docs/reference/graphql-orm/macros-and-attributes.md)
- [Backends and multi-backend workspaces](../../docs/reference/graphql-orm/backends.md)
- [Entities and relations](../../docs/reference/graphql-orm/entities-and-relations.md)
- [Typed grouped aggregates](../../docs/reference/graphql-orm/typed-aggregates.md)
- [Federation entities, keys, and operation roots](../../docs/reference/graphql-orm/federation.md)
- [Runtime predicates and keyset reads](../../docs/reference/graphql-orm/runtime-queries.md)
- [Runtime writes and repository operations](../../docs/reference/graphql-orm/runtime-and-writes.md)
- [Schema management](../../docs/reference/graphql-orm/schema-management.md)
- [Strict authorization](../../docs/reference/graphql-orm/strict-authorization.md)
- [SQL Server integration](../../docs/reference/graphql-orm/mssql.md)

SQL Server supports optional [native query notifications](../../docs/reference/graphql-orm/mssql.md#query-notifications) through an explicitly authorized, externally provisioned Broker connection.

## Authorization-aware pagination

Version 0.33 adds typed `RowPolicy::read_visibility` decisions, SQL-authorized
list/keyset pagination, and opt-in bounded callback scans on keyset-enabled
entities. Existing callback list offsets/counts remain exact. See the
[pagination guide](../../docs/reference/graphql-orm/pagination-migration.md)
for configuration, generic examples, continuation semantics, and limitations.

`RepositoryEntity` aggregate-field enums retain plain Rust `Clone`, `Copy`,
`Debug`, `Eq`, `Hash` and `PartialEq` traits and typed aggregate builders without
implementing GraphQL input/output traits. A repository consumer needs no direct
`async-graphql` dependency. The
[external consumer](tests/fixtures/repository-aggregate-consumer/src/lib.rs)
compiles scalar and composite keys on SQLite/PostgreSQL/MSSQL; its
[owned-backend tests](tests/fixtures/repository-aggregate-consumer/tests/owned_backend.rs)
execute grouped aggregates and denial before SQL on disposable SQLite/PostgreSQL.
Ordinary `GraphQLEntity` aggregate SDL remains unchanged.

Complete bounded SQLite text-group pages reuse generated aggregate enums and the
existing aggregate builder. See [typed aggregates](../../docs/reference/graphql-orm/typed-aggregates.md)
and the SQL-free private consumer example for native distinct identities, current
SQL visibility and typed continuation. Existing aggregate SDL/cursors are unchanged.

Repository-only Integer timestamps can use per-field `#[graphql_orm(timestamp = "host")]`.
See [host timestamp inputs, migration compatibility and cancellation limits](../../docs/reference/graphql-orm/repository-only-entities.md#host-managed-integer-timestamps).

Complete SQLite/PostgreSQL text-group pages and SQL-visible ordinary aggregates: see [typed aggregates](../../docs/reference/graphql-orm/typed-aggregates.md).

Owned runtime targets, explicit table ownership, read-only plans and guarded apply
are documented in [runtime migrations](../../docs/reference/graphql-orm/runtime-migrations.md).
Live capability checks reject FK actions/deferral the physical model cannot retain,
including incoming system/unowned constraints, and recheck on the apply transaction.
Static migration APIs remain unchanged.

Schema-bound runtime create/patch/delete with pinned authority, structural CAS,
atomic host journals and cancellation poisoning are documented in
[runtime mutations](../../docs/reference/graphql-orm/runtime-mutations.md).

The opt-in `runtime-graphql` feature provides [authorized dynamic reads and checked composition](../../docs/reference/graphql-orm/runtime-graphql.md), with host-owned authority and confidential cursors. It does not change static SDL or add mutations.

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

Version 0.92.0 adds a federation end-to-end test that serves generated aggregate counts from two subgraphs through the router. It changes no runtime behaviour and needs no migration; see [the migration guide](../../MIGRATION.md#0911-to-0920-aggregate-counts-from-two-subgraphs-test-only).

Version 0.91.1 only allocates a fresh package identity for fixture lockfiles that record AI 0.120.2. Generated code and runtime behaviour match 0.91.0.

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

Test-owned PostgreSQL infrastructure uses digest-pinned images.
