---
title: "graphql-orm-macros"
kind: reference
status: active
owner: graphql-orm-macros-maintainers
last_reviewed: 2026-11-25
review_by: 2027-02-01
supersedes: []
---

# `graphql-orm-macros`

Version 0.244.0 aligns with
[ORM 0.244.0](../../MIGRATION.md#02440-orm-compiler-diagnostics).

Version 0.241.0 adds `#[graphql_orm(time_offset_datetime)]` for persisted
`time::OffsetDateTime` and `Option<time::OffsetDateTime>` fields when the ORM
`time` feature is enabled. See the
[host timestamp example](../graphql-orm/README.md#host-timestamps-with-time).

Version 0.240.0 emits one set of field descriptors for generated GraphQL
upsert and create on upsert-enabled entities.
[Migration](../../MIGRATION.md#02400-shared-create-and-upsert-field-policies).

Version 0.238.0 aligns with the ORM's
[nested closure target update](../../MIGRATION.md#02370-to-02380-nested-closure-targets).

Version 0.237.0 generates each entity's write configuration, which the ORM's
write pipeline reads to admit a generated write and to
[bind its authority](../../MIGRATION.md#02360-to-02370-bound-generated-write-authority).

Version 0.235.0 generates a checked loader lookup for both to-one and
to-many relation fields. See the
[migration guide](../../MIGRATION.md#02340-to-02350-optional-relation-loaders).

Version 0.234.0 aligns with the ORM's internal date-time engine update.
See the [migration guide](../../MIGRATION.md#02330-to-02340-internal-date-time-engine).

Version 0.233.0 aligns with the ORM's
[nested closure SQL update](../../MIGRATION.md#02320-to-02330-nested-closure-sql).

Version 0.232.0 aligns with ORM 0.232.0. The full-stack test fixture builds
its selected backend. [Migration](../../MIGRATION.md#02310-to-02320-jwt-and-fixture-dependencies)

Version 0.231.0 generates GraphQL writes that call the ORM's shared
[write pipeline](../../MIGRATION.md#02300-to-02310-generated-write-pipeline)
instead of inlining their authorization sequence.

Version 0.230.0 splits query emission by operation family and delegates offset
list and aggregate execution to the ORM runtime.
See the [migration guide](../../MIGRATION.md#02290-to-02300-shared-generated-read-execution).

Version 0.228.0 aligns with the ORM's
[transient input validation redaction](../../MIGRATION.md#02270-to-02280-transient-input-validation-errors).

Version 0.227.0 validates SQL Server unsigned inputs before selecting targets
for updates and upserts, including updates that would affect zero rows.
See the [migration guide](../../MIGRATION.md#02260-to-02270-sql-server-varchar-and-bulk-update-validation).

Version 0.225.0 aligns with the ORM's
[partial-index introspection fix](../../MIGRATION.md#02240-to-02250-opaque-partial-index-predicates).

Version 0.224.0 lets a mutation hook's nested writes be authorized from the
originating request, as an opt-in, and passes an explicit `PolicyAuthority` to
entity, row and repository field policies. See the
[migration guide](../../MIGRATION.md#02230-to-02240-explicit-authority-for-hooks-and-workers).

Version 0.223.0 selects the ORM's seek-capable exact string filter renderer for
declared SQL Server BIN2 Unicode key columns (#867).
[Migration](../../MIGRATION.md#02220-to-02230-sql-server-exact-string-filter-seeks)

Version 0.222.0 adds the `upsert_bound_key`, `transient_input` and
`upsert_acknowledge` entity options.
See [principal-bound upserts](../../docs/reference/graphql-orm/runtime-and-writes.md#principal-bound-upserts).

Version 0.221.0 adds `changed_replay = true` for generated Changed streams
with `changed_started = true`. Clients can resume from an opaque cursor
within the process-local retention window.
[Changed replay reference](../../docs/reference/graphql-orm/macros-and-attributes.md#bounded-replay)

Version 0.220.0 stays aligned with the ORM's opt-in canonical PostgreSQL
journal keys. Existing macro call sites require no changes. See the
[migration guide](../../MIGRATION.md#02190-to-02200-canonical-postgresql-journal-keys).

Version 0.219.0 enforces assurance on every enabled entity query root in
generated Federation entity lookups.
See the [migration guide](../../MIGRATION.md#02180-to-02190-indirect-query-assurance).

Version 0.218.0 adds the `server_only_key(from, to)` relation option.
See [server-only partition keys](../../docs/reference/graphql-orm/entities-and-relations.md#server-only-partition-keys).

Version 0.217.0 enforces required query assurance when traversing generated
relations into the protected entity.
See the [migration guide](../../MIGRATION.md#02160-to-02170-relation-target-assurance).

Version 0.216.0 lets PostgreSQL choose the join for native search on
generated `#[graphql_rls]` entities with a column-based default sort. Both
RLS policies stay in force. See the [PostgreSQL search performance notes](../../docs/reference/graphql-orm/postgres.md#search-performance).

Version 0.215.0 lets SQL Server entities declare VARCHAR, NVARCHAR and integer
column types with the `mssql_type` field attribute.
See [SQL Server parameter types](../../docs/reference/graphql-orm/macros-and-attributes.md#sql-server-parameter-types).

Version 0.214.0 stays aligned with ORM 0.214.0 for the Auth update.
See the [migration guide](../../MIGRATION.md#02130-to-02140-agql-auth-v0270).

Version 0.213.0 aligns with the ORM's
[test harness fixes](../../MIGRATION.md#02120-to-02130-test-harness-coordination).

Version 0.212.0 aligns with the ORM's
[scoped SQLite column drop](../../MIGRATION.md#02110-to-02120-scoped-sqlite-column-drop).

Version 0.211.0 records the Rust type of described root results for
schema catalogue composition (#883).
[Migration](../../MIGRATION.md#02100-to-02110-described-root-result-metadata)

Version 0.210.0 adds two opt-in options for generated updates. One makes
`ExpectedVersion` required. The other selects assurance per request from the
typed patch and the locked row. Other entities are unchanged. See the
[migration guide](../../MIGRATION.md#02090-to-02100-required-expectedversion-and-conditional-update-assurance-operation-catalogue-0130).

Version 0.209.0 aligns with the ORM runtime's
[external migration preflight API](../../MIGRATION.md#02080-to-02090-external-migration-preflight).

Version 0.208.0 generates the adapter for opt-in
[bounded set-based deletes](../../docs/reference/graphql-orm/runtime-and-writes.md#bounded-set-based-deletes).

Version 0.207.0 aligns with the ORM's
[closure rendering update](../../MIGRATION.md#02060-to-02070-sqlite-closure-rendering-and-nested-closure-read-predicates).

Version 0.206.0 aligns with the ORM runtime's
[backend-typed backup API](../../MIGRATION.md#02050-to-02060-backend-typed-backup-runtime).

Version 0.205.0 reduces per-entity compile work for generated Changed
subscriptions by sharing their stream implementation. See the
[migration guide](../../MIGRATION.md#02040-to-02050-generated-changed-subscriptions).

Version 0.204.0 aligns with the ORM runtime's
[external backup and restore setting](../../MIGRATION.md#02030-to-02040-externally-managed-backup-and-restore).

Version 0.203.0 adds SQL Server-only options to
`backend_selected_graphql_entity` and opt-in exact string filters. See the
[migration guide](../../MIGRATION.md#02020-to-02030-durable-ai-sql-server-prerequisites).

Version 0.202.0 aligns with the ORM's
[integration test consolidation](../../docs/development/testing.md).
See the [migration guide](../../MIGRATION.md#02010-to-02020-test-and-repository-hygiene).

Version 0.201.0 makes `schema_roots!` report duplicate entity identities
through its fallible schema builders and resolver catalogue helper. Upgrade
ORM and macros together. See the
[migration guide](../../MIGRATION.md#02000-to-02010-duplicate-entity-identities-operation-catalogue-0120).

Version 0.200.0 includes the
[standalone closure rendering update](../../MIGRATION.md#01990-to-02000-closure-read-predicates).

Version 0.199.0 adds derive-level opt-outs for generated mutations and Changed
subscriptions. See [omitting generated roots](../../docs/reference/graphql-orm/macros-and-attributes.md#omitting-generated-roots).
Generated mutations, entity getters, and relations return a curated field error
when the schema lacks the database.

Version 0.198.0 supports size checks for AI restore pages.
See the [migration guide](../../MIGRATION.md#01970-to-01980-restore-page-size-checks).

Version 0.197.0 keeps SQL Server generated identifiers delimited and extends
the quoting cases shared with the runtime. The runtime's general SQL Server
identifier renderer also supports bare temporary table and variable names.
See the [migration guide](../../MIGRATION.md#01960-to-01970-database-binding-and-decoding).

Version 0.196.0 lets hosts use generated entity keys and subscription
authorization in opt-in [computed-field invalidations](../../docs/reference/graphql-orm/entities-and-relations.md#computed-field-invalidations).

Version 0.195.0 groups selected ORM integration tests. The
[testing guide](../../docs/development/testing.md) lists the grouped selectors.

In version 0.194.0, `schema_roots!` emits `graphql_orm_resolver_catalog()` beside the operation
and semantic catalogue functions, and `GraphQLRelations` and
`#[graphql_complex_object]` (with any options) record which generated
relation resolvers an object mounts. `GraphQLOperations` supplies each entity's
resolvable `@key` field sets and the roots whose static requirement its
traversals and entity resolvers enforce. Generated SDL and resolvers are
unchanged.
See the [migration guide](../../MIGRATION.md#01930-to-01940-resolver-catalogue-operation-catalogue-0110).

Version 0.193.0 adds `#[graphql_orm(object_fields)]` and
`#[graphql_object_fields]`: handwritten resolver fields on the object
`GraphQLEntity` generates, each behind the field read decision of a generated
field. Entities that do not opt in generate the same output.
See the [migration guide](../../MIGRATION.md#01920-to-01930-handwritten-fields-on-generated-objects).

Version 0.192.0 adds `propagate_change_action = "updated"` for upward relations.
See the [relation attribute reference](../../docs/reference/graphql-orm/macros-and-attributes.md#relations-and-foreign-keys).

Version 0.191.0 aligns with the ORM's
[read snapshot transactions](../graphql-orm/README.md#read-snapshot-transactions).
Existing generated reads work through the snapshot context.

In version 0.190.0, `GraphQLOperations` reports malformed keyset orders at
the keyset attribute. Use `field asc|desc [nulls first|last]` for each
comma-separated entry and end the order with the complete primary key.
See the [migration guide](../../MIGRATION.md#01890-to-01900-generated-query-resolvers).

Version 0.189.0 qualifies generated root search filters with the entity table
(#799). See the [migration guide](../../MIGRATION.md#01880-to-01890-root-search-filter-binding).

Version 0.188.0 generates a macro-only `ReadVisibilityFilter` hook and `table_name` for where
inputs. The first renders each filter column as `row.<column>` for the row it is given, the column
delimited as generated DDL and DML address it, so read predicates and related-row filters bind to
the row they describe without substituting a row name into SQL (#796). Related-row filter keys are
bound the same way. The entity derive refuses a table named like a generated alias (`__gom_self`,
`__gom_row_<n>`, `__gom_visibility_<n>`, `__gom_closure_<n>`). Relation-count orderings pass
their key columns to the runtime, which allocates the count's aliases. Other generated SQL is
unchanged.

Version 0.187.0 is the **breaking release**; see the
[migration guide](../../MIGRATION.md#01860-to-01870-breaking-release) for the steps
in upgrade order. For the macros:

- The 15 `resolver-case-*`, `argument-case-*` and `field-case-*` features are
  removed; the build script reads `GRAPHQL_ORM_RESOLVER_CASE`,
  `GRAPHQL_ORM_ARGUMENT_CASE` and `GRAPHQL_ORM_FIELD_CASE` instead. The unused
  `mysql` feature is removed.
- Every `schema_roots!` schema has a compile-time maximum page limit
  (`max_page_limit: N`, 100 when omitted), and generated resolvers that take
  an input object require `GeneratedInputValidation`, which generated schema
  builders install.
- Generated-operation fingerprints no longer include the Rust module path
  (operation catalogue 0.10.0), so every fingerprint changes once.
  `stable_type_id` is still accepted and no longer affects fingerprints.
- The generated `load_relations`/`bulk_load_relations` preloads are removed.
- Generated writes carry temporal types for chrono dates and timestamps and
  for text fields marked `#[date_field]` or `#[filterable(type = "date")]`.
- `#[graphql_entity(key_only_deletes)]` adds an opt-in key-only delete signal
  for `Key`/`Keys` subscribers.

Upgrade ORM and macros together.

Version 0.186.0 aligns with the ORM's
[owned PostgreSQL test harness fix](../../MIGRATION.md#01850-to-01860-owned-postgresql-test-readiness).

Version 0.185.0 aligns with the runtime's
[SQL Server filtered-index fix](../../MIGRATION.md#01830-to-01850-sql-server-filtered-index-literals).
Macro output is unchanged.

Version 0.182.0 passes a declared identifier through unchanged only when it is exactly one
well-formed delimited identifier, refuses a `table` or `db_column` part that only looks
delimited or contains NUL, and splits declared paths with each backend's own delimiters (#782).
Generated code is otherwise unchanged.

Version 0.181.0 pairs with the runtime's SQL Server closure read predicates. Generated where inputs
render their nested `and`, `or` and `not` lists as balanced parenthesised pairs, which keeps long
lists shallow for the SQL parser with the same meaning and bind order. Generated where-input and
ordering preparation share one per-statement relation-policy budget.

Version 0.180.0 passes the runtime a key accessor from generated `updateMany` and `deleteMany`,
which the opt-in SQL Server snapshot bulk-write target selection uses to lock its targets by key (#752).
Upgrade ORM and macros together; see the
[migration guide](../../MIGRATION.md#01791-to-01800-opt-in-sql-server-snapshot-bulk-write-targets).

Version 0.179.1 changes no macro output; it keeps the version line aligned with the ORM, whose test-owned PostgreSQL containers are now memory-capped.

Version 0.179.0 delimits a SQLite name that starts with U+FEFF in generated SQL, matching
the runtime renderer; every other name is generated as before.

Version 0.177.0 gives the root delete of a `dependent_deletes` entity the
same `errorCode` as a direct delete under `mutation_error_codes` (#763), and
generates `updateMany` and `deleteMany` against the runtime's bulk-target
selection, which can narrow targets to read visibility when the host opts in
(#752). Upgrade ORM and macros together; see the
[migration guide](../../MIGRATION.md#01760-to-01770-bulk-write-target-visibility-and-coded-dependent-delete-refusals).

Version 0.176.0 pairs with the runtime transitive-closure read predicates; generated code is unchanged.

Version 0.174.0 pairs with the runtime read-policy negation additions; generated code is unchanged.

Version 0.173.0 declares Rust 1.97.1 as the minimum supported compiler. CI verifies
the workspace and explicit backend/provider graphs with Rust 1.97.1.

Version 0.172.0 accepts the entity options `lifetime(...)`,
`changed_started` and `aggregate_source`, and the `schema_roots!` option
`client_metadata`, and emits `graphql_orm_interpretation_fingerprint` from
every `schema_roots!`. Without them generated output is unchanged. Recompile
entities with aligned ORM/macros versions. See the
[migration guide](../../MIGRATION.md#ormmacros-01720-and-operation-catalogue-081-client-facing-metadata).

Version 0.171.0 accepts `#[graphql_entity(mutation_error_codes)]`, which adds
a nullable `errorCode` to the entity's generated single-row results and a code
to its bulk filter refusals, and wraps generated mutation roots so their
errors can carry the opt-in `extensions.outcome`. Without the attribute and
the `Database` setting, generated SDL and responses are unchanged. Recompile
entities with aligned ORM/macros versions. See the
[migration guide](../../MIGRATION.md#01700-to-01710-opt-in-client-error-contract).

Version 0.170.0 installs `GeneratedInputValidation` in the generated schema
builders, so a value that is not an object where an input object is expected
is refused with `invalid_graphql_input` before any resolver runs, as is a
variable at a position the GraphQL rule does not allow. Recompile entities with
aligned ORM/macros versions. See the
[migration guide](../../MIGRATION.md#01690-to-01700-input-object-values-on-static-schemas-and-bulk-write-filter-authority).

Version 0.169.0 maps a SQL Server `String` field that is a key, unique, an
index member or a foreign-key source to `NVARCHAR(450)`, which SQL Server can
index, and an unmapped Rust type to `NVARCHAR(MAX)` instead of `TEXT`.
Recompile entities with aligned ORM/macros versions. See the
[migration guide](../../MIGRATION.md#01670-to-01690-sql-server-correctness-and-a-client-offset-bound).

Version 0.167.0 changes test code only: the macros unit tests no longer assume
the SQLite backend, so they run under each backend feature. No generated code
or behavior changes.

Version 0.166.0 adds opt-in transaction-bound dependent-effect verification.
`DependentDeleteGuard` compares declarations with the introspected delete plan
and enforces dependent row limits before writes. Generated physical opt-out and
bulk deletes refuse guarded requests. Unchanged hosts keep existing behavior.
See the [migration guide](../../MIGRATION.md#ormmacros-01660-guarded-dependent-deletes).

Version 0.165.0 preserves the stored value of an optional field when its
write policy denies an upsert update and the host opts to ignore that denial.
Recompile entities with aligned ORM/macros versions. See the
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

Version 0.162.0 is released with `graphql-orm` 0.162.0, which adds
context-bound authorized pages for handwritten resolvers. The macros are
unchanged. See the
[migration guide](../../MIGRATION.md#01620-context-bound-authorized-pages).

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

Version 0.152.0 stays aligned with ORM 0.152.0 and its batched row-security probe. Generated operations are unchanged; no data migration is needed.

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

Version 0.133.0 stays aligned with ORM 0.133.0, which refuses reserved runtime
schema table names and quotes PostgreSQL runtime literal defaults with escape
strings. Generated code is unchanged. Upgrade ORM/macros together; see the
[root migration guide](../../MIGRATION.md#01320-to-01330-reserved-runtime-table-names).

Version 0.133.0 also stays aligned with ORM 0.133.0, which makes Rust repository
`limit` and `offset` count visible rows when the row policy decides rows in
Rust. Generated code is unchanged. Upgrade ORM/macros together; see the
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

Version 0.128.0 stays aligned with ORM 0.128.0, which makes Rust repository
limits exact or refused, so repository-only `find_all` and `find_many` fail
with `PAGE_LIMIT_EXCEEDED` instead of returning a truncated set. Generated code
is unchanged. Upgrade ORM/macros together; see the
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

- Macros stay aligned with ORM 0.126.0, which selects published Auth
  `v0.26.0`. Generated code is unchanged by the Auth update
  ([details](../../MIGRATION.md#agql-auth-0260)).
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

Derived keyset order inputs include sortable portable Decimal fields. See
[Decimal keyset ordering](../../docs/reference/graphql-orm/pagination-migration.md#decimal-keyset-ordering).

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

For that release, the workspace selected published Auth `v0.22.0` at
`997707e060bcf5fabf879a5f98146dc2732b1c99` and required direct Auth dependencies
to use the same tag and resolve one Auth source in the lockfile. The
[0.53 migration](../../MIGRATION.md#0520-to-0530-post-await-and-owned-release-checks)
describes validated current-session proofs.

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

## Install from Git

This is the procedural-macro implementation for `graphql-orm`. Applications
normally depend on the runtime crate and import its prelude; that keeps the
macro/runtime versions aligned.

Find the package manifest in the
[workspace package inventory](../../docs/reference/workspace-packages.md)
and the released version and source revision in the
[workspace release manifest](../../docs/operations/release/process.md#release-identities).

If you depend directly on `agql-auth`, use the same Git URL and tag as the
reviewed workspace revision's [Cargo.toml](../../Cargo.toml). Refresh your
lockfile and verify that it resolves one Auth source. The workspace release
manifest records the locked Auth commit.

```toml
[dependencies]
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", default-features = false, features = ["sqlite"] }
```

Direct use is supported for tooling that needs the macro package:

```toml
graphql-orm-macros = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", default-features = false, features = ["sqlite"] }
```

The direct dependency still requires a compatible `graphql-orm` runtime in the
consumer crate because generated code refers to `::graphql_orm`. The macros do
not connect to a database, run migrations, host GraphQL, or authorize requests.

## Public macros

| Item | Purpose |
| --- | --- |
| `GraphQLEntity` | GraphQL object/filter/order/input types, row decoding, metadata, and query helpers |
| `GraphQLSchemaEntity` | schema metadata only |
| `RepositoryEntity` | typed repository CRUD and private projections with no GraphQL surface |
| `GraphQLRelations` | batched single/composite-key relation resolvers |
| `graphql_complex_object` | handwritten complex fields composed with generated relations |
| `graphql_object_fields` | handwritten resolver fields added to the object `GraphQLEntity` generates, with `#[graphql_orm(object_fields)]` |
| `GraphQLOperations` | generated GraphQL root operation types and operation metadata |
| `schema_roots!` | query/mutation/subscription roots, schema builders, metadata, and resolved catalog |
| `graphql_orm_custom_operations` | semantic metadata beside a handwritten root impl, plus its enforced authorization and assurance guards |
| `graphql_orm_root_fields` | root field names of a handwritten root in a legacy `extra_*` list, for the `schema_roots!` collision check |
| `GraphQLSemanticObject` | classified public field metadata for a handwritten result object |
| `mutation_result!` | a simple GraphQL mutation result object |
| `backend_selected_graphql_entity` | emits cfg-selected entity definitions for a multi-backend consumer |

For a handwritten root annotated by `graphql_orm_custom_operations`, prefer
the matching `described_query_types`, `described_mutation_types`, or
`described_subscription_types` list in `schema_roots!`. One entry composes the
root and automatically includes its operation descriptors and direct
`GraphQLSemanticObject` result metadata. The older `extra_*` plus
`semantic_custom_operations`/`semantic_types` lists remain compatible for
incremental migration, but a described root cannot also appear in them. A root
in an `extra_*` list or `query_custom_ops` carries `#[graphql_orm_root_fields]`
so `schema_roots!` can reject a field that collides with a generated operation.

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
than one.

Generated GraphQL naming is selected per build by the
`GRAPHQL_ORM_RESOLVER_CASE`, `GRAPHQL_ORM_ARGUMENT_CASE` and
`GRAPHQL_ORM_FIELD_CASE` environment variables, which replaced the
`*-case-*` features. See
[naming configuration](../../docs/reference/graphql-orm/macros-and-attributes.md#naming-configuration).

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

`GraphQLOperations` uses the `plural` override in bulk mutation result
type names such as `DeleteNotesResult`. If the override prevents
forming a Rust identifier, the derive points to the string literal.

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

Collection relations can opt into
[generated nested create](../../docs/reference/graphql-orm/runtime-and-writes.md#creating-a-parent-with-child-rows)
with `nested_create` and a matching child `nested_create_keys` declaration.

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

Test-owned PostgreSQL infrastructure uses digest-pinned images.

See the [workspace testing guide](../../docs/development/testing.md) for the ORM
integration test harnesses and backend selectors.
