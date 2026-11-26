---
title: "Changelog"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-25
review_by: 2027-02-01
supersedes: []
---

# Changelog

This file is the authoritative user-facing release chronology. The former
[release-notes ledger](docs/archive/2026/graphql-orm-release-notes.md) is retained
for historical context.

## Unreleased - ORM/macros 0.228.0

- **Changed (#724).** Generated GraphQL create, update, upsert, delete, bulk
  and nested-create mutations call a shared runtime write pipeline for request
  admission, entity admission, target admission and hook dispatch. The checks
  and their order are unchanged, as are the SDL and catalogues; generated code
  is smaller. A mutation hook receives the originating request's authority
  only with an authority that pipeline's admission issued. The hidden
  macro-internal `MutationContext::__run_generated_mutation_hook` is removed,
  which is a source break for host code that called it.
  [Migration](MIGRATION.md#02270-to-02280-generated-write-pipeline)

## Unreleased - ORM/macros 0.227.0

- **Fixed (#860).** Removed the 0.215.0 ASCII VARCHAR conversion because it
  could match different rows under legacy `SQL_` collations. String filters
  bind as NVARCHAR. Windows CP1252 and UTF-8 VARCHAR indexes sought at 2
  logical reads in the fixture. With conversion proof they used 10 reads.
  The `SQL_` fixture index scanned at 31 reads with NVARCHAR, compared with
  the 2-read VARCHAR seek in 0.215.0. **Action needed** for hosts that relied
  on the `SQL_` seek. Move the column to a Windows collation or index a
  persisted computed Unicode column for the affected comparison.
  [Migration](MIGRATION.md#02260-to-02270-sql-server-varchar-and-bulk-update-validation)
- **Fixed (#860).** Generated SQL Server updates and upserts validate unsigned
  inputs before selecting targets, including updates that would affect zero
  rows. Opt-in native integer widths and BIGINT fallback remain.

## Unreleased - ORM/macros 0.226.0

- **Added (#353).** Generated creates can opt into typed child lists on
  collection relations. The ORM binds the relation keys, authorizes each row,
  and commits the parent and up to 50 children in one transaction. When the
  call includes children, a field-write denial on any row refuses the whole
  create, even when the field policy allows legacy omission on a standalone
  create.
  [Migration](MIGRATION.md#02250-to-02260-generated-nested-create)

## Unreleased - ORM/macros 0.225.0

- **Fixed (#880).** SQLite and PostgreSQL retain opaque partial-index filters
  during introspection. SQL Server distinguishes a filtered index when the
  catalog indicates a filter and the definition is unavailable. SQLite and
  PostgreSQL emit a failing statement if asked to render an opaque predicate.
  PostgreSQL preserves declared single-value filters through introspection.
  [Migration](MIGRATION.md#02240-to-02250-opaque-partial-index-predicates)

## Unreleased - ORM/macros 0.224.0 and AI 0.165.0

- **Added (#291).** A mutation hook's nested writes can be authorized from
  the request that caused them. With
  `DatabaseBuilder::propagate_request_authority_to_hooks(true)`, a hook that a
  generated GraphQL mutation invokes presents a `HookRequestAuthority` (the
  request's subject, and its accepted agql-auth identity with `auth-agql`) on
  the entity and row policy checks of the operations it makes through its
  `MutationContext`. It is off by default and grants nothing by itself.
  `ScopeEntityPolicy` then holds the originating subject to its scope
  requirement, so a cleanup hook no longer fails `UNAUTHENTICATED` for a
  caller that holds the scope. The authority belongs to the transaction: no
  `Database` handle carries it, so a repository call, another transaction, a
  spawned task, a post-commit action or a later request never presents it.
  It is attached only by a generated GraphQL mutation whose request is
  admitted to write the mutation's entity, never by
  `MutationContext::run_mutation_hook`, and every read of it checks that the
  hook invocation is still running. Repository field decisions do not
  receive it.
- **Added (#291).** `EntityPolicy`, `RowPolicy` and `FieldPolicy` have
  `*_with_authority` methods that receive a `PolicyAuthority`: the named
  `SystemAccess` of the handle or scope, and, for entity and row decisions, a
  hook's originating request. The ORM always calls them. Their defaults call
  the existing methods with the same arguments, so existing policies are
  unchanged. A policy can now refuse a call that presents no authority
  (`ctx` is `None` and `PolicyAuthority::is_none()`) while admitting a named
  worker or a hook. `Database::policy_authority()` returns a handle's
  authority.
- **Added (#291).** `AgqlCurrentAuthorityPolicy` admits a hook's nested
  entity decision on the originating request's accepted identity, with the
  same status and authority revalidation as a request, when the host's
  `AgqlBindingAuthorizer::authorize_hook_operation` allows it. The default
  denies, so `ctx = None` calls stay refused until the host implements it.
- **Fixed (#291), AI 0.164.0.** The AI crate's internal handle forwards
  host-owned repository field decisions to the host policy's
  `*_with_authority` methods; see the
  [AI migration guide](crates/graphql-orm-ai/MIGRATION.md#01640-to-01650-authority-aware-host-field-decisions).
  **No action.** See the
  [migration guide](MIGRATION.md#02230-to-02240-explicit-authority-for-hooks-and-workers).

## Unreleased - ORM/macros 0.223.0

- **Fixed (#867).** Generated exact SQL Server equality and membership filters
  include a seek predicate for Unicode key columns declared with BIN2 collation.
  SQL text changes for all opted-in exact comparisons, including non-BIN2
  columns, `ne` and `not_in`, and the AI crate's opted-in entities. Review
  SQL Server plan guides and Query Store forced plans that match by statement
  text. The exactness check always uses Unicode. On a live `VARCHAR` column
  declared as Unicode, exact filters and read policies now accept true matches
  that the old byte-length check rejected and reject padded false matches.
  [Migration](MIGRATION.md#02220-to-02230-sql-server-exact-string-filter-seeks)

## Unreleased - ORM/macros 0.222.0

- **Added (#908).** A generated GraphQL upsert can take a private conflict key
  from the verified request, carry a typed transient input, and let its hook
  acknowledge a retry without writing. All three are opt-in entity options.
  - `upsert_bound_key(field = "...", binding = "path::Type")`: the resolver
    fills that conflict-key member from `PrincipalKeyBinding::bind` before the
    conflict lookup, and again after the input transform. The column must be
    `#[graphql_orm(private)]` with no other GraphQL surface, so no client input
    can set or replace it. A failed binding refuses the mutation before any
    policy, read or write, with a fixed `FORBIDDEN` that carries nothing from
    the binding's own error. A row the database matched but whose stored key
    is not exactly the bound value is refused with `FORBIDDEN`, so a
    case-insensitive collation cannot merge two principals. Every ownership
    refusal is one identical response. Generated updates
    never assign the column, whatever their input transform does. Every
    existing write authorization still applies. Unsafe declarations are
    compile errors, including an entity that also generates `create` or takes
    part in a nested create, and a key column shared with another field or an
    implicit timestamp or version writer.
  - `transient_input(type = "...", on = "upsert")`: a typed argument that
    `WriteInputContext::transient_input` and `MutationContext::transient_input`
    expose to the input transform and both hook phases, inside the write
    transaction. It is never stored or published in `Changed` events or the
    change journal. The argument is marked `secret`, so async-graphql's
    rendered execution document redacts it, and `@transientInput`, so AI tool
    profiles and typed AI capabilities cannot carry it. A request value over
    `max_bytes` (default 64 KiB) or `max_nodes` (default 1024) is refused
    before coercion, a value that does not coerce is refused without being
    quoted, and a defaulted variable in the argument is refused. Transaction
    code can set one with `MutationContext::set_transient_input`.
  - `upsert_acknowledge = true`: the `Before` hook may call
    `MutationContext::acknowledge_without_write` on the update branch. The
    upsert then writes nothing, publishes nothing and returns the existing row
    with `action: null`. A call made where it is not allowed also blocks the
    commit, so a hook that ignores the refusal cannot let the write through.

  [Migration](MIGRATION.md#02210-to-02220-principal-bound-upserts)

## Unreleased - ORM/macros 0.221.0 and operation catalogue 0.16.0

- **Added (#668).** Generated Changed subscriptions can opt in to an opaque
  cursor and bounded, process-local replay. Replay checks current subscriber
  authority and signals when a cursor requires a refetch.
  [Migration](MIGRATION.md#02200-to-02210-generated-changed-replay)

## Unreleased - ORM/macros 0.220.0

- PostgreSQL hosts can opt into canonical change-journal keys (#592). The
  recorder and full snapshot use the same encoding for single-column keys.
  The cutover retires legacy cursors and requires a new full backup.
  [Migration](MIGRATION.md#02190-to-02200-canonical-postgresql-journal-keys)

## Unreleased - ORM/macros 0.219.0

- **Security fix (#902).** Runtime node lookups and relation layers enforce the
  target collection's query assurance before database access. Generated
  Federation lookups enforce assurance on every enabled query root of the
  entity. [Migration](MIGRATION.md#02180-to-02190-indirect-query-assurance)

## Unreleased - ORM/macros 0.218.0

- **Added (#898).** A readable composite relation can declare
  `server_only_key(from = "...", to = "...")` for a key pair whose target
  column is server-only (`#[graphql_orm(private)]` with no other GraphQL
  surface). Generated traversal still joins on the complete tuple, so a target
  in another partition resolves to `null` or an empty list. The column stays
  out of SDL, inputs, `Changed`, catalogues and client metadata. Combining it
  with `filter`, `order_aggregate`, `search_relation` or upward propagation is
  refused at compile time. So are floating-point and boolean key members,
  targets with non-ASCII, over-long or `__gom_`-prefixed names, and a source
  member that is skipped, read-transformed or decoded (JSON, date, boolean
  field, decimal, spatial). A target column is refused when any field mapping
  exposes it, however its name is spelled, or when it belongs to the primary
  key. Batched loads of such a relation associate rows with their parent by
  the database's match of the complete tuple, so they agree with the direct
  query for every type and collation.
  [Server-only partition keys](docs/reference/graphql-orm/entities-and-relations.md#server-only-partition-keys)

## Unreleased - ORM/macros 0.217.0

- **Security fix (#899).** Generated relations enforce every required assurance
  policy on the target's generated query roots before reading target rows or
  evaluating existence or counts.
  [Migration](MIGRATION.md#02160-to-02170-relation-target-assurance)

## Unreleased - operation catalogue 0.15.0

- **Fixed (#544).** Authorization-metadata fix. The resolver catalogue
  described a generated relationship with a declared field policy as needing
  only the target root's requirement (`same_as_root`), which is looser than
  the runtime enforces. The runtime always enforced the field policy. Such
  relationships now export `subgraph_only`. The resolver catalogue fingerprint
  changes for schemas with such relationships; the operation and semantic
  catalogue fingerprints are unchanged. **No action.**

## Unreleased - ORM/macros 0.216.0

- **Improved (refs #555).** PostgreSQL native search on generated RLS entities can use
  a document primary-key lookup or a bulk join, depending on planner
  estimates. Both RLS policies stay in force. [Migration](MIGRATION.md#02150-to-02160-postgresql-rls-search)

## Unreleased - ORM/macros 0.215.0

- **Added (#522).** SQL Server pools can opt in to binding the SQL types
  declared on generated fields. Eligible ASCII VARCHAR comparisons convert
  the parameter. Policies, patterns and non-ASCII text retain Unicode binding.
  INT, SMALLINT and TINYINT can use their native widths. Generated unsigned
  values outside the declared SQL Server range return `INVALID_INPUT`.
- **Fixed (#849).** SQL Server character column ALTER statements use the parsed
  column name when looking up collation, including literal brackets.

## Unreleased - ORM/macros 0.214.0

- **Changed (#857).** The `auth-agql` adapter uses published `agql-auth`
  `v0.27.0`, locked to `71a3acee5942bd5b6aac57dcaccbb6391b383880`.
  Upgrade direct Auth dependencies with the ORM.
  [Migration](MIGRATION.md#02130-to-02140-agql-auth-v0270)

## Unreleased - ORM/macros 0.213.0

- **Fixed (#887, #888).** Compile UI tests coordinate on Cargo's resolved
  target directory. Consumer fixture builds initialize their own target
  directories before launching Cargo.
  [Migration](MIGRATION.md#02120-to-02130-test-harness-coordination)

## Unreleased - ORM/macros 0.212.0

- **Fixed (#882).** An admitted owned SQLite migration with a single column
  drop can preserve unrelated indexes and incoming foreign keys using native
  `DROP COLUMN`. Preflight identifies dependencies before live DDL, and the
  drop and history updates commit together.
  [Migration](MIGRATION.md#02110-to-02120-scoped-sqlite-column-drop)

## Unreleased - ORM/macros 0.211.0, operation catalogue 0.14.0

- **Fixed (#883).** Described roots returning a registered generated entity
  use its schema-bound semantic metadata, including generated list
  relationship page limits. When distinct Rust types claim the same GraphQL
  name, catalogue composition reports both sources.
  [Migration](MIGRATION.md#02100-to-02110-described-root-result-metadata)

## Unreleased - ORM/macros 0.210.0, operation catalogue 0.13.0

- **Added (#851).** `#[graphql_entity(update_expected_version = "required")]`
  makes the generated update's `ExpectedVersion` non-null (`Int!`) in the SDL
  and the operation catalogue. An omitted or null value is refused by
  validation, a stale one with `CONFLICT`, and a matching one advances the
  managed version once. Delete keeps its optional argument.
- **Added (#851).** `#[graphql_entity(update_assurance(select = "path", levels = "a,b"))]`
  selects the generated update's assurance policy per request from the typed
  patch and the locked row. The selection runs inside the write transaction,
  after the row is locked and before any write. A refusal rolls back the
  transaction with its related writes and commits no `Changed` event.
  `UpdateAssuranceInput` and `enforce_selected_assurance` are new. Enforcement
  fails closed with `AUTHORIZATION_MISCONFIGURED` for an undeclared level, or
  for a schema without `AssuranceEnforcement`, which is checked before the
  selector runs. A selector error or panic reaches the client only as a fixed
  `INTERNAL_ERROR`. A row whose managed version cannot advance is refused
  before the selector runs. The chosen level is observable to the caller, as the
  reference documents.
- **Added (operation catalogue, #851).** `GraphqlConditionalAssurance` and
  `conditional_assurance()` on generated and resolved descriptors list the
  levels an operation may require and mark the selection as conditional. The
  declaration is part of the operation and router-export fingerprints. Both
  options require an entity that generates neither `update_many` nor `upsert`
  and declares no `set_null` relation. `RuntimeSchema::from_static_entities*`
  refuses such entities, whichever typed constructor built their metadata.
  Entities that opt into neither keep byte-identical SDL, catalogues,
  fingerprints and debug output.

## Unreleased - ORM/macros 0.209.0

- `SchemaManager::preflight_unique_indexes` exposes the read-only duplicate-key
  check to hosts using external migration tools, including SQL Server (#878).
  [Migration](MIGRATION.md#02080-to-02090-external-migration-preflight)

## Unreleased - ORM/macros 0.208.0

- **Added (#852).** Typed transactions can delete a bounded key set or scalar
  filter match with one DELETE for opted-in mutable entities. The operation
  checks row write authorization and exact cardinality, runs per-row delete
  hooks and queues the generated change notifications.
  [Migration](MIGRATION.md#02070-to-02080-bounded-set-based-deletes)

## Unreleased - ORM/macros 0.207.0

- **Security fix (#876).** From ORM/macros 0.200.0 through 0.206.0, a SQLite
  closure read predicate (`related_closure_within` and its shorthands) could
  reach a different verdict than the original rendering when its pointer
  column's collation treats values as equal that the key column distinguishes,
  for example a `NOCASE` or `RTRIM` pointer joined to a `BINARY` key. The
  compact walk deduplicated such pointers (`'a'` and `'A'`, or `'a'` and
  `'a '`) at the same depth and followed only one of them, so an `Any`
  closure could deny a row that the original rendering admits. A randomized
  search found no over-admission, but it is not proven impossible, because
  SQLite does not guarantee which equal pointer survives. SQLite now renders
  every closure, single-level and nested, in the original form, byte for
  byte as before 0.200.0. PostgreSQL and SQL Server are not affected.
- **No action.** On PostgreSQL and SQL Server, directly nested closure read
  predicates over built-in column tests can reuse the inner closure's scalar
  result, reducing SQL and bind duplication. The admitted rows are unchanged
  on all three backends (#785).

## Unreleased - ORM/macros 0.206.0 and backup 0.30.0

- **Fixed (#870).** SQLite and PostgreSQL database handles expose the backup
  runtime in multi-backend builds. The ready-made backup adapter and object
  index use the handle selected by their own feature.
  [Migration](MIGRATION.md#02050-to-02060-backend-typed-backup-runtime)

## Unreleased - ORM/macros 0.205.0

- **Changed (part of #677, #358).** Generated Changed subscriptions share
  the runtime's stream and authorization code, reducing the work rustc does
  for each entity with subscriptions enabled.
  **No action.** See the
  [migration guide](MIGRATION.md#02040-to-02050-generated-changed-subscriptions).
- **Added.** Changed subscriptions support combining `changed_started = true`
  with `key_only_deletes`. An authorized exact-key subscriber can request a
  started frame and receive key-only `DELETED` frames with `started: false`.

## Unreleased - ORM/macros 0.204.0 and backup 0.29.0

- **Added (follow-up to #731).** Opt in per database when another product
  owns backup and restore. The ORM and its backup adapter refuse backup and
  restore before I/O. SQL Server queries and writes on opted-in handles
  run without the restore marker or fencing lock.
  [Migration](MIGRATION.md#02030-to-02040-externally-managed-backup-and-restore)

## Unreleased - ORM/macros 0.203.0

- **Action needed.** Before applying a regenerated SQL Server plan, review
  cleanup jobs and foreign-key cascades that touch `append_only = true`
  tables. The plan now installs triggers that reject updates and deletes,
  including cascaded deletes. There is no SQL Server trigger opt-out while
  the entity remains append-only (#813).
- **Action needed.** Replan SQL Server schemas and resolve any table
  declarations that map to the same physical identifier. SQL Server
  planning now refuses these collisions (#813).
- **No action.** SQL Server supports bounded row-size preflights and filtered
  unique indexes over non-null keys.
- **Action needed, only when opting in.** Entities opting in to
  `exact_string_filters = true` get length-sensitive string comparisons, and
  SQL Server plans give their generated Unicode string keys (primary keys,
  unique columns, relation foreign-key sources and index members)
  `COLLATE Latin1_General_100_BIN2`. Opt in both sides of a string foreign key
  together, because SQL Server requires matching collations. On an existing
  table, the generated `ALTER COLUMN … COLLATE` fails on indexed or constrained
  columns: drop and recreate the dependent indexes and constraints around it.
  Removing the option later leaves the live BIN2 collation in place (#813).

## Unreleased - ORM/macros 0.202.0

- Grouped additional SQLite GraphQL, owned database and consumer fixture tests.
  See the [test lanes](docs/development/testing.md) for the selectors.
- Documented the blocking filesystem operations in `write_migration_file`.

## Unreleased - ORM/macros 0.201.0, operation catalogue 0.12.0

- **Fixed (#829).** Repeated entity identities return a typed composition
  error from `GraphqlOperationCatalog::try_compose`. The generated resolver
  catalogue and fallible schema builders return that error through their
  existing error types. The error records the repeated identity and both Rust
  types.

## Unreleased - ORM/macros 0.200.0

- **Changed (#778, part of #785).** Standalone closure policies can use fewer
  target SQL copies and binds. Composed closures retain their existing SQL
  and budget. The closure reference documents incomplete descendant walks
  and non-unique-key fan-out.
  [Migration](MIGRATION.md#01990-to-02000-closure-read-predicates)

## Unreleased - ORM/macros 0.199.0

- **Added (#677).** `GraphQLOperations` accepts `mutations = false` and
  `subscriptions = false` on `graphql_entity`. Omitted roots do not appear
  in the schema or catalogues. Repository and transaction writes remain available.
- **No action (#677).** Default schemas keep the existing `MutationRoot` and
  `SubscriptionRoot` tuple structs. The generated builder selects an empty
  GraphQL root only when opt-outs leave that root with no fields. See the
  [migration guide](MIGRATION.md#01980-to-01990-generated-mutation-and-subscription-roots).
- **Fixed (#542).** Generated writes, entity scalar getters, and relation
  resolvers return a GraphQL field error when a hand-built schema is missing its
  database.
  Generated writes and Changed subscriptions qualify their Rust paths.
  Mutation-shape conflicts point to the shape declaration, and implicit
  update timestamps use the backend's identifier delimiters. Dependent
  deletes use boundaries declared by the direct mutation emitters.
- **Refactored (#723).** The create, upsert, update and delete families emit
  their GraphQL resolvers from separate modules.

## Unreleased - ORM/macros 0.198.0

- Check payload sizes for bounded SQLite and PostgreSQL restore pages before
  loading rows. Preflight residual scans before decoding any candidates and
  measure PostgreSQL spatial fields in their projected form (#532). [Migration](MIGRATION.md#01970-to-01980-restore-page-size-checks)

## Unreleased - ORM/macros 0.197.0

- **Fixed (#541):** SQL Server's compatibility decoders select conversions
  from the wire type, preserve the source of decode failures, and return
  an error for out-of-range column indexes. Missing-policy decisions share
  one mode table. The documentation describes the typed NULL, legacy JSON
  and SQL Server identifier contracts.
- **Fixed (#821):** runtime search qualifies collection columns and chooses
  distinct helper names, so colliding physical column names work in search
  projections, filters, ordering and cursor pagination.

## Unreleased - ORM/macros 0.196.0

- **Added (#842).** Trusted transaction writers can enqueue a typed computed-field
  invalidation. An opted-in subscription releases the target ID and public topic
  under a disclosure policy and current target authorization. Related-state
  changes leave the target row, revision and ordinary Changed delivery intact.
  [Subscription reference](docs/reference/graphql-orm/entities-and-relations.md#computed-field-invalidations)

## Unreleased - ORM/macros 0.195.0

- Consolidated selected PostgreSQL, SQL Server and SQLite integration test binaries.
  See the [test lanes](docs/development/testing.md) for the grouped selectors.

## Unreleased - ORM/macros 0.194.0, operation catalogue 0.11.0

- **Added (operation catalogue, #544).** `GraphqlResolverCatalog` lists what has no root coordinate and so appears in no operation catalogue: every generated relationship field (owning type, field, target, cardinality, argument names, field-policy flag) and every entity with a resolvable Federation key (its `@key` field sets, including `NodeId`). It is a separate document, wire version 1, with its own fingerprint (`graphql-orm-resolver-catalog-rfc8785-sha256-v1`), bound to the semantic catalogue by fingerprint. `schema_roots!` emits `graphql_orm_resolver_catalog()`, which returns a `Result` and reads the served schema: a declared relation with no mounted generated resolver, and a key the schema does not resolve, are left out; handwritten relationship fields and entity resolvers are out of scope for this version, and a type is listed under its registered GraphQL name with its semantic catalogue name beside it when they differ. An authorization requirement is given only as a reference to an exposed generated root, never as scope names. Existing SDL, descriptors, wire formats and fingerprints are unchanged.
- **Added (operation catalogue, #544).** `rfc8785_canonical_json` and `rfc8785_json_hash` encode JSON as RFC 8785 specifies and refuse an integer that is not exactly a double, including integers of any length under `serde_json/arbitrary_precision`. They are opt-in: `canonical_json_hash`, the semantic catalogue and the client metadata keep their encoding and bytes.
- **Documented (#544).** The catalogue README states which encoding each fingerprint uses and where it differs from RFC 8785, that `exact_document_hash` hashes exact bytes without normalisation, and what each export discloses, including that a router descriptor carries scope names and scope templates.

## Unreleased - ORM/macros 0.193.0

- **Added:** a `GraphQLEntity`-generated object can host handwritten resolver
  fields. Mark the entity `#[graphql_orm(object_fields)]` and put
  `#[graphql_orm::graphql_object_fields]` on one inherent impl of it. The
  generated column fields, descriptions, `RecordVersion` and `NodeId` are
  unchanged; the handwritten fields follow them in the same object. Each
  handwritten field runs the checks of a generated column field before its
  body: the field read decision for its row, with the key it may declare with
  `#[graphql_orm(read_policy = "…")]`. Generated read paths authorize the row
  before the object exists; an object a host resolver builds itself is the
  host's to authorize. A refused `Option<T>` field is an explicit `null` with
  a path-specific error. The field is refused in generated `Changed`
  subscriptions unless it declares `#[graphql_orm(subscribe)]`, which needs a
  scalar or enum output, or an `Option` or list of those. The fields are listed by
  `Entity::computed_object_fields()` and in the semantic catalogue as
  selectable, never-exported fields with no filter, ordering, grouping or
  aggregate. Entities that do not opt in generate exactly what they did
  ([#808](https://github.com/Dastari/graphql-orm/issues/808)). See the
  [migration guide](MIGRATION.md#01920-to-01930-handwritten-fields-on-generated-objects).

## Unreleased - ORM/macros 0.192.0

- **Added (#812).** Set `propagate_change_action = "updated"` on an upward relation
  to report child creates and deletes as `UPDATED` in the parent's `Changed`
  subscription and action filter.
  [Migration](MIGRATION.md#01910-to-01920-propagated-parent-actions)

## Unreleased - ORM/macros 0.191.0

- **Added (#831):** `Database::read_snapshot_transaction` runs multiple
  policy-aware reads against one pinned snapshot, with a companion runner for
  transaction-local authorization. SQLite requires WAL and normal locking
  for every database file. PostgreSQL uses read-only repeatable read, and
  SQL Server requires snapshot isolation.
  Backend prerequisites and refused writes have typed errors.
  See the [migration guide](MIGRATION.md#01900-to-01910-read-snapshot-transactions).

## Unreleased - ORM/macros 0.190.0

- **Fixed (generated queries, #542 and #723).** Query resolver emission is shared
  across writable and read-only entity profiles. Standard library names in the
  generated queries are qualified. Generated list, point, keyset, scan, search,
  aggregate and Federation queries and Changed subscriptions return a curated
  field error when required schema data is missing. The NodeId object field
  returns a field error instead of panicking when the schema lacks its database.
- **Fixed (keyset orders, #542).** Incomplete null ordering and trailing tokens
  produce a diagnostic on the keyset attribute.
  [Migration](MIGRATION.md#01890-to-01900-generated-query-resolvers)

## Unreleased - ORM/macros 0.189.0

- **Fixed (security, #799):** generated root search filters qualify each
  column with the entity table. A same-named column from the native search
  join can no longer change which rows match or make the query ambiguous.
  Native search also qualifies entity projections and chooses helper key and
  score names that are distinct from the entity's columns.

## Unreleased - ORM/macros 0.188.0

Read predicates bind every column they render to their own row ([#796](https://github.com/Dastari/graphql-orm/issues/796)). See the [migration guide](MIGRATION.md#01870-to-01880-read-predicate-columns-bound-to-their-row).

- **Fix (authorization): read-predicate columns no longer resolve against an outer row (#796).** `ReadPredicate::column`, `is_null` and `is_not_null` rendered an unqualified column. Several constructs evaluate their target's predicate inside a correlated subquery: relation, `not_related` and closure predicates (the closure CTE on SQLite and PostgreSQL, and the SQL Server walk), generated related-row filters and relation-count orderings. When a column was declared in the entity's metadata but missing from its physical table, for example after schema drift or before a migration was applied, SQLite, PostgreSQL and SQL Server resolved the name against an outer query's table. The policy was then evaluated against another row and could admit it.
- **Each row has its own alias.** Inside a predicate, "the row this predicate governs" is a value of its own, not text, which each place the predicate is rendered binds:
  - At the root of a statement it binds to the entity's table path, as `"records"."owner_id" = ?`.
  - Inside a subquery it binds to an alias that no table or alias in scope has: `__gom_row_N` for the related or counted row and `__gom_visibility_N` for its derived table, `__gom_closure_N` for a closure's common table expression, `__gom_closure_N_t` for its reached row, `_k` for its key lookup, `_n` and `_e` for its completeness test, and `_hK` for each SQL Server hop.

  A table qualifier alone is not enough on SQLite, which keeps searching outer scopes for another table exposing the same name, such as the same table name in a second schema. The relation keys, the `not_related` NULL-key test and closure pointers are bound the same way. A column the table lacks now fails the statement on every read path, and ordinary policies admit exactly the same rows.
- **No row name is substituted into generated SQL.** The ORM's own predicates are built from SQL text and row references, and a generated where input renders its conditions for the row it is given (`ReadVisibilityFilter::row_bound_filter`), writing `row.<column>` with the column delimited exactly as generated DDL and DML address it (folded first on PostgreSQL, bracketed on SQL Server). A physical column named `__gom_self`, or like a row alias, is therefore still that column: an earlier revision of this change replaced every bare word `__gom_self`, including a column name after the dot. Generated filters elsewhere (root reads, writes, aggregates), where no outer row is in scope, render as before; the `filter_sql_*` snapshots are unchanged, and the `filter_sql_row_bound_*` snapshots pin the bound form.
- **One allocator for aliases and closure names.** Each alias and each closure's common table expression takes the lowest level `N` whose names differ from every reserved name in scope: those the policy's SQL mentions, bare or delimited, and the source and target tables. A relation-count ordering also avoids the source table the statement reads.
- **Generated names are reserved as table names.** The entity derive refuses a table named `__gom_self`, `__gom_row_<n>`, `__gom_visibility_<n>`, `__gom_closure_<n>` or `__gom_closure_<n>_<suffix>` at compile time. A hand-built entity over one is refused with `AUTHORIZATION_MISCONFIGURED` by every read-predicate constructor, related-row filter and count ordering, and when a predicate is attached to a statement over it. Other names starting with `__gom_` are ordinary tables.
- **Hand-written filters are single-row expressions.** Textual binding remains only for a hand-written `ReadVisibilityFilter`, which may write each column as `__gom_self.<column>` (`READ_PREDICATE_ROW`) and return its table from `table_name`. Because a name in SQL the ORM did not write can be spelled in ways that capture a generated alias or the root table, such a clause may only be a boolean expression over the row's own columns and bound parameters. It is refused with `AUTHORIZATION_MISCONFIGURED` (the reason is the error's server-side diagnostic) when, outside string constants, it holds `SELECT`, `FROM`, `JOIN`, `WITH`, `UNION`, `INTERSECT` or `EXCEPT`; a comment; a PostgreSQL `U&` string or identifier; adjacent string constants; a delimited identifier naming the qualifier or a generated alias; `VALUES` or `TABLE`; an unmatched parenthesis or bracket or a semicolon (each clause is validated on its own before composition); a string constant beside a dot; a byte-order mark or other non-ASCII whitespace; a SQL Server `#` name; on SQLite `rowid`, `_rowid_` or `oid`, or `IN` over a table or table-valued function; a numbered or named placeholder, a clause whose `?` placeholders and bind values differ in number, or a filter whose conditions hold a different number of placeholders in total than its flat bind list has values (a surplus value used to be dropped silently); `__gom_self` with no dot after it; or an unclosed string, identifier or dollar quote. Policies over another table use `related`, `not_related` and the closure constructors. Hand-written SQL is trusted host code: the ORM guarantees that an accepted clause is evaluated against the row it governs and cannot change how it composes, and refuses what it cannot classify; deliberately obfuscated SQL is outside the guarantee, and the migration guide names the known residuals (a SQLite column literally named `true` or `false`; SQL Server table names that equal a generated alias only under an accent- or case-insensitive collation).
- **Hand-written filters in correlated subqueries bind every column.** A hand-written predicate that names a column other than through the qualifier is accepted only at the root of a statement. As a relation, `not_related` or closure target, or as the policy of an entity under a generated related-row filter or relation-count ordering, it is refused with `AUTHORIZATION_MISCONFIGURED` instead of being evaluated with a column that could resolve against an outer row. Numbers, parameters, function calls and the dialect's constants (`CURRENT_TIMESTAMP` and the like) are not columns. A word is read as a keyword only where its dialect reads it as one, so `unknown = 0` names a column on SQLite and is unbound.
- **One lexical pass for trusted SQL.** Hand-written policy SQL is read once with a dialect-aware lexer, shared with placeholder counting and rebasing, the alias allocator and the nesting measure. It reads line and block comments (nested on PostgreSQL and SQL Server), strings with doubled quotes, PostgreSQL `E'...'` and dollar-quoted strings, `"..."` identifiers, SQL Server `[...]`, and SQLite backticks and brackets; on PostgreSQL `[` is an array subscript. An ordinary PostgreSQL string is read with `standard_conforming_strings` on, which generated SQL already requires. Placeholder counting now closes a SQLite `[...]` name at its first `]`, as SQLite does.
- **A read policy keeps its own bind values.** An attached policy is stored apart from the caller's `where_clauses` and `values`, which are public and flat, and is composed with them only when a statement is rendered. The caller's state is validated on its own first, by one checked splitter used by the list, count, aggregate, projection, search, delete and write-target statements: every clause was added with exactly its values, the placeholders in total equal the values, and on SQLite no clause holds a numbered or named placeholder (`?N`, `:name`, `@name`, `$name`; also SQL Server `@name`, PostgreSQL `$name`). Otherwise the statement is refused with `AUTHORIZATION_MISCONFIGURED`. Before, a caller clause `status = ?` with two values handed its second value to the policy's placeholder, and a SQLite `?1` left the policy's `?` reading the caller's argument. PostgreSQL `$N` and SQL Server `@pN` stay accepted: each is renumbered in order, and generated filters write them. A query that holds a generated related-row filter or relation-count ordering counts as policed, because those carry their target's policy inside the query's own clause or ordering term; ordering terms are checked like clauses. A transaction's `count()` attaches the policy before the statement is built. A filter that returns its own expression must pair each clause with exactly its values. A query with no read policy is passed through as written, as before. `ReadPredicate::from_filter` reads a hand-written filter once, with the lexer of the predicate's backend rather than the build's default one.
- **PostgreSQL column case in predicate constructors.** `ReadPredicate::column`, `is_null`, `is_not_null`, relation keys and closure pointers now render a column with the same identifier rule as generated filters: a column declared without quotes is folded to lower case before it is delimited, as generated DDL creates it. `db_column = "OwnerId"` used to be quoted as `"OwnerId"`, a different column from the `ownerid` the table has.
- **SQLite implicit row id.** `rowid`, `_rowid_` and `oid` are refused as policy columns on SQLite by the predicate constructors and in hand-written SQL, declared or not: if the column were missing, SQLite would read the implicit row id instead of failing.
- **Closure nesting is counted from how a predicate is built**, not from its text, so closures beside one another never count as nested. Rendered closure instances still count towards the relation budget.
- **Relation-count orderings allocate both aliases.** The counted rows are named `__gom_visibility_<n>` instead of the fixed `__graphql_orm_order_relation`, and the row the target's policy governs `__gom_row_<n>`; both differ from the source table the statement reads and from every name in the policy.
- **Budget.** The composition budget measures the rendered text, so the row names count towards `READ_PREDICATE_MAX_RENDERED_BYTES` and the per-statement relation budget. A predicate is measured as the root of a statement renders it, with the table path, and a subquery is measured again as rendered with its aliases. Each column term is longer by its qualifier plus one byte. Depth, nesting and relation counts are unchanged. A 25,000-term `or` of single-column tests is about 750 KB to 900 KB and builds and runs on every backend. The 50,000-term figure in the 0.181.0 notes no longer fits, because each term is about 1.5 to 2 times longer.
- **Tests.**
  - **Drift regression.** It runs on SQLite, owned PostgreSQL and owned SQL Server. Its target table physically lacks a column named in its policy, and the source table has a column of that name.
    - Every read path fails because the database refuses the missing column, each asserted on its own: repository list and count, GraphQL page, keyset page and point read, the aggregate root, a relation loader and the locking read of a bulk update and delete narrowed to read visibility. GraphQL paths are asserted by their sanitized code and by the server-side diagnostic behind it.
    - The policies cover relation, `is_null`/`is_not_null`, `not exists`, `not_related`, closure (any and all, inside a relation) and `from_filter` shapes, plus a related-row filter and a relation-count ordering under the target's own policy.
    - Before this change, each read admitted rows bound to the source's column. Once the column exists, each shape admits exactly the rows the target's values select. Every source value differs from its target's, in nullness too, so the controls show which row supplied it.
  - **Same table name in two schemas.** A further test runs on all three backends with source and target tables of the same name in two schemas, and with relations nested over the target entity itself whose key column the target lacks. With table qualifiers only, SQLite admitted rows bound to the source; with row aliases every read fails with the missing-column error.
  - **Names.** A column named `__gom_self` beside one named `__gom_row_1` is read as itself; generated table names are refused at compile time, for hand-built entities and when a predicate is attached, and `__gom_orders` is accepted; the allocator is unit-tested against tables named like every alias.
  - **Hand-written filters.** Each refusal is tested per dialect, including an alias spelled `'__gom_row_1'` and an alias spelled like the root table; unqualified, delimited and otherwise-qualified columns are refused in every correlated context and accepted at the root.
  - **Others.** PostgreSQL `OwnerId` beside `"OwnerId"`; SQLite `rowid`, `_rowid_` and `oid`, undeclared and declared; three sibling closures under a negation and three nested ones; the count ordering's aliases; the lexer per construct per dialect.
  - **Budget tests** are resized for the longer terms.

## Unreleased - breaking release: ORM/macros 0.187.0, operation catalogue 0.10.0, AI tool profiles 0.27.0, AI 0.151.0, backup 0.28.0, router 0.17.0

Every change that breaks hosts or invalidates stored fingerprints, bindings,
approvals or sessions ships in this one release. Host steps, in upgrade order,
are in the [migration guide](MIGRATION.md#01860-to-01870-breaking-release).

Schema and storage:

- **Breaking: migration plans no longer drop undeclared tables by default (#441).** `PlanOptions::default()` is `managed_tables_only()`. A plan drops a live table the target does not declare only when the caller plans with `PlanOptions::strict()`. Schema stages and ABI upgrades drop only tables that a stage declared.
- **Breaking: lossy column type changes are `Destructive` (#568).** A type change is `Risky` only when it is a known lossless widening; every other type change needs `ApplyOptions::allow_destructive`.
- **Breaking: integrity-removing steps are `Destructive` (#569).** `SetAppendOnly { enabled: false }`, `DropIndex`, `DropForeignKey`, and `SetCompositeUniques`/`SetCheckConstraints` steps that remove or change a constraint need `allow_destructive`. No new option was added.
- Column UNIQUE/primary-key removal requires destructive admission. Of the append-only steps, only enabling retention purge does: `SetAppendOnly { enabled: true, retention_purge: true }` is `Destructive` on a table that is already append-only and when the prior state is unknown, and `Additive` on an unprotected table; enabling enforcement without purge stays `Additive`. Schema stages classify against the schema they are planned from (the new `PlannedSchemaStage::source_schema` field) instead of an empty schema. Type widening respects backend value domains: on SQL Server, Unicode `NVARCHAR` into code-page `VARCHAR`/`CHARACTER VARYING`/`TEXT` and changes into a length-less `VARCHAR`/`NVARCHAR`/`CHAR`/`NCHAR` are `Destructive`; bare `DECIMAL`/`NUMERIC` is `(18,0)` on SQL Server and SQLite; SQLite `INTEGER` is 64-bit. PostgreSQL validation reports live cascade gaps added outside the ORM.
- **Breaking: the PostgreSQL change journal refuses `TRUNCATE` and unjournaled cascades (#592).** Journaled tables carry a `BEFORE TRUNCATE` guard that refuses the statement unless the effective `graphql_orm.change_journal` setting is `off`; hosts must never configure that default on a role or database. A foreign key that changes rows of a table without a recorder when a journaled row is deleted is refused when the recorder is planned, installed or validated (`ChangeJournalCascadeGap`). `CHANGE_JOURNAL_SETTING` is public. Backup's `clear_restore_target` turns the setting off for its truncate.
- **Added (migrations, #532):** `SchemaManager::apply_migration` and `apply_schema_target` refuse a plan, before any statement runs, when existing rows repeat the key of a unique index the plan creates. The error is read with `UniqueIndexDuplicates::from_error` and names the table, index and key columns only. Previously the database rejected the `CREATE UNIQUE INDEX` statement during the apply and the transaction rolled back with an untyped error.
- **Fixed (migrations):** the unique-index duplicate preflight filters with the predicate exactly as `CREATE INDEX` renders it. On SQL Server that is the Unicode `N'...'` form (#795), so a value outside the database code page no longer hides duplicates from the check; an opaque catalogue filter is checked with its validated text, or left to the database when the plan adds a column to the table or the filter is invalid. SQL Server has no migration apply path yet, so this changes no released behaviour there.
- **Fixed (migrations):** conditional unique-index duplicate preflights skip indexes whose predicate column is added by the same plan. Existing-column predicates still restrict the duplicate check to included rows.

Build configuration and Rust API:

- **Breaking (build configuration):** the 15 mutually exclusive `resolver-case-*`, `argument-case-*` and `field-case-*` features are removed. The `graphql-orm` and `graphql-orm-macros` build scripts read `GRAPHQL_ORM_RESOLVER_CASE`, `GRAPHQL_ORM_ARGUMENT_CASE` and `GRAPHQL_ORM_FIELD_CASE` (an async-graphql rename rule each, `camelCase` when unset) instead, so one build has one naming contract and dependants can no longer fail to compile by enabling different case features. An unknown value fails the build. `graphql-orm-ai`'s `graphql-case-pascal` feature becomes `GRAPHQL_ORM_AI_CASE=PascalCase`, which requires the three ORM variables to be `PascalCase`. The same settings produce the same names ([#508](https://github.com/Dastari/graphql-orm/issues/508)).
- **Breaking (build configuration):** the unused `mysql` feature of `graphql-orm-macros` is removed. It selected no generated code and the ORM has no MySQL backend.
- **Breaking (fail closed):** generated resolvers that take an input object refuse with `INPUT_VALIDATION_REQUIRED`, before authorization or database work, when the schema does not install `GeneratedInputValidation`. Generated schema builders install it; a schema built by hand from generated roots adds `.extension(graphql_orm::graphql::input_validation::GeneratedInputValidation)`. Without it a wrong-shaped input object used to fail at the field, uncoded, after earlier root fields may have written ([#753](https://github.com/Dastari/graphql-orm/issues/753)).
- **Breaking:** every `schema_roots!` schema has a compile-time maximum page limit (`max_page_limit: N`, 100 when omitted). The schema builders panic for a `Database` that applies another maximum or none, so the semantic catalogue and the relation complexity estimate always agree with the database ([#674](https://github.com/Dastari/graphql-orm/issues/674), [#667](https://github.com/Dastari/graphql-orm/issues/667)).
- **Breaking:** a keyset or authorized-scan read without a GraphQL context (`Record::keyset_page`, `keyset_connection_page`, `tx.keyset_page`, `fetch_authorized_keyset*`, `fetch_authorized_scan`) refuses a limit above the maximum page limit with `PAGE_LIMIT_EXCEEDED` instead of returning fewer rows than asked for (#667).
- **Breaking:** so does a trusted `EntityQuery::paginate(page)` read through `fetch_all`, `fetch_all_with_auth`, `fetch_all_on`, `fetch_one*`, `fetch_connection`, `fetch_connection_with_auth` or `fetch_connection_selected_with_auth`, and an `EntitySearchQuery::paginate(page)` read, with or without `refuse_oversized_page_limit`. A limit at or below the maximum, or below `EntityQuery::with_max_limit(n)`, is exact. Generated GraphQL resolvers resolve the client page first with `ClientPaginationConfig::resolve_client_page`, so client pages are still reduced, or refused only when the host opted in (#667).
- **Breaking (Rust API):** `SqlValue` is non-exhaustive and gains `Temporal(TemporalValue)` with `TemporalKind` and `From` conversions for chrono's `NaiveDate`, `NaiveDateTime`, `DateTime<Utc>`, and `DateTime<FixedOffset>`. Update exhaustive matches and host mutation hooks that expect date values to be `SqlValue::String`.
- **Breaking (Rust API):** `RuntimeQueryErrorCode` and `RuntimeRelationErrorCode` gain `CursorKeyTooLarge`; exhaustive matches need an arm. `code()` returns it for a cursor key that is too long, instead of 0.164.0's `ResourceLimit` with `detail() == Some("cursor_key_too_large")`; hosts that match that pair must match the new variant. ([#580](https://github.com/Dastari/graphql-orm/issues/580)).
- **Breaking (removal):** the `graphql::orm::RelationLoader` trait and its generated `load_relations` / `bulk_load_relations` / `*_with_auth` implementations are removed. They preloaded relations without entity, row or field policy. The `graphql::loaders::RelationLoader` data loader becomes generated-resolver plumbing for the same reason: it loaded any key it was given with no policy. It leaves the prelude and loses `new` and its `Loader<String>` impl, and its relation keys get private fields; their constructors are generated-only, `#[doc(hidden)]` and outside semver, and a key built by hand applies no policy. Handwritten schemas register it with the new `graphql::loaders::relation_data_loader`; `BatchLoadEntity` is no longer used ([#445](https://github.com/Dastari/graphql-orm/issues/445)).
- **Breaking (backup 0.28.0): result and error types.** `PruneResult` and `DeleteSnapshotResult` gain `deleted_snapshot_ids`, an unadmitted snapshot is refused with `BackupError::SnapshotNotAdmitted`, and all three types are `#[non_exhaustive]`.

Defaults and behaviour:

- **Breaking (secure default):** `DatabaseBuilder::narrow_bulk_writes_to_read_visibility` defaults to `true`. Generated GraphQL `updateMany` and `deleteMany` add the SQL predicate of the row policy's `read_visibility` to their locking read when no `write_target_prefilter` is declared, so rows outside the caller's read visibility are not passed to row callbacks or counted, and on PostgreSQL not locked. A row the caller may write but not read is left out unless a `write_target_prefilter` selects it, and an error from `read_visibility` on the `GraphqlMutation` surface refuses the bulk write. `narrow_bulk_writes_to_read_visibility(false)` restores the previous target selection. The default does not enable `mssql_snapshot_bulk_write_targets`, which stays a separate opt-in, off by default. Release 0.177.0 shipped the narrowing as an opt-in ([#752](https://github.com/Dastari/graphql-orm/issues/752)).
- **Breaking (secure default):** `DatabaseBuilder::max_page_offset` defaults to `PaginationConfig::DEFAULT_MAX_OFFSET` (`Some(10_000)`) instead of `None`, so a `Database` refuses a client page offset above 10,000 with `PAGE_LIMIT_EXCEEDED` before any row is read. `max_page_offset(None)` (or `Database::with_max_page_offset(None)`) is the explicit opt-out that leaves client offsets unbounded. Trusted builder offsets (`offset(n)`) and a `ClientPaginationConfig` built from a `PaginationConfig` without a `Database` stay unbounded. Release 0.169.0 shipped the bound as an opt-in ([#456](https://github.com/Dastari/graphql-orm/issues/456)).
- **Breaking (security default):** an entity-policy, row-policy, write-transform or mutation-hook error with no `code` extension is reported as `INTERNAL_ERROR` with the default message by generated GraphQL operations, `graphql_error_from_sqlx` and repository callers; its diagnostic text stays server-side. Relation boundaries are the exception and are unchanged: an authored error without a public code from relation authorization, including a relation field's read policy, is still `FORBIDDEN` with no message. `DatabaseBuilder::expose_uncoded_callback_messages(true)` (or `Database::with_uncoded_callback_messages_exposed(true)`) restores the old GraphQL rendering for the schemas built from that `Database` only, through the new `graphql::errors::UncodedCallbackMessages` schema extension; repository callers see the default message either way ([#682](https://github.com/Dastari/graphql-orm/issues/682)).
- **Breaking (row policies):** `ReadPredicate::related(keys, target)` is three-valued: UNKNOWN when a source key is NULL or when no related row has `target` TRUE but one has it UNKNOWN. Positive results and their single `EXISTS` rendering are unchanged; a negation, also one reaching the relation through `and`/`or`, renders `<keys IS NOT NULL> AND NOT EXISTS (<related rows where target is not FALSE>)` and no longer admits rows with a NULL key or only UNKNOWN related values. Double negation is the identity, `ReadPredicate::not_related` is `related(..).not()` and renders that form with the target bound once, and a negated relation is budgeted like the admitting form: one `CASE` level per relation level, so a chain of more than ten nested relations cannot be negated. A `not_related` or negated closure whose target contains a relation can admit fewer rows than before, because an inner relation with a NULL key is UNKNOWN, not FALSE ([#770](https://github.com/Dastari/graphql-orm/issues/770)).
- **Breaking:** runtime GraphQL serves `RecordVersion` and integer live keys as `RuntimeInt64` strings by default. `RuntimeGraphqlOptions::with_legacy_int_identities()` keeps the `Int` form ([#528](https://github.com/Dastari/graphql-orm/issues/528), item 4).
- **Breaking (router 0.17.0): composed `@authenticated` and `@requiresScopes` are enforced on every selected field (#729).** An authenticated router evaluates the directives on nested object fields, joined entity fields, interface and union members, fragments and aliases, as well as on root fields, and refuses the whole operation before any subgraph request with `UNAUTHENTICATED` or `FORBIDDEN` and the response path. Per-subgraph scope requirements are combined with AND semantics, implementations inherit rules declared on their interfaces (including `@interfaceObject` types), and a malformed or empty scope set, an authorization directive spelling that does not resolve, or an authorization directive on an argument, input field or other unsupported location refuses the graph when it is built. It used to check only root fields' protocol descriptors. `__typename` is never refused, and the router still grants nothing. See the [router changelog](crates/graphql-orm-router/CHANGELOG.md).

Fingerprints, sessions and approvals:

- **Breaking (fingerprints, #683):** generated-operation fingerprints no longer include the Rust module path. `GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM` is `graphql-orm-sha256-len-v2`; the catalogue is ordered by entity name and table, not by Rust path. Moving or re-exporting an entity no longer changes any fingerprint. Every operation, catalogue and router-export fingerprint changes once. `stable_type_id` is still accepted and no longer affects fingerprints. AI tool profiles 0.27.0 writes and requires the new algorithm name in tool manifests.
- **AI 0.151.0, AI tool profiles 0.27.0 (#551):** capability broker version 2. Describe returns compact typed signatures and `describe_type` expands one named type at a time; discovery returns one entity capability per entity with the root chosen per call; search uses an inverted index with fixed per-entry weights; large surfaces default to the fixed broker, with an opt-out. Retained fixed-broker sessions rebind once.
- **AI 0.151.0:** fixed-broker sessions survive catalogue changes, `capability_stale` and `selection_stale` are separate model-facing codes (#551), HighImpact approvals force step-up and the approval lifetime cap has a 7-day ceiling (#465), and the AI schema module adds three unique indexes (#532, module `0.83.0`). See the [AI changelog](crates/graphql-orm-ai/CHANGELOG.md).

Added, opt-in or no action:

- **Added (SQL Server):** chrono parameters bind natively. Generated DDL maps `NaiveDate` to `DATE`, `NaiveDateTime` and `DateTime<Utc>` to `DATETIME2`, and `DateTime<FixedOffset>` to `DATETIMEOFFSET`, including optional fields, so entities with chrono fields or `#[date_field]` text fields now compile and write on SQL Server.
- **Added (SQL Server, opt-in):** `MssqlPoolOptions::typed_temporal_parameters(true)` uses native types for generated temporal text writes and date-filter operands. The default sends those values as text. Review the effects on text columns, offset comparisons, and legacy `datetime` precision before opting in. Runtime GraphQL date-time values, keyset cursor values, and backup restore values remain text. [Migration](MIGRATION.md#sql-server-native-temporal-parameters-521) · [#521](https://github.com/Dastari/graphql-orm/issues/521)
- **Added (opt-in per entity):** `#[graphql_entity(key_only_deletes)]` sends a key-only `DELETED` event to `Key`/`Keys` subscribers of the exact key when a delete releases no row, after checking the subscriber's authentication, scopes, assurance, entity access and read access to every key field through the host field policy. It is refused at compile time when a primary-key field declares a `read_policy`, sets `subscribe = false`, is `graphql(skip)` or is not GraphQL-readable ([#666](https://github.com/Dastari/graphql-orm/issues/666)).
- **Added (router 0.17.0, opt-in): the router says when it refused an operation before execution (#764).** With `RouterConfig::with_refusal_outcome(true)` (`refusalOutcome: true`), each error of an operation the router refused before sending any subgraph request carries `extensions.outcome: "refused"`, the value generated services send under `mutation_error_outcome(true)`, over HTTP and the WebSocket gateway. An operation that reached a subgraph, an execution failure and the request timeout are never marked, and a subgraph's own `outcome` passes through. Off by default. See the [router changelog](crates/graphql-orm-router/CHANGELOG.md).
- **Fixed:** `EntitySearchQuery::pagination_config` keeps the client page-offset maximum the search already had; it used to drop it, so a search whose page limits were replaced served any client offset.
- **Fixed (AI tool profiles, AI):** runtime live key inputs compile under the ORM's default `RuntimeInt64` identities as well as `with_legacy_int_identities()`. A UUID-keyed runtime collection with live subscriptions and a node identity no longer fails every capability catalogue with `InvalidConfiguration`.
- **Tests:** the 40 trybuild compile-contract binaries of `graphql-orm` are one `compile_ui` test binary with a module each (#508).

## Unreleased - ORM/macros 0.186.0

- Owned PostgreSQL tests wait for the server to accept TCP connections before
  connecting. Startup reports the container name if readiness exceeds 30 seconds.
  [#806](https://github.com/Dastari/graphql-orm/issues/806)

## Unreleased - ORM/macros 0.185.0

- SQL Server migrations render closed-set filtered-index string values as
  Unicode literals (for example, `N'状態'`). Rebuilding an index preserves
  values outside the database code page. ASCII values also gain the `N`
  prefix. Opaque filters read from SQL Server are emitted verbatim. See the
  [migration guide](MIGRATION.md#01830-to-01850-sql-server-filtered-index-literals)
  ([#795](https://github.com/Dastari/graphql-orm/issues/795)).

## Unreleased - ORM/macros 0.183.0 and operation catalog 0.9.1

- The workspace dependency check compares documented direct edges with
  Cargo metadata, including optional and dev-only dependencies (#503).
- Install examples use a reviewed Git revision and point to the package
  inventory and release manifest. Direct Auth guidance follows the selected
  revision's workspace manifest (#509).

## Unreleased - ORM/macros 0.182.0

SQL identifier quoting fixes ([#782](https://github.com/Dastari/graphql-orm/issues/782), [#783](https://github.com/Dastari/graphql-orm/issues/783)), found by the identifier-quoting property suite. See the [migration guide](MIGRATION.md#01810-to-01820-sql-identifier-quoting).

- **Fix (SQL rendering): only one well-formed delimited identifier is passed through (#782).** `SqlDialect::quote_identifier`, `DatabaseBackend::render_identifier`, `quote_identifier_path`, `render_identifier_path` and the derive macros returned any input unchanged that merely started and ended with a delimiter, so text such as `"a"b"`, `"`, `[a]]` or `[a] , [b]` reached SQL as an unterminated identifier or as more than one token. Input is now passed through only when it is exactly one well-formed, non-empty delimited identifier for the dialect (every inner closing delimiter doubled; SQLite `[...]` with no `]` inside). Any other input is delimited as one literal name with the closing delimiter doubled, so the result always reads back as exactly one identifier. Well-formed delimited names and paths, and every undelimited name, render byte-identically.
- **Declared names that start and end with a delimiter but are not one well-formed delimited identifier are refused at compile time (#782).** A `#[graphql_orm(table = "...")]` or `db_column = "..."` path part such as `"a"b"` or `[a] , [b]` now fails the derive with a spanned error that names the part, instead of generating malformed SQL.
- **Fix (SQL Server): names outside the regular-identifier rule are bracketed (#783).** `DatabaseBackend::Mssql.render_identifier` wrote every non-ASCII name bare, and SQL Server refused names such as `x²`, `雪🦀`, `x\u{3000}y` or names with letters added after Unicode 3.2 with error 102. A name is now written bare only under SQL Server's regular-identifier rule, measured against the server for every Basic Multilingual Plane character: a first character that is a Unicode 3.2 letter, letter number, U+FF3F or `_ @ #`, then those, Unicode 3.2 marks, decimal digits, connector punctuation, format characters or `$`. Everything else is bracketed. Every name SQL Server accepted bare keeps its exact text, so runtime DDL, migration plans and their hashes are unchanged for those names. The tables are generated from Unicode 3.2.0 by `scripts/generate-mssql-identifier-tables.py`. The derive macros already bracket every SQL Server name.
- **Schema introspection treats catalogue names literally.** SQL Server introspection quotes every raw schema, table and column name it reads back unconditionally, with `]` doubled, both in the reported model and in the `OBJECT_ID` lookups for indexes and foreign keys, so a table literally named `[ledger]` no longer reads `ledger`'s metadata or collides with it. SQLite `PRAGMA` introspection and the PostgreSQL search-metadata read quote raw names literally too, and PostgreSQL foreign-key targets render a raw name bare only when it is plain, never passing it through as SQL text.
- **Paths are split with each backend's own delimiters.** The runtime and derive path splitters read only the dialect's delimited forms: `"..."` on PostgreSQL; `"..."`, backticks and `[...]` (closed at the first `]`) on SQLite; `[...]` and `"..."` on SQL Server. A PostgreSQL part that starts with `[` no longer swallows the following separator.
- **Empty and NUL names.** A delimited identifier containing NUL is never passed through, and the derives refuse a declared name containing NUL. SQLite's empty delimited names (`""`, `[]`, two backticks) are still passed through; on PostgreSQL and SQL Server, which refuse them, they are quoted literally.
- **Table-qualified columns in read predicates, group pages and dependent deletes.** New `DatabaseBackend::quote_column_identifier` renders the column part of a declared column path. `ReadPredicate::column`, `is_null`/`is_not_null`, and the relation, `not_related` and closure joins use it, so a `db_column` such as `"t"."c"` names column `c` through any alias instead of erroring or (on SQLite) reading as a string constant. A qualifier that does not name the entity's own table is refused by the derives at compile time, so no generated filter, sort or projection references another table, and by read predicates with `AUTHORIZATION_MISCONFIGURED` rather than stripped (SQL Server compares the qualifier's exact text). Dependent-delete selection receives its column rendered by the caller: a declared key as its column part, a catalogue foreign-key column as one literal name (SQLite and PostgreSQL report raw names, always escaped and delimited; SQL Server reports them delimited), so a literal `parent.ref` or `"x"` is never read as `ref` or `x`; the `exclude_blank` filter of `fetch_group_page` uses the path renderer, as grouping and metrics do. A test checks the exact rows admitted through each predicate on SQLite, owned PostgreSQL and owned SQL Server.
- **Catalogue names round-trip.** SQL Server introspection reports bracketed or dotted index, index-column, filtered-index predicate and foreign-key constraint names delimited, so dropping or recreating them names the same object, and its filter parser is character-boundary safe and bracket-aware, so a filter on a non-ASCII column reads back instead of panicking. PostgreSQL reports foreign-key targets and table names that bare SQL would fold or split, or that start with `"`, delimited, and always delimits another schema's name (`"Archive"."Parent"`, `"local.parent"`), so they resolve to the referenced table. Ordinary names are reported as before.
- **NUL in PostgreSQL row-level security columns.** The policy column helper no longer passes through a quoted name containing NUL.
- New tests: a seeded property suite (`identifier_quoting_properties`) checks that every input quotes to exactly one identifier naming it, that bare renderings are plain and unreserved, and that each rendering round-trips through `CREATE TABLE`, `INSERT`, `SELECT` and `DROP` on in-memory SQLite and owned PostgreSQL and SQL Server, with the minimized cases from both issues retained. An owned SQL Server test compares the generated tables with the server for every non-ASCII Basic Multilingual Plane character, and another introspects tables named `ledger` and `[ledger]` side by side. Path properties round-trip `schema.table` paths in every delimited form through each database, and a group-page test covers a table-qualified column on SQLite and owned PostgreSQL. The shared runtime/derive cases gain the minimized malformed inputs, and a derive unit test covers the compile-time refusal.

## Unreleased - ORM/macros 0.181.0

Transitive-closure read predicates on SQL Server ([#779](https://github.com/Dastari/graphql-orm/issues/779)). See [read-predicate transitive closures](docs/reference/graphql-orm/pagination-migration.md#read-predicate-transitive-closures) and the [migration guide](MIGRATION.md#01800-to-01810-closure-read-predicates-on-sql-server).

- **`ReadPredicate::related_closure`, `related_closure_all` and `related_closure_within` work on SQL Server.** They were refused there with `AUTHORIZATION_MISCONFIGURED`, because SQL Server does not allow a common table expression inside a subquery. SQL Server now renders the bounded walk as a correlated breadth-first search: one nested derived table and one correlated `OUTER APPLY` key lookup per hop, deduplicated per level by pointer with the hop it was first reached at, then one lookup per reached pointer to evaluate the target. It adds no schema objects or DDL, is evaluated per row in SQL (never in memory or for the whole table), and applies on every read path: reads, counts, aggregates, keyset and connection pages, and subscription delivery. SQL Server has no search execution, so search does not apply there.
- **Identical semantics.** Three-valued TRUE/FALSE/UNKNOWN, fail-closed incomplete walks (a partially NULL composite pointer, a non-NULL pointer naming no row, or the depth bound), all-NULL pointers as roots, cycles that are complete once the bound covers them, both quantifiers, `not()` and the deny-fallback refusal all match the CTE form.
- **SQL Server bound.** `related_closure_within` accepts 1 to the new `READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH` (64) hops on SQL Server and refuses larger bounds with `AUTHORIZATION_MISCONFIGURED`. SQL Server refuses statements nested about 100 levels deep. The default bound (64) is unchanged on every backend, so `related_closure` and `related_closure_all` behave identically everywhere. SQLite and PostgreSQL keep the 1,024-hop maximum.
- **Cost.** Each level holds only distinct pointers, so the work per row is bounded by the bound times the distinct pointers reached, whatever the fan-out. Each hop's `OUTER APPLY` carries a no-op `TOP (2147483647)` fence, which keeps plan compilation fast (about 0.1 to 0.3 s for a first compile at the default bound) and keeps every hop a nested-loops seek on the key.
- **Composition budget on every backend.** Relations, negations and closures inline their target (a closure four times), and a closure inside a closure evaluates the inner walk per reached row, so unbounded composition could render a multi-megabyte statement, run for minutes or exhaust server memory. Every `ReadPredicate` is now measured as it is built. A constructor, `not()`, `and` or `or` that would exceed a limit is refused with `AUTHORIZATION_MISCONFIGURED`, before anything is rendered:
  - per predicate, at most `READ_PREDICATE_CLOSURE_MAX_NESTING` (2) closures nested inside one another;
  - per predicate, at most `READ_PREDICATE_MAX_RENDERED_BYTES` (1 MiB) of rendered SQL, with placeholders counted at full width;
  - per predicate, at most `READ_PREDICATE_MAX_EXPRESSION_DEPTH` (256) levels of expression nesting, counted as nested parentheses;
  - per predicate, at most `READ_PREDICATE_MAX_CASE_NESTING` (10) nested `CASE` expressions, SQL Server's own limit (error 125). Each `not_related` adds two and each closure one, so `not_related` chains, whose target is inlined twice per level, stop at five;
  - per predicate, at most `READ_PREDICATE_MAX_RELATION_NESTING` (32) nested relations and `READ_PREDICATE_MAX_RELATIONS` (128) relations in total on SQLite and PostgreSQL, or `READ_PREDICATE_MSSQL_MAX_RELATION_NESTING` (9) and `READ_PREDICATE_MSSQL_MAX_RELATIONS` (9) on SQL Server. Each closure counts as `READ_PREDICATE_CLOSURE_RELATION_WEIGHT` (16) relations, or `READ_PREDICATE_MSSQL_CLOSURE_RELATION_WEIGHT` (1) on SQL Server, counted as rendered;
  - per predicate on SQL Server, at most `READ_PREDICATE_MSSQL_MAX_NESTING` (75) nested subqueries and `READ_PREDICATE_MSSQL_MAX_NESTING_UNITS` (900) weighted levels (11 per subquery, 2 per `NOT (`, 1 per other parenthesis). The owned server failed with "nested too deeply" (error 191) from about 1,050 units (97 subqueries, or 550 negations, alone);
  - per statement, at most `READ_PREDICATE_MAX_STATEMENT_RELATION_BYTES` (1 MiB) of policy SQL and `READ_PREDICATE_MAX_STATEMENT_RELATIONS` (64; `READ_PREDICATE_MSSQL_MAX_STATEMENT_RELATIONS`, 5, on SQL Server) relations added by all generated relation filters and relation-count orderings together, each also checked like a predicate and each count ordering charged at least one. A statement therefore holds at most 2 MiB of policy SQL and 192 weighted relations (14 on SQL Server);
  - per request, at most `MAX_ORDER_BY_TERMS` (16) terms in a generated `orderBy` list, refused as `INVALID_INPUT`.
- **Fix (availability, reachable from a client): relation-heavy statements exhausted server resources.** A host policy of a deep relation chain, combined by a client with many relation filters (`parents(where: {or: [16 × {kids: …}]})`, each embedding the policy), passed every per-fragment check. On PostgreSQL memory grows steeply with nested and total relations: a 128-deep chain peaked at 2.5 GB, and that shape would have needed about 40 GB (extrapolated), enough to exhaust a shared host. SQL Server failed with 701 (out of memory for the whole server) on an `or` of 256 relation chains, and with 8623 and 8621 on smaller shapes. Closures and relation-count orderings were not charged at all. The relation limits above, closure weights and count-ordering charges refuse those shapes when the policy is built or the request is prepared. Measured margins: with the limits lifted, SQL Server statements failed from 28 weighted relations, twice its per-statement 14; within the limits, every probed shape ran or was refused, a 4 GB-capped PostgreSQL peaked at about 470 MB, and the 3 GB-capped SQL Server at 1.49 GB. Not guaranteed: SQL Server plan-resource errors (8623, 8621) on untested shapes can still fail individual reads, and execution time is not bounded (admitted `or` and closure shapes took up to about 8 s on 60 rows on PostgreSQL, and about 4.5 s on SQL Server for a policy of nine closures with nested closures). Set PostgreSQL `statement_timeout`, and on SQL Server a query timeout and Resource Governor limits.
- **Fix (availability): deep `and`/`or` folds no longer overflow the stack.** `and` and `or` nested a binary node per call and rendering recursed once per node. A host folding about 2,000 grants with `p = p.or(q)?` built a predicate whose render overflowed a 2 MiB stack and aborted the process (about 30,000 terms in a release build). This affects releases since `ReadPredicate::and`/`or` were added. They now flatten a child of the same kind, so a fold of any length is one node. Every `and`/`or` list of three or more operands renders as a balanced binary tree of parenthesised pairs, so a list of `n` operands is about `log2(n)` levels deep to the SQL parser. Each counted level is one parser level. Generated where inputs render their `and`, `or` and `not` lists the same way. A plain chain is as deep as it is long, and SQLite refused one past 1,000 operands. A 50,000-term `or` is 18 levels deep, and builds and runs on SQLite, PostgreSQL and SQL Server. Deeper alternating trees and `not()` chains are refused by the expression limit above; on SQLite, every tested family fails only from 450 counted levels or more.
- **Fix (SQL Server closure):** the authorized row's own target is evaluated in a scope that sees no internal `__gom_` column, so a target column named like one (for example `__gom_max`) binds to the row, not to the closure's summary.
- New tests run the closure suite on owned SQL Server: truth tables, the depth bound, keyset and connection pages, refusals, rendering and bind order, and a 3,000-row performance check that asserts the plan shape. Subscription delivery also runs on owned SQL Server. A new parity test runs 248 generated cases on SQLite, owned PostgreSQL and owned SQL Server and checks each admitted set against a reference evaluator of the documented rules. The cases cover cycles, self-loops, chains past the bound, unreachable and partially NULL pointers, composite and fan-out keys, nested closures and `not()`. Budget tests cover each refusal on every backend and run the largest policies that build on every read path on SQLite, owned PostgreSQL and owned SQL Server. A further test covers a target column named like an internal column. Expression-depth tests cover refusals at the limit on every backend, the deepest `not()` chains and alternating trees that build running on every read path (on SQL Server also around the deepest closure nesting), a 50,000-term fold on every backend, rendering at the limit on an explicit 2 MiB stack, and the per-statement relation budget through generated GraphQL relation filters. Relation and CASE tests refuse the reviewed deep-chain and many-relation-filter shapes, through generated GraphQL, and run the largest admitted shapes on every backend. A budget-shape suite runs, on all three backends, every shape grown to the limits on every statement and GraphQL path, including wide alternations, relation chains with negations and long generated `or` lists, and checks on SQLite and PostgreSQL that balanced grouping matches the plain chain on 150 random trees.

## Unreleased - ORM/macros 0.180.0

A separate opt-in lets narrowed SQL Server bulk writes select their targets from snapshots, so they no longer lock or wait on hidden rows ([#752](https://github.com/Dastari/graphql-orm/issues/752)). See the [migration guide](MIGRATION.md#01791-to-01800-opt-in-sql-server-snapshot-bulk-write-targets).

- **New opt-in: `DatabaseBuilder::mssql_snapshot_bulk_write_targets(true)`** (also `Database::with_mssql_snapshot_bulk_write_targets(bool)`, getter `mssql_snapshot_bulk_write_targets_enabled()`). It is off by default and stays separate from `narrow_bulk_writes_to_read_visibility`, which it requires. Narrowing alone keeps SQL Server's serializable `UPDLOCK, HOLDLOCK` read.
- **What the opt-in changes.** It applies to generated GraphQL `updateMany`/`deleteMany` with a SQL predicate (read visibility or a `write_target_prefilter`), when all of these hold:
  - the database has `READ_COMMITTED_SNAPSHOT` on;
  - the target is a base table with an enabled, unfiltered unique index whose only key column is the key;
  - every index of the table allows row locks;
  - the key is `uniqueidentifier`, `nvarchar`/`nchar` or an integer.

  The write then:
  - reads every candidate in one snapshot without a lock (at most `max_scanned_rows` + 1);
  - locks those keys exclusively with `XLOCK` and a `FORCESEEK` on that index (read committed, so no key-range lock reaches a neighbouring row), and checks the filter and the predicate again on each locked row (the re-check column alias can't collide with a table column of the same name);
  - reads the candidates again in a new snapshot, under the same budget. If they are not exactly the locked rows that still match, the write is refused with a retryable `SERVICE_UNAVAILABLE` and nothing is written.

  Otherwise the serializable locking read is kept.
- **Exact guarantee.** The target set equals the rows that match at the validation snapshot, and they cannot change before the write commits. An atomic multi-row insert is never split. The write never locks, or waits on a lock on, a row outside its candidates, whatever plan SQL Server picks, except through lock escalation (see below).
- **Not serializable.** No range is locked, so another transaction can commit a row that starts to match, with other changes, without waiting. That row isn't written, but later reads in the same bulk write (hooks, field authorizers, triggers) can see the other changes.
- **What else remains.**
  - **Exclusive locks:** candidates stay exclusively locked until the write ends, including rows that drop out. Readers using row versions don't wait, and neither does a `READCOMMITTEDLOCK` read of an unchanged row. Readers that hold locks (`REPEATABLE READ`, `SERIALIZABLE`, `UPDLOCK`) wait.
  - **Lock escalation** to a table lock still covers hidden rows.
- Defaults are unchanged on every backend. PostgreSQL and SQLite are unchanged in both modes.
- Generated `updateMany` and `deleteMany` pass the runtime a key accessor, so ORM and macros must be upgraded together.
- **New tests (owned SQL Server):**
  - **Hidden-row locks:** with the opt-in, a narrowed bulk write neither waits on another transaction's update lock on a hidden row (the next key after a target) or on an uncommitted change to one, nor holds a lock on another hidden row. Narrowing alone, the unnarrowed controls and `READ_COMMITTED_SNAPSHOT` off keep the single locking read.
  - **Atomic insert:** a concurrent atomic two-row insert is never split. With the opt-in it refuses the write as a retryable conflict, and the retry writes both rows. Without it, the range locks block the insert.
  - **Key path by key type:** `nvarchar` and integer keys take the key path. `varchar`/`char` keys keep the single read.
  - **Key path by table shape:** a unique non-clustered key index takes the key path. A heap, a non-unique key index, a composite unique index, a view, and an index with `ALLOW_ROW_LOCKS = OFF` keep the single read.
  - **Budget and bound:** the scan budget bounds both candidate reads, and the bulk bound counts targets.
  - **Alias:** a column named `__gom_still_matches` works.
- **New tests (every backend):** a narrowed bulk write over three pages of visible rows interleaved with hidden ones writes each visible row once, and the scan budget counts only the narrowed candidates.

## Unreleased - ORM/macros 0.179.1

Test-owned PostgreSQL containers are memory-capped; no runtime change.

- Every PostgreSQL container started by the ORM, backup and AI test harnesses now
  runs with a hard `--memory 4g --memory-swap 4g` limit, matching the SQL Server
  harness, so a runaway test backend cannot exhaust the build host. Library code,
  public API, generated SQL and stored data are unchanged.

## Unreleased - ORM/macros 0.179.0

Security property suites for identifier quoting and canonical JSON ([#725](https://github.com/Dastari/graphql-orm/issues/725)). See the [migration guide](MIGRATION.md#01770-to-01790-identifier-quoting-and-canonical-json-properties).

- **Fix: a SQLite name that starts with U+FEFF is delimited.** SQLite reads a byte-order mark that starts a token as whitespace, so `render_identifier` and the derive macros wrote such a name bare and SQLite used the name without it. Both now delimit it. Other names render exactly as before.
- New seeded property tests check, for SQLite, PostgreSQL and SQL Server, that an undelimited name quotes to one delimited identifier that reads back as the same name, that quoting and rendering are idempotent, that a bare rendering contains only plain identifier characters, and that quoted paths keep each part one identifier. On in-memory SQLite and the owned PostgreSQL and SQL Server containers, each case creates, writes, reads and drops a table named by generated identifiers, and checks the catalogue for exactly the expected names.
- New canonical JSON v1 property tests check that written defaults read back, that equivalent values (key order, `-0.0`, escaped spellings, whitespace) produce identical bytes, that accepted text is a fixed point and every other spelling is refused, and that arbitrary input fails closed at the depth, node and byte limits.
- Known issues found by the suites are tracked with ignored regression tests: [#782](https://github.com/Dastari/graphql-orm/issues/782) (`quote_identifier` passes through text that only starts and ends with a delimiter) and [#783](https://github.com/Dastari/graphql-orm/issues/783) (SQL Server bare non-ASCII names that the server rejects).

## Unreleased - ORM/macros 0.177.0

- **Opt-in: bulk writes narrowed to read visibility ([#752](https://github.com/Dastari/graphql-orm/issues/752), part).** `DatabaseBuilder::narrow_bulk_writes_to_read_visibility(true)` (or `Database::with_narrow_bulk_writes_to_read_visibility`) makes generated GraphQL `updateMany` and `deleteMany` add the SQL predicate of `RowPolicy::read_visibility` (`Prefilter` or `Complete`, `GraphqlMutation` surface) to their locking read when the policy declares no `write_target_prefilter`. Rows outside it are not passed to `can_write_row`/`can_read_row` or counted against the bulk bound or scan budget. On PostgreSQL they are also not locked, so another transaction's lock on a hidden row no longer delays the write. On SQL Server the `UPDLOCK, HOLDLOCK` read still locks rows a scan reads before testing the predicate; an index seek avoids it. Every selected row is still classified by the row callbacks, and visible rows give the same payload and writes. With the setting on, a row outside read visibility that `can_write_row` allows is left out (a declared `write_target_prefilter` takes precedence and keeps it), and a `read_visibility` error refuses the bulk write. Off by default: bulk writes are unchanged. The residual lock and timing behaviour is documented in [strict authorization](docs/reference/graphql-orm/strict-authorization.md#row-policy-on-generated-writes). Single-row writes are unchanged.
- **Coded dependent-delete root refusals ([#763](https://github.com/Dastari/graphql-orm/issues/763)).** On an entity with `dependent_deletes` and `mutation_error_codes`, a root delete the plan refuses (for example `Entity not found` for a row hidden once the transaction holds it) reports the direct delete's `errorCode` (`NOT_FOUND`) instead of null.
- **Error-code reference ([#763](https://github.com/Dastari/graphql-orm/issues/763)).** Lists the router codes `SUBSCRIPTIONS_NOT_SUPPORTED` and `MUTATION_NOT_ALLOWED_OVER_HTTP_GET`, states that a bulk payload's `errorCode` can be a host refusal's own code, and that a subgraph's `extensions.outcome` passes through the router (now covered by a router wire test). The client metadata reference states that `carried_fields` grants no access and points to `classification` for secret fields. A new test checks that a key-field or default-sort change changes the interpretation fingerprint.

## Unreleased - ORM/macros 0.176.0

Transitive-closure read predicates ([#772](https://github.com/Dastari/graphql-orm/issues/772)). See [read-predicate transitive closures](docs/reference/graphql-orm/pagination-migration.md#read-predicate-transitive-closures) and the [migration guide](MIGRATION.md#01750-to-01760-transitive-read-policy-closures).

- **`ReadPredicate::related_closure`, `related_closure_all` and `related_closure_within`.** A policy can admit a row based on every row reachable from it through a self-referencing key, such as `&[("parent_id", "id")]`, with composite keys supported. The existential form is TRUE when some reached row (the row itself included) matches `P`. The universal form is TRUE when every reached row matches `P` and the walk is complete: "no reachable row fails `P`". Both are three-valued and fail closed. An UNKNOWN `P`, a partially NULL composite pointer, a pointer that names no row, or a walk cut short by the depth bound makes the result UNKNOWN, so the universal form never admits on it and a negation never admits either. An all-NULL pointer ends the walk at a root. Cycles terminate, and a cycle is complete once the bound covers its length. The default bound is `READ_PREDICATE_CLOSURE_DEFAULT_DEPTH` (64) hops, and `related_closure_within` accepts up to `READ_PREDICATE_CLOSURE_MAX_DEPTH` (1,024). As with `related`, the policy owns the walk: rows hidden by their own policy are reached like any other. The result reveals, hop by hop, what `related`/`not_related` reveal, plus whether the walk was complete, which can only deny a row.
- It renders a correlated `WITH RECURSIVE` scalar subquery on SQLite and PostgreSQL, used by every read path: reads, counts, aggregates, keyset and connection pages, search and subscription delivery. SQL Server cannot nest a common table expression in a subquery, so there the closure is refused with `AUTHORIZATION_MISCONFIGURED` when the policy is built, rather than evaluated in memory.
- New tests cover truth tables with NULLs and UNKNOWNs, chains, cycles and self-loops, unreachable and ambiguous steps, composite and fan-out keys, nested relations and closures, the depth bound, universal versus existential forms and their negations, bind order, refusals, keyset and connection pages and aggregates on SQLite and owned PostgreSQL, a performance check over 3,000 rows with a 1,000-row chain, and subscription delivery and search on SQLite. An owned SQL Server test confirms the refusal and the server limitation behind it.

## Unreleased - ORM/macros 0.174.0

Read-policy negation with a stated fail-closed NULL rule ([#766](https://github.com/Dastari/graphql-orm/issues/766)). See [read-predicate negation and NULLs](docs/reference/graphql-orm/pagination-migration.md#read-predicate-negation-and-nulls) and the [migration guide](MIGRATION.md#01730-to-01740-read-policy-negation).

- **`ReadPredicate::not_related`.** The fail-closed "no related row matching P" counterpart of `ReadPredicate::related`, with the same key, entity and backend validation. It is three-valued: UNKNOWN when a source key is NULL, FALSE when a related row has `P` TRUE, UNKNOWN when no related row has `P` TRUE but one has it UNKNOWN, TRUE otherwise (including no related rows). It renders a `CASE` over two correlated `EXISTS` subqueries on SQLite, PostgreSQL and SQL Server, so it composes with `not`, `and` and `or` under the same rule as every other predicate, and `not_related(k, p).not()?` admits exactly the rows of `related(k, p)`. As with `related`, the policy owns the join and the target's row policy is not imported. The outcome reveals what `related(k, p)` reveals plus whether some related row, hidden or not, has `p` UNKNOWN; that case fails closed (it can deny a row, never admit one).
- **`ReadPredicate::is_null` and `is_not_null`.** Validated, two-valued NULL tests on persisted columns, including private ones, for policies that want NULL rows (`p.not()?.or(ReadPredicate::is_null::<B, T>("column")?)?`) or every row with a value.
- **Fix (authorization, fail closed): negating the deny fallback is refused.** A generated filter renders the deny fallback `1 = 0` for an empty `in_list` or `or` list, for a filter that fails validation (such as a list over `MAX_FILTER_LIST_OPERANDS`, 1,000, operands) and for an invalid filter value. `ReadPredicate::not()` now refuses with `AUTHORIZATION_MISCONFIGURED`, when the policy is built, any predicate that renders `1 = 0` anywhere, including inside a relation. Previously, negating such a predicate admitted all rows of the entity, including rows whose column is NULL; it is now refused. `not_related` refuses such a target too. A generated client filter's own `not: { field: { inList: [] } }` is unchanged: it still matches every row the request can see, and row policy still applies.
- **Documented NULL rule.** `not()` keeps SQL three-valued semantics: it is TRUE only when the inner predicate is FALSE, so a NULL compared column never admits a row, double negation is the identity, and the rule holds in nested `and`/`or` trees. `related(..).not()` remains the two-valued `NOT EXISTS` it has been since 0.92.0. Every read path, including keyset and connection pages, authorized scans, search and subscription delivery, applies the same SQL clause.
- New tests run truth tables with NULLs for negation, double negation, nested trees, `related`/`not_related` with matching, non-matching, UNKNOWN and absent related rows and NULL keys, empty-list refusals, bind order and placeholder numbering, and keyset and connection pages over negated policies on SQLite and owned PostgreSQL and SQL Server; a SQLite test checks that subscription delivery admits exactly the rows of the SQL read.

## Unreleased - ORM/macros 0.173.0

- The operation catalogue 0.9.0 and companion package releases declare the same
  minimum compiler; serialized contracts and stored data remain unchanged.
- Refresh eight root-collision diagnostic snapshots for the pinned Rust 1.97.1
  panic-display excerpt and require `rust-src` in the toolchain/ORM CI setup; collision rejection and error messages are unchanged.
- Declare Rust 1.97.1 consistently across published packages, pin development to
  1.97.1 and verify workspace/backend/provider MSRV graphs in CI (#501).
- Retire the obsolete macros postgres-smoke fixture and its floating PostgreSQL
  image (#684; removes its #615 pin only).
- Validate scoped-release and router-binary approval URLs through the GitHub API
  using the guard job token with read-only PR permission (#499).
- Retain Tiberius 0.12.4: unpatched upstream 0.13.0 cannot compile the existing
  query-notification API. See the SQL Server reference for exact errors (#516).
- Record dated advisory exceptions, release/changelog policy, generator split
  and security property-testing scope (#504, #505, #513).

## Unreleased - ORM/macros 0.172.0, operation catalogue 0.8.1

Client-facing metadata, all opt-in. Without the new options, generated SDL, operation catalogues, semantic catalogues and fingerprints are unchanged. See [client-facing schema metadata](docs/reference/graphql-orm/client-metadata.md) and the [migration guide](MIGRATION.md#ormmacros-01720-and-operation-catalogue-081-client-facing-metadata).

- **Client metadata export ([#340](https://github.com/Dastari/graphql-orm/issues/340), [#670](https://github.com/Dastari/graphql-orm/issues/670), [#672](https://github.com/Dastari/graphql-orm/issues/672)).** `schema_roots!(client_metadata: true)` emits `graphql_orm_client_metadata()`, a `GraphqlClientMetadata` document bound to `graphql_orm_semantic_catalog()` by its fingerprint and exported at build time with `export_json` (canonical JSON). Per entity it lists the fields the generated `Changed` stream can carry (computed by the same rules as the subscribe-time selection check), the declared lifetime field and its encoding, and each public relationship's key mapping: `batchable` with ordered source and target public field names and whether the target filter accepts `Equal` and `In`, `not_batchable` for relationships with a source or target condition or a field policy, or `no_exposed_key` when a key field is not public, is policy-guarded or is classified `secret` (`sensitive`). Conditions and column names are never published. Router-served delivery is later.
- **Lifetime fields ([#670](https://github.com/Dastari/graphql-orm/issues/670)).** `#[graphql_entity(lifetime(field = "...", format = "rfc3339_utc" | "unix_seconds" | "unix_milliseconds" | "sqlite_utc_text"))]` names the field holding a record's UTC expiry time; `DateTime<Utc>` defaults to `rfc3339_utc`, and integer and `String` fields must state their format. The derive refuses private, policy-guarded, secret, list and non-time fields. `Entity::graphql_lifetime()` returns it. Expiry is not enforced.
- **Started signal on `Changed` streams ([#341](https://github.com/Dastari/graphql-orm/issues/341)).** `#[graphql_entity(changed_started = true)]` adds `Started: Boolean` to the entity's `Changed` root. With `Started: true` the subscriber first receives one frame with `Started: true` and every other field null, sent once the stream is admitted, authorized and listening: every change committed after it is delivered. The event type keeps its name, gains `Started: Boolean!` and makes `Action`, `ChangeKind`, `Id` and `Key` nullable. No new error code.
- **Aggregate source entity ([#372](https://github.com/Dastari/graphql-orm/issues/372)).** `#[graphql_entity(aggregate = true, aggregate_source = true)]` makes the aggregate root return `[<Entity>AggregateResultRow!]!`, whose always-null `SourceEntity: <Entity>` field names the source entity in the served schema, through introspection and composition.
- **Interpretation fingerprint ([#671](https://github.com/Dastari/graphql-orm/issues/671)).** Every `schema_roots!` emits `graphql_orm_interpretation_fingerprint(salt)`: SHA-256 over the cursor-format version, the backend dialect, and for each entity (sorted by name) its name, whether it serves `NodeId`, the exact keyset order fingerprint its cursors are bound to, its effective default sort and its primary-key fields, plus the digest of an optional host salt. It is stable across restarts, replicas and reorderings of `entities:`, and changes when any input changes. `GRAPHQL_ORM_INTERPRETATION_HEADER` and `GRAPHQL_ORM_INTERPRETATION_ACK_FIELD` name where a host serves it; router composition is later.
- **Multi-page reads ([#673](https://github.com/Dastari/graphql-orm/issues/673)), docs only.** The pagination reference states that every page is an independent read and why the ORM offers no snapshot token.
- **Design plan for `Changed` replay ([#668](https://github.com/Dastari/graphql-orm/issues/668)).** [docs/plans/active/changed-stream-replay](docs/plans/active/changed-stream-replay/README.md); no implementation.
- Operation catalogue 0.8.1 adds the `GraphqlClientMetadata` types, `relationship_key_field_exposed` and `relationship_target_filterable`. Additive; no existing type, wire format or fingerprint changes.

## Unreleased - ORM/macros 0.171.0

An opt-in client error contract for generated roots, with one code vocabulary shared with the router. Nothing changes for a host that does not opt in: generated SDL, responses, operation catalogues and fingerprints are unchanged. See the [migration guide](MIGRATION.md#01700-to-01710-opt-in-client-error-contract) and the [error-code reference](docs/reference/graphql-orm/error-codes.md#one-vocabulary-with-the-router).

- **A dedicated code for an unsupported subscription selection ([#342](https://github.com/Dastari/graphql-orm/issues/342)).** With `DatabaseBuilder::subscription_selection_unsupported_code(true)` or `Database::with_subscription_selection_unsupported_code(true)`, a generated `Changed` subscription whose selection can never be streamed (a relation, a computed field or an unavailable key) is refused with `extensions.code` `SUBSCRIPTION_SELECTION_UNSUPPORTED` (`graphql_orm::graphql::subscriptions::SUBSCRIPTION_SELECTION_UNSUPPORTED`) instead of `INVALID_INPUT`, with the same message. Other invalid stream input, such as a `Keys` list over its bound, stays `INVALID_INPUT`.
- **A mutation's error says whether the write took effect ([#371](https://github.com/Dastari/graphql-orm/issues/371)).** With `DatabaseBuilder::mutation_error_outcome(true)` or `Database::with_mutation_error_outcome(true)`, every error from a generated create, upsert, update, delete, `updateMany` or `deleteMany` root carries `extensions.outcome`: `refused` (refused before the write was attempted, for example by a policy or a before hook), `failed` (attempted and rolled back, for example after a failed statement or an after hook's refusal), `committed` (for example a `notify` handler failing after the commit) or `unknown` (the commit itself failed). The ORM sets the value itself and replaces any `outcome` a callback put on its error. Under the setting, a bulk write whose commit fails raises a top-level `unknown` error instead of returning `success: false`, so `success: false` always means nothing was written. The new public `graphql_orm::graphql::errors::MutationOutcome` names the vocabulary, which runtime GraphQL mutations already use for `committed` and `unknown`.
- **A stable code on a generated mutation result's `Error` ([#385](https://github.com/Dastari/graphql-orm/issues/385)).** An entity declared with `#[graphql_entity(mutation_error_codes)]` gains a nullable `errorCode` on its single-row `…Result`, beside `error`: `NOT_FOUND` for `Entity not found` and `Entity not found after update`, `INVALID_INPUT` for `No fields to update`, and the error's own code, such as `CONSTRAINT_VIOLATION`, for a failed write statement. The bulk `Where filter is required…` refusals get `INVALID_INPUT` in their existing `errorCode`. The codes reuse `OrmErrorCode` values; no new code is added. A `success: false` from a single-row root means nothing was written.
- New tests run these on SQLite and owned PostgreSQL: each code and the outcome appear with the opt-in and not without it, including `failed` with a rollback after an after hook's refusal and `unknown` for a commit refused by a deferred foreign key.

## Unreleased - ORM/macros 0.170.0

- **Generated static schemas refuse a value that is not an object where an input object is expected ([#697](https://github.com/Dastari/graphql-orm/issues/697)).** async-graphql 7.2.1 accepts a scalar, list or enum for an input-object argument while it validates a request, and leaves an omitted variable's default unchecked until execution. Such a value used to fail while its field resolved, with an uncoded coercion message and partial data, after earlier fields of the same operation (mutations included) had already run. Generated `schema_builder` helpers now install `graphql_orm::graphql::input_validation::GeneratedInputValidation`, which checks every argument of the selected operation with the values execution resolves (literals, variables, variable defaults and fragments, at any depth) and refuses the whole request before any resolver, policy callback or SQL with message and `extensions.code` `invalid_graphql_input`, as runtime GraphQL schemas do. Each variable reference must stand where the GraphQL rule "All Variable Usages Are Allowed" admits its declared type (nullability, defaults and list wrapping; a single value may still stand for a one-element list), which async-graphql 7.2.1 never applies in either validation mode. The check's work bound is sized from the parsed document, so a request supplied already parsed is checked without a false refusal. In a oneof input object a variable standing for the member must be declared non-null and resolve to a value. The runtime guard and the generated schemas share one checker, so runtime GraphQL schemas now also refuse a variable at a position the rule does not allow; that used to run whenever the value happened to coerce, and this tolerance is removed. Field arguments are checked; directive arguments are left to async-graphql. Spec-valid requests are unchanged. A schema built by hand from generated roots does not get the check unless it installs the extension.
- **Bulk-write filter authority is pinned on SQLite and PostgreSQL ([#602](https://github.com/Dastari/graphql-orm/issues/602)).** Generated `updateMany` and `deleteMany` have authorized every filter field with its read policy since 0.129.0 (#649). New tests run on SQLite and owned PostgreSQL and cover `and`, `or` and `not` at any depth. They show that a denial is refused in the payload with the list query's message and code before any target is selected or asked about, that it writes nothing, and that related-row bulk filters are refused alike whatever the caller may read. The reference now states this. It also states that the repository bulk helpers run host-built filters without field read checks.
- See the [migration guide](MIGRATION.md#01690-to-01700-input-object-values-on-static-schemas-and-bulk-write-filter-authority).

## Unreleased - ORM/macros 0.169.0

SQL Server correctness follow-ups and an opt-in client offset bound. Generated SDL, operation catalogues and fingerprints are unchanged. See the [migration guide](MIGRATION.md#01670-to-01690-sql-server-correctness-and-a-client-offset-bound).

- **A `DECIMAL(38,38)` value is a decode error, not a panic (#452).** The SQL Server driver asserts while it reads a non-NULL value in a column declared with scale 38, before the ORM sees the value. Every pooled, restore and transaction statement, and a notification registration, now catches that panic and fails with `sqlx::Error::Decode`; the connection is closed, a transaction fails, and the pool keeps serving. The process's panic hook still prints the assertion, and a binary built with `panic = "abort"` still aborts. Values beyond `rust_decimal` in other scales were already decode errors (0.151.0). The guard covers the driver's decoding of a response, not the ORM's later conversion of a decoded row. A caller-built `Config` at `EncryptionLevel::On` with the login-only opt-in, which makes the driver panic during prelogin against a server that declines encryption, is now refused with `sqlx::Error::Configuration` before any connection is opened.
- **Opt-in client offset bound on every backend (#456).** `DatabaseBuilder::max_page_offset(Some(n))` or `Database::with_max_page_offset(Some(n))` refuses a client page offset above `n` with `PAGE_LIMIT_EXCEEDED`, the code of a refused page limit, with the message `requested offset N exceeds the maximum page offset M`. It covers the pages the limit refusal covers: generated list roots, to-many relation pages and search pages, custom resolvers through `ClientPaginationConfig::resolve_client_page`, and Rust reads that take a `PageInput` (`EntityQuery::paginate`, `EntitySearchQuery::paginate`). The new `ClientPaginationConfig::admit_client_offset`, `admit_client_page`, `with_max_offset` and `max_offset` are public. The default is unbounded, as before; `PaginationConfig::RECOMMENDED_MAX_OFFSET` (10,000) documents a recommended maximum. An offset that trusted code sets with a builder's `offset(n)`, including `search_db(...).offset(n)` and `EntitySearchQuery::offset`, is not bounded. `limit: 0` already renders valid T-SQL (`SELECT TOP (0)`, since 0.151.0).
- **Fix: the 0.164.0 oversized-limit refusal now covers generated GraphQL search on a native index.** With `refuse_oversized_page_limit(true)`, a generated `…Search` page whose limit is above the maximum was refused only on the scan path (SQL Server, row-policy scans); on the SQLite FTS and PostgreSQL `tsvector` index it was reduced to the maximum. The search now admits the client page, limit and offset, before either path reads a row.
- **SQL Server routing redirect, tested on the wire (#518).** A test drives the pool through a fake Azure-style gateway that logs the client in and redirects it, and through a second redirect. The redirect itself shipped in 0.151.0 and named instances in 0.153.0. A second redirect is now refused with a message that says only one is followed, instead of the driver's text.
- **SQL Server introspection and rendered DDL (#524).** `introspect_mssql_schema` reads ordinary and unique indexes (not those backing a primary key or unique constraint), with their column order, descending columns and `IN`-list filters, so an applied schema with indexes plans no change. A filter the model cannot express never compares equal to a declared index. A table without a primary key reports none instead of an invented `[id]`. On SQL Server a `String` field that is a primary key, unique, a member of a declared index or composite unique, or a foreign-key source maps to `NVARCHAR(450)`, which SQL Server can index, instead of `NVARCHAR(MAX)`; the fallback for other Rust types is `NVARCHAR(MAX)` instead of the deprecated `TEXT`. Search metadata statements are T-SQL (`IF OBJECT_ID(...) IS NULL CREATE TABLE`, `UPDATE` then `INSERT`) instead of `CREATE TABLE IF NOT EXISTS` and `INSERT OR REPLACE`, and the SQL Server arms of the group-page and spatial renderers render T-SQL.
- **`ALTER COLUMN` keeps a SQL Server column's collation (#742).** Rendered SQL Server `ALTER COLUMN` to a character type reads the column's live collation when it runs and adds `COLLATE <collation>` when it differs from the database default, after checking the name against `sys.fn_helpcollations()`; without it SQL Server resets the column to the default, which can change a non-Unicode column's code page. Other target types render as before.
- **Deterministic SQL Server pool test (#734).** `pool_bounds_acquire_and_statements_and_resets_idle_sessions` no longer opens connections inside the 500 ms acquire bound it tests, and its busy and timed-out statements wait on application locks the test holds instead of a fixed 200 ms head start and `WAITFOR DELAY`. Test-only.
- **Restore marker follow-ups (#744).** A statement refused because a restore holds the lock stays `RestoreInProgress` (with an empty identifier) when the follow-up marker read fails, instead of `RestoreStateUnavailable`, whose message said no restore was running. The session reset also releases the marker install lock. `install_restore_marker` refuses a hand-made table whose primary key is not exactly `marker_key` or that lacks an enabled, trusted `marker_key = 1` check (column-level or table-level; not added `WITH NOCHECK`). The `resume_restore` documentation lists `RestoreStateUnavailable`.

## Unreleased - ORM/macros 0.167.0

- Test-only: the macros unit tests for update assignments and write-transform rejection name no backend, so `cargo test -p graphql-orm-macros --no-default-features --features postgres` and `--features mssql` pass as well as the default SQLite run (#735). Generated code and runtime behavior do not change; the version moves because the macros package archive carries its unit tests. See the [migration guide](MIGRATION.md#ormmacros-01670-test-only-backend-agnostic-macros-unit-tests).

## Unreleased - ORM/macros 0.166.0

- Add request-local `DependentDeleteGuard`: compare admitted effects with the
  introspected dependent-delete graph and enforce row bounds inside the write
  transaction. Unknown and unsupported effect paths refuse. No default change.

## Unreleased - AI 0.146.0, tool profiles 0.24.0

- AI capability scale (part of #551): reviewed static read tools can sit behind the fixed broker instead of the provider surface, fixed-broker `describe` defines each repeated input type once, capability search no longer re-tokenises the catalogue, one oversized generated root no longer fails its target, large surfaces can opt into the fixed broker over native deferred loading, and a startup benchmark is published from CI. Also adds an opt-in host-held epoch anchor that keeps a file-level database restore closed (#572), an opt-in dependent-effect policy for generated deletes (#389), and opt-in strict query roots (#463). No schema change, and no fingerprint changes for a host that changes nothing. Opting into the delivery preference requires rebinding; see the [AI changelog](crates/graphql-orm-ai/CHANGELOG.md#unreleased---01460) and [migration guidance](MIGRATION.md#ai-01450-to-01460-and-tool-profiles-0230-to-0240-capability-scale).

## Unreleased - ORM/macros 0.165.0, backup 0.26.0

- **A snapshot restores in one transaction on PostgreSQL and SQLite (#574).** `restore_backup_rows` imports every table, runs the row-count and row-hash checks and commits once. A table the database refuses, or a failed check, rolls back the tables imported before it, so a failed restore leaves the target empty and can be retried without cleanup. The hash check used to run after each table's commit. `import_table_rows` still restores one table in its own transaction.
- **A restore lock and crash marker for host-run SQL Server restores (#574).** SQL Server has no ORM restore, so a host's own restore may commit table by table. `Database::begin_restore` returns an `MssqlRestore` guard that holds an application lock exclusively; every ORM transaction and autocommit statement takes the same lock in shared mode, in the batch that begins it. A restore therefore starts only after in-flight ORM work has finished, and every transaction or statement started during it, in any process, fails with an error carrying `RestoreInProgress` before it reads or writes. A marker row keeps statements refused after a restore whose process died, until `Database::resume_restore` adopts it and the restore is finished. The schema owner must install `dbo.__graphql_orm_restore_marker` before runtime use (`MssqlPool::install_restore_marker`) and grant runtime roles SELECT. Missing or unreadable marker state fails closed with `RestoreStateUnavailable` (reason: marker not installed, permission denied, connection failed or other), never `RestoreInProgress`; `install_restore_marker` needs a writable pool and is idempotent and race-safe. PostgreSQL and SQLite keep no lock or marker.
- **`SchemaManager::reset_runtime_search` (#280).** Discards a runtime collection's search documents and marks its index `building` in one transaction, so search is refused with `search_index_not_ready` until `backfill_runtime_search` rebuilds it. A snapshot restore does the same, inside its transaction, for the readiness-tracked index of every restored table. Static search indexes are not rebuilt by a restore; see the runtime search reference.
- **An ignored field-write denial no longer clears the stored value on upsert (#688).** With `FieldPolicy::ignore_denied_field_write`, a GraphQL upsert onto an existing row leaves a denied optional column out of the conflict update, on SQLite, PostgreSQL and SQL Server. It used to write `NULL`. Mutation hooks no longer see the column among the upsert's changes.
- **Backup 0.26.0: host-held snapshot records (#610).** `ManifestAuthenticator::with_known_snapshots` and `with_revoked_snapshots` refuse a snapshot the host's records do not admit (`BackupError::is_snapshot_not_admitted`), so a pruned snapshot that is put back into the repository no longer verifies. `prune_observed` and `delete_snapshot_observed` report each snapshot id to a `SnapshotDeletionObserver` before deleting it, so the host has every deleted id even when the call fails. No existing backup type changed. See the [backup changelog](crates/graphql-orm-backup/CHANGELOG.md).
- See the [migration guide](MIGRATION.md#01640-to-01650-atomic-restore-search-reset-and-kept-values-on-ignored-denials).

## Unreleased - ORM/macros 0.164.0

- ORM and macros 0.164.0: opt-in contract settings for page limits and 64-bit identities, a stable code for cursor keys that are too long, and deferred subscription lag. Defaults are unchanged except for the timing of a lag end. See the [migration guide](MIGRATION.md#01630-to-01640-page-limits-64-bit-identities-cursor-keys-and-subscription-lag).
  - **Added (opt-in):** Oversized page refusal, set with `DatabaseBuilder::refuse_oversized_page_limit(true)` or `Database::with_oversized_page_limit_refusal(true)`. A generated GraphQL page whose explicit limit is above the maximum page limit (list roots, to-many relations, search connections and keyset connections) then fails with `PAGE_LIMIT_EXCEEDED`, naming the limit asked for and the maximum, instead of being reduced without notice. New `ClientPaginationConfig::admit_client_limit` and `resolve_client_page` for custom resolvers. `Database::client_pagination_config()` exposes the client policy separately, preserving existing struct literals ([#667](https://github.com/Dastari/graphql-orm/issues/667)).
  - **Added (opt-in):** `schema_roots!(max_page_limit: N)` declares the maximum page limit at compile time. The generated module exposes `GRAPHQL_ORM_MAX_PAGE_LIMIT`, the semantic catalogue publishes `N` as the ceiling of pageable relationship lists, the relation complexity estimate caps pages at `N`, and the schema builders panic for a `Database` that applies another maximum. Without the declaration the catalogue and the estimate keep the compile-time 100, as before ([#674](https://github.com/Dastari/graphql-orm/issues/674), #667).
  - **Added:** `EntityQuery::with_max_limit(n)` gives one trusted keyset or authorized-scan read (`fetch_authorized_keyset*`, `fetch_authorized_scan`) its own maximum, as the repository builders' `with_max_limit` has done since 0.128.0. The page-size documentation now says which host-side reads are exact or refused and which keep GraphQL page semantics (#667).
  - **Added (opt-in):** `RuntimeGraphqlOptions::with_int64_identity_strings()` types the runtime `RecordVersion` field and integer live keys (`…Changed` `keys`, `…KeyInput` members, the event `id` and `key`) as `RuntimeInt64` and sends them as decimal strings, so a value above 2^53 reaches a JSON client intact. Off by default because it changes the SDL and fingerprints ([#528](https://github.com/Dastari/graphql-orm/issues/528), item 4).
  - **Behaviour change (fix):** a runtime page whose row has order values too long for a cursor is refused with the new stable code `cursor_key_too_large` (the existing `ResourceLimit` category with `detail()` set to `Some("cursor_key_too_large")`). Through runtime GraphQL such a page used to fail with `invalid_request` (above `max_cursor_bytes`) or `invalid_cursor` (above the cursor protection bounds, about 8 KiB of sort key with the defaults); the host read APIs returned `resource_limit`. Both enums retain their existing variants ([#580](https://github.com/Dastari/graphql-orm/issues/580)).
  - **Behaviour change:** a generated `Changed` subscription or runtime live subscription that lagged no longer ends when the lag is detected. It delivers nothing more and ends on its next jittered authority tick: with `FORBIDDEN` (generated) or `authorization_denied` (runtime) when the subscriber's authority is gone, otherwise with `SUBSCRIPTION_CONTINUITY_LOST`. A revoked subscriber no longer receives a continuity-loss frame, and writes it cannot see no longer choose when its stream ends. The end arrives up to one recheck period later than before ([#387](https://github.com/Dastari/graphql-orm/issues/387), option B).
  - **Docs:** a static keyset cursor (`gomk1.`) is an integrity envelope with a checksum that uses no secret, not a tamper-evident token. A rewritten cursor with a recomputed checksum is accepted and only moves the resume position; row policy applies to every page. The documentation and the `CURSOR_INVALID` description no longer claim tamper rejection, and a test pins the behaviour ([#448](https://github.com/Dastari/graphql-orm/issues/448)).
  - **Docs:** PostgreSQL JSON numbers of 2^64 or more, or with more than about 17 significant digits, come back through `serde_json` as the nearest `f64` and can differ in the last place. `float_roundtrip` is not enabled: it is a build-wide feature that roughly doubles float parsing cost ([#581](https://github.com/Dastari/graphql-orm/issues/581)).
  - **Correction (router):** the 0.18.0 entry below says the router restores the composed `authenticated` and `requiresScopes` SECURITY links "required by Hive enforcement". The links are restored, but the router does not enforce SDL `@authenticated` or `@requiresScopes` on nested fields. It authorizes root fields from protocol metadata before execution; a nested field is authorized only by the owning subgraph's resolver guards and policies, as [the federation reference](docs/reference/graphql-orm/federation.md#the-router-does-not-authorize-a-joined-field) states. Enforcement is tracked in [#729](https://github.com/Dastari/graphql-orm/issues/729) ([#485](https://github.com/Dastari/graphql-orm/issues/485)).
  - **Not changed:** the PostgreSQL search index plan under row-level security ([#555](https://github.com/Dastari/graphql-orm/issues/555)) is left as it is; none of its three designs is a small change.

## Unreleased - ORM/macros 0.163.0

- Lower the compile cost of `GraphQLOperations` (part of [#677](https://github.com/Dastari/graphql-orm/issues/677)). The seven per-entity wrapper objects (`…Edge`, `…Connection`, `…Result`, `…ChangedEvent`, `Delete…Result`, `Update…Result` and `…AuthorizedScanPage`) no longer derive `async_graphql::SimpleObject`. The macros emit their `OutputType`, `ContainerType` and `ObjectType` impls directly over shared helpers in the hidden `graphql_orm::graphql::wrapper_object` module, so the per-field resolver glue is type-checked once in the runtime crate instead of once per entity in the host crate. Generated SDL, operation and semantic catalogues and fingerprints are byte-identical; the structs, their fields and the derive's public `async fn` field getters are unchanged; no authorization or visibility check moved. No action; see the [migration guide](MIGRATION.md#01590-to-01630-wrapper-object-compile-cost).

## Unreleased - ORM/macros 0.162.0

- Add context-bound authorized pages for handwritten GraphQL resolvers and query builders ([#169](https://github.com/Dastari/graphql-orm/issues/169)). `EntityQuery::fetch_authorized_page` runs the bounded, re-authorized scan of `fetch_authorized_scan` and seals its continuation with a service-owned key under a server-authored `AuthorizedPageBinding` that the resolver supplies on every request. A continuation issued under a different binding, entity, order, filter or key, or one that was altered, truncated, is oversized or has expired, is refused with `CURSOR_INVALID` before any row is read. Entity, row and field authorization are resolved again on every page, and `PAGE_FULL`, `BUDGET_EXHAUSTED` and `EXHAUSTED` keep their meanings. Additive: generated pagination, `fetch_authorized_scan` and their cursors are unchanged. No action is needed; see the [migration guide](MIGRATION.md#01620-context-bound-authorized-pages) and the [reference](docs/reference/graphql-orm/pagination-migration.md#context-bound-authorized-pages-for-handwritten-resolvers-01620).
  - New public API: `AuthorizedPageBinding`, `AuthorizedPageBindingBuilder`, `AuthorizedPageBindingError`, `AuthorizedPageKeys`, `AuthorizedPageConfig`, `AuthorizedPageConfigError`, `AuthorizedPageRequest`, `Database::with_authorized_page_config`, `Database::authorized_page_config` and the `AUTHORIZED_PAGE_*` bounds and reason constants.
  - The codec is version 1 (`gomp1`), XChaCha20-Poly1305 over the existing `chacha20poly1305` dependency, with key identifiers for rotation and a per-cursor lifetime (1 hour by default). Bounds cover cursor bytes, binding bytes, page size and scanned rows; a `limit` above the page bound is refused with `PAGE_LIMIT_EXCEEDED`.
  - `CURSOR_INVALID` gains the optional `reason` values `AUTHORIZED_PAGE_CURSOR_EXPIRED` and `AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE` for this API only.

## Unreleased - ORM/macros 0.159.0

- Fix PostgreSQL static entities with bare mixed-case table names (#716). Render quoted table references with the historical PostgreSQL folding of bare table components, and resolve declared names during introspection. Create/alter/index/trigger/RLS statements, reads and writes, catalog checks and search helpers address the same table while metadata and helper digests remain unchanged. Existing folded installs need no migration; lower-case names are unchanged. See [migration guidance](MIGRATION.md#01580-to-01590-postgresql-table-name-case).

- Planning preserves current-schema isolation for bare paths and distinguishes
  quoted tables from their folded counterparts. Duplicate physical table
  declarations return `TableIdentifierConflict` before DDL, with or without
  search. Dependent deletes use resolved catalog identities. The optional
  backup adapter 0.23.0 fixes mixed-case target clearing and restore preflight;
  see its [changelog](crates/graphql-orm-backup/CHANGELOG.md).

## Unreleased - AI 0.145.0

- AI adds per-message structured entity references. Only the reference handle is stored; the provider-call executor resolves it for the current principal at every dispatch. **Breaking** for custom `AiProvider` implementations (the dispatch methods take `ModelDispatchRequest`) and for hosts that build `SendAiMessageInput` or `SendAiMessagePayload` literals; apply AI schema module `0.81.0`. Details are in the [AI changelog](crates/graphql-orm-ai/CHANGELOG.md#unreleased---01450).

## Unreleased - ORM/macros 0.158.0

- ORM and macros 0.158.0 (placeholder): runtime response budget, relation statements, search helper names and request-path panics. One change is breaking for hosts whose searchable base table name is longer than 42 bytes or ends in `_` and 16 hexadecimal digits. See the [migration guide](MIGRATION.md#01570-to-01580-runtime-budget-search-helper-names-and-typed-refusals).
  - **Fix:** the runtime GraphQL response budget reserves the whole response structure before any projected host object is copied. A read first reads every selected layer, parents before children, and reserves each layer's connection objects, edges, cursors, page fields, relation keys, to-one nulls and ordinary stored fields; only then are projections resolved, parents first. A mutation payload reads and reserves its relation layers before the committed record's own projections are copied. A response one byte over its budget, with relations below projected parents, now copies no projected object; it copied one before. A projector is therefore called after the layers below its records are read, and is not called when those layers alone exceed the budget ([#704](https://github.com/Dastari/graphql-orm/issues/704)).
  - **Fix:** the runtime read path no longer deep-copies records before their bytes are reserved. The GraphQL executor borrows the records it read, and the read layers move a decoded record into its result instead of copying its values. The one copy left is of a related record, or a page of them, that several parents of one batch share, because each parent's result owns its records. Every `RuntimeRecord::clone` emits a `TRACE` event with target `graphql_orm::runtime_record` (#704).
  - **Behaviour change (fix), SQLite:** a runtime relation layer that binds more than 999 values is sent as several statements of at most 999 bound values each, in the same order and with the same results, where it used to send one statement that SQLite builds with the older 999-variable limit refuse. One parent group that alone needs more than 999 (its filter and cursor values leave no room for the parent key) cannot be split and is refused before any statement runs with `resource_limit`, whose source is the `INVALID_INPUT` refusal the static relation loaders give the same condition; it used to be sent and failed with `backend_execution` on such builds. Every statement's rendered bind count is checked before it is sent. `RuntimeRelationLimits::max_bind_parameters` still bounds the whole layer and its default is unchanged; PostgreSQL applies the same rules at 32,000 bound values ([#528](https://github.com/Dastari/graphql-orm/issues/528)).
  - **Breaking (fix), unusual table names only:** `search_table_name` returns at most 63 bytes, PostgreSQL's identifier limit, and no two base tables whose sanitized names differ share a result. A base table name of up to 42 bytes keeps `__graphql_orm_search_<table>`, unless it ends in `_` and 16 lowercase hexadecimal digits. Every other base table gets a digest name: up to 46 bytes of the plain name, then `_` and 16 hexadecimal digits of the SHA-256 of the whole base table name. PostgreSQL truncated the longer names, so two searchable tables that agree in their first 42 bytes shared one helper name and the second index could not be created. An existing PostgreSQL or SQLite fallback search table of a base table that now gets a digest name must be renamed once (#528).
  - **Fix:** planning or applying a migration refuses, before any statement runs, a search index whose helper table name is already taken: by another base table of the same plan (for example `audit.logs` and `audit_logs`, whose sanitized names are equal), or by a table that exists in the database without a search metadata row for that base table. A whole target is checked as well as the plan's steps, so two entities that share a helper name are refused even when only one is being created. Names are compared as the backend compares the identifiers the ORM writes: without regard to ASCII case on SQLite, and on PostgreSQL as unquoted identifiers, which fold to lower case. Static plans and applies return the new `SearchHelperTableConflict` (read with `SearchHelperTableConflict::from_error`); runtime (owned) migrations report `TargetCollision` with the helper table as the subject. It used to fail in the middle of the migration with the database's own "already exists" error (#528).
  - **Fix:** a poisoned request error buffer no longer panics generated `Changed` subscriptions, the post-commit mutation output check or the response that follows. The buffer is emptied, so no private diagnostic outlives the failure, and its poison is cleared, because async-graphql locks the same buffer when it builds the response. The subscription ends with one `INTERNAL_ERROR`; the mutation reports `INTERNAL_ERROR` with `outcome: committed`. New hidden helpers `graphql::errors::buffered_errors`. An aggregate operator that this version cannot render, from a newer operation catalogue, is refused when the metric is added instead of reaching an `unreachable!` while SQL is rendered ([#511](https://github.com/Dastari/graphql-orm/issues/511)).
  - **Fix:** the runtime GraphQL planner refuses an `orderBy` term on an unsortable or JSON field itself, with `unsupported_order`, before `runtime_order` checks it again ([#541](https://github.com/Dastari/graphql-orm/issues/541), in part).
  - **Tests and hygiene:** `cargo clippy -p graphql-orm --no-default-features --features sqlite,postgres,runtime-graphql --all-targets -- -D warnings` is clean: a change-journal helper is compiled only with its caller, and twelve single-backend test targets and one example no longer compile for two backends at once. `runtime_search_long_tables` runs in the SQLite and PostgreSQL runtime lanes and in CI; `runtime_relation_bind_limit`, on a connection limited to 999 variables, and `poisoned_error_buffer` run in the default suite, the former also in the SQLite lane.

## Unreleased - AI 0.144.0

- AI adds a pinned Anthropic model-snapshot mapping and an exact OpenAI stored-response deletion adapter, fixes a missed cancellation wake-up and bounds published skill schemas (a hardening that can reject a schema accepted before). No schema module change. Details are in the [AI changelog](crates/graphql-orm-ai/CHANGELOG.md#unreleased---01440).

## Unreleased - ORM/macros 0.157.0

- ORM and macros 0.157.0 (placeholder): **breaking/data safety:** migrations, the change journal and retention. See the [migration guide](MIGRATION.md#01560-to-01570-migrations-change-journal-and-retention).
  - **Breaking (fix, data loss):** `Database::export_changes` pages the change journal by a new monotonic `seq` column instead of comparing random UUID ids, which skipped some entries and repeated others. `ChangeWindow::after_snapshot_id` is replaced by `after_sequence: Option<i64>`, and `BackupChange` gains `sequence: i64`; struct literals of either type must change, and a position saved from an earlier export cannot be reused. Consecutive exports that continue from the last returned `sequence` return every entry exactly once, in the order written. `until` ends the returned run at the first later entry instead of leaving that entry out. On PostgreSQL the export waits for journaled write transactions that were open when it started (up to `CHANGE_EXPORT_DEFAULT_WAIT`, 30 seconds; `export_changes_with_wait` takes another limit) and fails with `ChangeExportTimedOut`, exporting nothing, when they outlast the wait, and refuses to run on a standby. A journal created by an earlier release is refused until its owner runs `ensure_change_journal_recorder`, `ensure_change_journal_table`, or the schema target, which numbers the existing entries. The SQLite upgrade is one transaction that a failure or cancellation rolls back, and it refuses with `ChangeJournalUpgradeConflict`, changing nothing, when an object named `__graphql_orm_change_log_sequenced` already exists (#591).
  - **Fix:** a PostgreSQL model may declare any alias of a built-in type (`varchar(50)`, `int`, `int8`, `bool`, `decimal(10,2)`, `timestamptz`, `float8`, and so on). The planner and the foreign-key member check treat it as the type name PostgreSQL reports, so a freshly applied schema plans no change instead of a `Risky` `AlterColumn` per aliased column and a dropped and re-added foreign key per aliased pair (#561).
  - **Fix:** a PostgreSQL plan keeps a foreign key attached when both of its members only widen within one base type (a longer `varchar` or `varbit`, or a `numeric` with more precision at the same scale). It used to drop and re-add the key, which rechecks every referencing row (#566).
  - **Fix:** planning and validating a PostgreSQL target reads each schema-qualified entity table (`table = "audit.logs"`) wherever it lives, so a second plan of the same target no longer creates the table again and fails with `42P07`. `IntrospectionBackend` gains the provided method `introspect_schema_including`. Tables of other schemas that the target does not name are still never read, and a schema-qualified table removed from the target is not dropped (#621).
  - **Fix:** a SQLite plan that removes a child's foreign key and then the parent's composite UNIQUE key it referenced applies, keeping every row, instead of being refused with `UnsupportedCapability`. The parent rebuild rechecks the catalog inside the migration transaction. A parent rebuild scheduled first, and a key that another table still references, are still refused before any change (#547).
  - **Fix (regression since 0.23.0):** on PostgreSQL, a generated or repository insert into an entity whose row-level security declares no `UPDATE` policy, such as an append-only entity, failed with `NOT_FOUND`, and `RetentionContext::purge` failed with an internal error. The read-back of a created row and the retention selection no longer take `FOR UPDATE`, which PostgreSQL filters by the `UPDATE` policy. `retention_purge_postgres` now runs in the owned PostgreSQL lane and in CI (#554).
  - **Tests:** `runtime_many_to_many` asserted the risk of the first step for the join table, which on PostgreSQL is the `Risky` foreign-key drop that precedes the `Destructive` table drop. It now asserts the table's highest risk and runs in the owned PostgreSQL lane and in CI; a planner test pins the `Destructive` classification of a table drop on SQLite, PostgreSQL and SQL Server (#598).
## Unreleased - ORM/macros 0.156.0

- ORM and macros 0.156.0 (placeholder): **behaviour changes:** query, filter, pagination and relation loading. See the [migration guide](MIGRATION.md#01550-to-01560-queries-filters-and-relation-loading).
  - **Behaviour change (fix):** a batched nested relation without `orderBy` returns each parent's rows in the target's declared `default_sort`, with the primary key as the final tie-break, as the direct relation path and root lists already did. It used to return them in primary-key order, so the same relation could order differently depending on whether it was batched. An explicit `orderBy` still overrides the default. Pages, counts and visibility predicates are unchanged (#245).
  - **Breaking for clients (fix):** static `String`, `Int`, `UUID` and `Bytes` filters accept at most 1,000 operands in one `inList` or `notIn` list (`MAX_FILTER_LIST_OPERANDS`), and a `contains`, `startsWith`, `endsWith` or `similar` operand of at most 1,000 UTF-16 code units (`MAX_FILTER_PATTERN_LENGTH`). A longer list or pattern is refused with `INVALID_INPUT` before any SQL runs, for GraphQL input and for filters built in Rust (new `validate()` on each filter). Lists were unbounded and could exceed SQL Server's 2,100 parameters; a long pattern exceeded its 8,000-byte `LIKE` limit. `Int64Filter` keeps its limit of 256 (#455).
  - **Fix:** one failing key group in a relation or Federation key batch no longer fails every other key in the same `DataLoader` call. The loaders registered by `schema_builder` are shared by concurrent requests; generated resolvers now load `IsolatedLoad(key)`, whose value is a per-key `Result`, so a statement that fails for one group (one relation, filter, order, page and database auth context) fails only the keys it carried. The existing `Loader` implementations for the plain keys keep their all-or-nothing result (#444).
  - **Feature:** a relation target no longer needs a hand-written `BatchLoadEntity` implementation. `RelationLoader`'s relation keys require only the new blanket-implemented `RelationTarget`; `BatchLoadEntity` remains for the single-column `Loader<String>` form. Existing implementations still compile and are simply unused by generated relations (#285).
  - **Behaviour change (fix):** counting a SQL fragment's placeholders and rebasing them read the same tokens. A placeholder is `?`, `$<digits>` or `@p<digits>`; text inside string literals, quoted identifiers, comments and PostgreSQL dollar-quoted strings is skipped by both, and a bare `$` or `@p`, a variable such as `@page`, and `$1` attached to an identifier are no longer rewritten as placeholders. New `count_placeholders_for(backend, sql)`; `count_placeholders(sql)` uses the default backend. A search filter clause with more placeholders than values reaches the database as written instead of panicking (#458).
  - **Fix:** full-text search finds entities with a binary (`Vec<u8>`) primary key on SQLite and PostgreSQL. The search join renders the key as its document stores it (`bin:<lowercase hex>`) (#560).
  - **Fix:** a projection query binds the values of a prepared relation-count order instead of rendering its placeholders with no values. `order_aggregate_visibility` runs on PostgreSQL and SQL Server in CI, with a bound-filter case, a registered-`DataLoader` case and a `Prefilter` target case (#563).
  - **Fix:** SQL Server named instances given without a port (`server=host\INSTANCE`) are resolved through SQL Server Browser (UDP 1434). A connection string that names a port connects to it directly (#518).
  - **Behaviour change (fix):** `render_keyset_after` returns one parenthesized predicate, like `render_keyset_before`. Relation loader keys compare their structured filter, sorts and page as well as their signature strings, and keys that share signatures but not structure are run as separate statements. `Database`'s `Debug` output reports whether entity and row policies are installed. The three backend row-helper modules share one implementation (#541, in part).

## Unreleased - ORM/macros 0.152.0, AI 0.143.0

- Clarification of the AI 0.131.0 stored-reference refusal: claims can create and fail an internal attempt/lease in the same transaction. No lease reaches a worker, and no provider/tool dispatch or host callback is authorized by the refused reference (#608).

- ORM adds `RlsIntrospectionBackend::probe_row_security`: PostgreSQL resolves every supplied table path and probes the current role in one read-only catalog query with catalog-qualified functions and operators, resisting search-path operator shadowing. Unknown/missing/unsupported relations return `None`; consumers must fail closed on errors or incomplete results. SQLite returns inactive security; other backends default to unknown. AI recovery uses this contract (#687). Macros remain version-aligned without generated-code changes.
- AI runtime/provider fixes are detailed in the [AI changelog](crates/graphql-orm-ai/CHANGELOG.md#unreleased---01430).

## Unreleased

- ORM and macros 0.155.0 (placeholder): **breaking/security:** the runtime GraphQL request guard and async-graphql agree on every request, and runtime GraphQL fails closed for PostgreSQL row-level security. See the [migration guide](MIGRATION.md#01520-to-01550-runtime-graphql-guard-and-row-level-security).
  - **Breaking (security):** a runtime query, `node` lookup, mutation or live subscription registration whose module includes a PostgreSQL table with row-level security enabled is refused with `authorization_misconfigured` when the request carries no `DbAuthContext`, before any host authority call or database work. A live subscription registered without context is checked again in each read transaction of an event (which locks the tables it reads `IN ACCESS SHARE MODE` with a bounded lock timeout, checks them, and only then reads) and immediately before each payload is delivered (after release authorization); once RLS is enabled, or a lock times out, it ends with `authorization_misconfigured` and that event delivers nothing. RLS is read live from `pg_class` for the module's collection, link and search tables and, for writes, the tables of verified incoming dependencies. Such requests previously ran without `app.*` settings: silently filtered under a role subject to RLS, unfiltered under the table owner or a bypassing role. `SystemAccess` in the request data and `AuthorizationMode::LegacyPermissive` keep the earlier behaviour ([#559](https://github.com/Dastari/graphql-orm/issues/559)).
  - **Fix:** a runtime query or mutation sent through `Schema::execute_stream`, which the request guard cannot prepare, is always refused without doing any work: every runtime root, `RuntimeHostField` roots included, refuses with the stable code `request_not_prepared` when the schema has a subscription root, and async-graphql refuses the request itself (uncoded) when it has none. ORM roots returned an error naming an internal type, and host roots ran without the assurance pre-pass ([#578](https://github.com/Dastari/graphql-orm/issues/578)).
  - **Fix:** the request guard writes each omitted variable's operation default into the request before validation, so `@skip`/`@include`, argument validation, planning and execution read the same values. With `query($v: Boolean = true)` and no variables, `@skip(if: $v)` now skips (the root returned `authorization_missing`) and `@include(if: $v)` now includes (the root was prepared, read and then dropped). A document replaced by another extension so that a defaulted variable is missing, or an explicit `null` directive condition, is refused with `invalid_graphql_input` ([#582](https://github.com/Dastari/graphql-orm/issues/582)).
  - **Breaking (fix):** every argument of a runtime GraphQL operation, host-field and host-type arguments included, is checked again after validation with the values execution resolves; a runtime scalar outside its input bounds next to an omitted variable, or a value that is not an object for an input object, is refused with `invalid_graphql_input` before any resolver runs. `RuntimeGraphqlComposer::finish` refuses with `invalid_composition` an argument default that is not a valid value of its type, on host fields, types added with `register` and host subscription roots ([#583](https://github.com/Dastari/graphql-orm/issues/583)).
  - **Fix:** a `link_target_visibility` predicate with a relation filter correlates with the link target row; every link mutation on such a relation failed with a database error. A PostgreSQL `timestamptz` holding `infinity`, `-infinity` or a year past 262143 decodes as an `InvalidValue` record error instead of panicking. The response budget charges a projected host object once for each record that carries it, by the same rule as the final response measure (response keys and aliases, worst-case escaping, structure), against what earlier output left (earlier roots and mutation roots, ordinary fields, preceding projections), and refuses before copying it into the response; read layers and mutation payloads keep one running total in those units. That total reserves each layer's structure (connections, edges, cursors, page fields, each parent's relation key and the null of a missing to-one relation) and each record's ordinary fields, measured from the stored values without encoding them, before any of them is encoded or any projected object is copied: a response over the budget is refused with `cost_exceeded` before its ordinary stored values are encoded. Known limits, tracked in [#704](https://github.com/Dastari/graphql-orm/issues/704): records are still cloned before reservation, a child relation's structure is reserved after its parent layer's projections are copied, and host projector output is encoded before it is charged; the response is refused all the same. Documented: `RecordVersion` and integer live-event keys carry 64-bit values as GraphQL `Int`, and an `Unprotected` cursor implies read access to its order's fields (part of [#528](https://github.com/Dastari/graphql-orm/issues/528)).
  - **Internal:** the agql-auth assurance evaluator no longer `expect`s a denial code; a denied state without one is `FORBIDDEN` (part of [#511](https://github.com/Dastari/graphql-orm/issues/511)).

- ORM and macros 0.151.0 (placeholder): **breaking:** SQL Server correctness. See the [migration guide](MIGRATION.md#01490-to-01510-sql-server-correctness).
  - **Breaking (fix):** an error SQL Server reports is `sqlx::Error::Database` holding the new `MssqlServerError` (number, state, class), not `sqlx::Error::Protocol`. Host code that matches on `Protocol` or searches the message for an error number must change. Unique, foreign-key and check violations are reported as `CONSTRAINT_VIOLATION`; retryable errors are matched by number (1205, 1222, 3960, 41301, 41302, 41305, 41325, 41839, 40501, 40197, 40613) instead of by searching the message; I/O and TLS failures keep their class (#451).
  - **Breaking (fix):** SQL Server `datetime2`, `datetime` and `smalldatetime` columns mapped to `String` read as RFC 3339 in UTC (`2026-11-09T12:34:56Z`), not `2026-11-09 12:34:56`. Host code and clients that parse the old text must change. `datetimeoffset` columns, which failed to decode, read with their offset, and a legacy `datetime` fraction reads as the millisecond SQL Server shows (#521, read side only; values are still bound as text).
  - **Fix:** a failed statement no longer hides its error behind a failed rollback. `Database::transaction` and `transaction_with_auth` return the callback's error on every backend and log a rollback failure. A SQL Server transaction keeps its connection after an error the server reported, refuses further statements and the commit, and rolls back normally (#451).
  - **Fix:** a `DECIMAL` value beyond `rust_decimal`'s range is a column decode error instead of a panic. A non-NULL value in a column declared with scale 38 (`DECIMAL(38,38)`) still panics inside the driver while the row is read (#452, in part).
  - **Fix:** `limit: 0` renders `TOP (0)` on SQL Server instead of the rejected `FETCH NEXT 0 ROWS ONLY`. Offsets are still uncapped (#456, in part).
  - **Fix:** `LIKE` patterns escape `[`, which opens a character class on SQL Server (#457).
  - **Fix:** SQL Server aggregate group keys, text `MIN`/`MAX` and relation keys convert to text without loss: floats with 17 digits, dates as ISO 8601, strings as `NVARCHAR(MAX)`. Float relation keys are compared as numbers on every backend (#519).
  - **Fix:** relation, Federation key and single-column batches split into several statements before they exceed the backend's bind limit (2,000 values on SQL Server, 32,000 elsewhere; `SqlDialect::max_statement_binds`). Every bound value of the statement counts, each statement is checked again before it is sent, and a request whose own filter and sort values leave no room for one key fails with `INVALID_INPUT` instead of reaching the server. A relation key repeated within one batch resolves for every occurrence; the repeat used to lose its rows (#520).
  - **Fix:** the rendered SQL Server upsert is atomic outside a transaction, stops at a failed update, and reports the original error when the server has already rolled back the caller's transaction (#523).
  - **Fix:** rendered SQL Server DDL uses `ON DELETE NO ACTION` for `DeletePolicy::Restrict` (new `DeletePolicy::as_sql_for`), scopes each composite-unique drop so a plan composes into one batch, adds columns with `ADD`, and keeps nullability in `ALTER COLUMN`. Introspection reports parameterised types in full, reads foreign keys with their delete action, and no longer splits a table name at `.`; a rendered schema that is applied and read back plans no change (#537, #546, #524 in part: ordinary indexes are still not read).
  - **Fix:** an explicit SQL Server `default_sort` without whitespace quotes each column on its own, including quoted names with doubled delimiters (#430).
  - **Breaking (behaviour):** `MssqlQueryNotificationConnection::receive` delivers at least once and hosts must be idempotent. A message is returned while its receive transaction is still open and is acknowledged by the next `receive` or `register`, or by the new `acknowledge()` and `close()`; it used to be committed before it was returned. An unacknowledged message (dropped connection, crash, failed acknowledgement) is delivered again, a cancelled call cannot lose one, and the connection opens a new session by itself after an error or cancellation. Hosts must acknowledge or close before dropping a connection: unacknowledged drops are rollbacks, and five in a row disable the queue under SQL Server's poison-message handling. New: `acknowledge()`, `close()`, `has_unacknowledged_message()`, `uncertain_acknowledgements()` (#462).
  - **Fix:** SQL Server connections follow one routing redirect. Named instances are still not resolved (#518, in part).
  - **Tests:** the macros `mssql_money` unit test no longer assumes the `sqlite` feature (#626).
- ORM/macros 0.149.0 and catalogue 0.8.0: generated order expressions have optional, closed OrderBy contracts. Entity/catalogue equality compares public metadata and excludes in-memory input contracts, consistently with serialization and fingerprints.

- ORM/macros 0.149.0 and catalogue 0.8.0: **breaking Rust API:** `GraphqlEntitySemanticMetadata` gains `generated_input_members`; struct literals must initialize it. The generated input metadata includes host-only contracts for undisclosed members. Public SDL/catalogue bytes and operation/semantic fingerprints are unchanged. Tool profiles 0.23.0 preserves readable inputs, keeps producer-hidden members closed, and omits capabilities requiring those members with host diagnostics. Generated shape errors identify the input/member. See the [migration guide](MIGRATION.md#01470-to-01490-host-only-generated-input-contracts).
- AI 0.142.0, tool profiles 0.23.0, operation catalogue 0.8.0: **security:** AI tool registration and provider disclosure fail closed. The AI control-plane root set is now derived from the AI crate's own GraphQL roots and applies to static tools, generated capability sets and remote requests; static and remote documents are refused when they select a control-plane field at any depth, introspection, or `_service`/`_entities`. Static tool documents must parse to exactly one operation of the declared kind and contract name. Handwritten input objects, wherever they are reached, now disclose only fields the host declares with `AiCustomInputDisclosure`, never by entity-name prefix; an input counts as a generated entity input only when it has the canonical generated identity and member types and a generated operation or relationship reaches it, and a canonical aggregate metric input applies the generated field and operator rules on every root that uses it. Aggregate plans match exact field names and refuse repeated normalised keys. No data migration. See the [AI migration guide](crates/graphql-orm-ai/MIGRATION.md#01400-to-01420-exact-tool-registration-and-derived-control-plane-roots) and the [tool-profiles migration guide](crates/graphql-orm-ai-tool-profiles/MIGRATION.md#0220-to-0230-explicit-handwritten-input-disclosure).

- ORM and macros 0.147.0 (placeholder): **breaking/security:** generated write paths, hooks and authority. See the [migration guide](MIGRATION.md#01440-to-01470-write-paths-hooks-and-authority).
  - **Breaking (security):** the generated repository `Entity::insert(provider, input)` takes one authorized path whatever the provider is. Called with a bare pool (or any `PoolProvider` that is not a `Database`) it ran a raw `INSERT`: no entity, row or field policy, no schema policy, no `WriteInputTransform`, no mutation hook, no written-row check and no transaction. It now behaves as the same call on `Database::new(pool)` does: an entity that declares a `write_policy`, or a field that declares one, is refused with `AUTHORIZATION_MISCONFIGURED`, a SQL Server pool keeps the access mode it was opened with (a read-only pool refuses, an external-writable pool writes), and an entity without policy keys inserts as before, inside a transaction. Pass the `Database` that carries the host's policies and hooks (#601).
  - **Breaking:** each generated GraphQL `updateMany` or `deleteMany` field may write at most `PaginationConfig::max_bulk_write_rows` rows (default `10_000`, `PaginationConfig::DEFAULT_MAX_BULK_WRITE_ROWS`). A bulk write that matches more rows the caller may write is refused before any row changes, with `errorCode = "PAGE_LIMIT_EXCEEDED"` in the payload; it is never truncated, and the message names no count. They previously loaded and locked every matching row. The bound counts rows after visibility, so rows the caller can neither read nor write never decide the outcome. With a row policy installed, or a filter part evaluated in Rust (SQLite spatial predicates), candidates are read and classified in pages and the scan takes at most `PaginationConfig::max_scanned_rows` candidates, with the refusal authorized reads already use; it no longer materializes every candidate. Results under the bound are unchanged; the rows are now locked and written in primary-key order. Set the bound with `PaginationConfig::with_max_bulk_write_rows` or `DatabaseBuilder::max_bulk_write_rows`; `None`, `PaginationConfig::unbounded()` and `DatabaseBuilder::unbounded_pagination()` remove it. It applies per bulk mutation field, not per request. Repository `update_where` and `delete_where` are trusted code and stay unbounded (use the `*_bounded` helpers there). `PaginationConfig` has a new public field, so a struct literal without `..` needs it (#446).
  - **New:** `RowPolicy::write_target_prefilter` lets a host narrow, in SQL, the rows a generated GraphQL `update`, `delete`, `updateMany` or `deleteMany` selects as targets. Rows outside the predicate are never selected, locked or passed to a row callback, which removes the timing and lock-contention difference between a hidden row and a missing one, and keeps hidden rows out of the bulk bound. The predicate only narrows: every selected row still goes through `can_write_row` and `can_read_row`. The default returns `None` and changes nothing (#602).
  - **Breaking (security):** a `#[graphql(entity)]` resolver in a `#[graphql_orm_custom_operations]` root enforces operation assurance in its body: the requirement registered for its own catalogued coordinate, then any registered for `Query._entities`, after its declared authorization. It was catalogued and could be classified, but nothing enforced the requirement below `_entities`. Such a resolver now needs a named `&Context<'_>` argument and a `Result` return type even when it is public (`authorization = false`); without them it does not compile (#575).
  - **New:** hooks and workers can present an explicit authority to entity, row and field policies. `Database::with_system_access(SystemAccess)` labels a handle for a trusted worker; `MutationContext::with_system_access(access, |tx| ...)` labels the writes a mutation hook makes inside the scope. Policies read the label with `Database::system_access()`, and repository field decisions receive it as `AccessContext::System` instead of `None`. The label grants nothing by itself, and calls without one still reach policies with no authority, as before. Inside a scope every policy check of a write, the written-row check included, uses the scope's access. The surface (`EntityAccessSurface::Repository`) and the `ctx` policies receive are unchanged (part of #291).
  - **New:** `schema_roots!` accepts `require_policies: true`, which makes a listed entity with no explicit policy decision a compile error that names it. An entity `read_policy` or `write_policy` is a decision, as are an entity-level `auth = "none"` and the new `#[graphql_entity(policy = "open")]` marker, which records that the entity is deliberately open and changes nothing at run time. `Entity::POLICY_DECISION_DECLARED` exposes the same fact. Without the setting nothing changes (#287).
  - **New:** typed transaction-scoped extensions. The code that began a transaction attaches a value with `MutationContext::insert_extension`, and mutation hooks read it with `MutationContext::extension::<T>()` for the writes of that transaction only, including bulk and nested writes. It is dropped before the commit or rollback, is never visible to deferred post-commit actions, other transactions, other handles or generated GraphQL mutations, and a hook cannot set or remove one (#564).
  - **New:** an ordered sort form for Rust callers. Every entity derive generates a Rust-only `<Entity>OrderField` enum, every generated query builder has `order_by_fields([(Field::A, Asc), (Field::B, Asc)])`, which sorts by the first pair first and composes with `order_by` in call order, and `<Entity>KeysetOrderByInput::ordered(...)` builds an ordered keyset sort. One `OrderByInput` value with several fields set still sorts in field declaration order; that is now documented on the builders and in the reference. GraphQL behaviour and the generated SDL are unchanged (#694).
  - Regression tests only, no behaviour change: default field names of described custom roots where digits meet letters (#576) and static API-name metadata for every order of `skip = false`, `skip_output = false`, `visible` and `name` (#405); both were fixed in earlier releases.

- graphql-orm and graphql-orm-macros 0.144.0: pin test-owned PostgreSQL images by digest. No runtime API or data migration.
- CI/release hygiene: exact expiring advisory dispositions for every tracked lockfile, scoped checkout credentials, manifest-bound release tags and main-only broad PostgreSQL execution; retain protected release approval and read-only bundle generation.

- ORM and macros 0.143.0 (placeholder), AI 0.139.0 (placeholder): **breaking/security:** one authorization contract for field policies, handwritten roots and refusals. See the [migration guide](MIGRATION.md#01410-to-01430-one-authorization-contract).
  - **Breaking (fix):** every generated read path asks the installed `FieldPolicy` for a field's read decision, with the field's declared key or `None`. Generated scalar getters, `RecordVersion`, relation fields, relation-target field checks and `search_relation` sources previously asked only for fields that declare a `read_policy`, while `where`, `orderBy`, aggregates, search sources, Keyset cursors, subscription keys, `graphql_policy_object` fields and repository reads already asked for every field. A provider that denies a key-less field is now obeyed when the field is selected, on get-by-key roots, list and connection items, relations, mutation payloads, `Changed` events, Federation `_entities` and `NodeId` results and search hits. The generated `NodeId` field asks for itself and for every key field it encodes, each under a field context of its own name. Without a provider a key-less field stays readable in every mode. Scalar getters of a plain `SimpleObject` entity stay async-graphql's and are not asked; `graphql_policy_object` applies the rule to them. See [field read decisions](docs/reference/graphql-orm/strict-authorization.md#field-read-decisions) ([#447](https://github.com/Dastari/graphql-orm/issues/447)).
  - **Breaking (fix):** generated writes ask the field write decision with the declared `write_policy` key or `None`. The repository writes of a `GraphQLOperations` entity (`insert`, `insert_many`, `insert_if_absent`, `update_by_id`, `update_by_key`, `update_where`, `update_if`, `compare_and_swap` and their transaction forms) asked nothing, and `upsert` asked only for keyed fields; they now ask `can_write_repository_field` for every field the host code writes, as repository-only entities already did, and a refusal names the field. ORM-managed columns are not asked about. The default `can_write_repository_field` denies a declared key, so repository writes of fields with a `write_policy` need a provider that implements it; the callback receives `access: None` on every generated repository write. GraphQL create, update, upsert and bulk update keep asking `can_write_field` for fields with a key exactly as before, and now also ask for a key-less field when it is part of the GraphQL input type and the client supplied it; an upsert that resolves to an existing row asks about every field its update sets, including a key-less optional field the client omitted (written as `NULL`) and a field outside the input type (written as its default), with the current row and the value about to be written. Otherwise they never ask about a field the client cannot send or a value only a transform or hook set, and never also ask the repository decision. Without a provider a key-less field stays writable in every mode. See [field write decisions](docs/reference/graphql-orm/strict-authorization.md#field-write-decisions) ([#447](https://github.com/Dastari/graphql-orm/issues/447)).
  - **AI 0.139.0:** the AI crate's own persistence no longer depends on the host's repository field decisions. Its internal database handles answer repository-surface field decisions for the crate's own records and delegate every other entity and surface to the host `FieldPolicy`; `database()` accessors return the host's handle. See the [AI migration guide](crates/graphql-orm-ai/MIGRATION.md#01380-to-01390-custody-of-the-crates-own-records-under-a-host-field-policy).
  - **Breaking (security):** `#[graphql_orm_custom_operations]` enforces the authorization it declares. A described root requires a typed `AuthSubject` (`UNAUTHENTICATED` otherwise) before the operation-assurance guard, any host guard and the resolver body, whatever the schema's `auth` default is. `authorization = true`, the default, previously recorded `has_authorization_policy` in the catalogue and enforced nothing. The new `all_scopes = [...]` and `any_scopes = [[...], ...]` arguments add the fixed scope requirement of an entity's `operation_authorization` (`FORBIDDEN` when unmet). `authorization = false` declares the root public and is the only way to get `has_authorization_policy: false`. A resolver's `#[graphql_orm(...)]` declaration narrows its root's requirement (`authorization = true` keeps it, `all_scopes`/`any_scopes` add to it); a different requirement must be written as `authorization(replace, ...)`, and a public resolver as `authorization = false`. `rename_args` on the root attribute is refused, like `rename_fields`. A `#[graphql(entity)]` resolver, which async-graphql never guards, is checked in its body and must take a `Context` and return a `Result`, or declare itself public. A described subscription's composed guard (declared authorization, assurance and host guard) is evaluated again while its stream is open, before each item and on the timer generated `Changed` subscriptions use, and the stream ends when it refuses. The declared authorization reads the subject accepted for the connection, so live revocation comes from assurance or a host guard such as `AgqlCurrentAuthorityGuard`. New public `graphql::auth::DeclaredAuthorizationGuard` and `enforce_declared_authorization` ([#449](https://github.com/Dastari/graphql-orm/issues/449)).
  - **Breaking:** `schema_roots!` rejects, at compile time, two composed types that contribute the same field to one root: a handwritten root field with the name of a generated operation, two handwritten roots that declare the same field, two entities whose generated operations share a name, or one entity that generates the same root field twice (a `plural` equal to its name). The error names the root, the field and both owners. Such a schema previously compiled and served only one of the two; the only signal was a panic from `graphql_orm_semantic_catalog()` for described roots. Roots in `extra_query_types`, `extra_mutation_types`, `extra_subscription_types` and `query_custom_ops` must now declare their field names with the new `#[graphql_orm_root_fields]` attribute, or implement `GraphqlRootFieldNames` by hand for a `MergedObject` root; a root described with `#[graphql_orm_custom_operations]` already does. Names are derived with async-graphql's own rule (also for a described root's catalogued field and argument names, which could differ where a digit meets a letter), a `graphql` attribute applied through `cfg_attr` is refused, and the generated schema builders panic when a handwritten root exposes a field its declaration omits ([#282](https://github.com/Dastari/graphql-orm/issues/282)).
  - **Fix:** host policy callback errors keep their class and never carry their text. `FieldPolicy::can_read_repository_field` and `can_write_repository_field` errors were all reported as `INTERNAL_ERROR`; an authored `FORBIDDEN`, `UNAUTHENTICATED` or `AUTHORIZATION_MISCONFIGURED` refusal, or a typed `OrmPublicError` returned with `?`, now reaches repository callers and GraphQL clients with its code, and every other error from them, including a plain `Error::new(text)`, stays `INTERNAL_ERROR` with the default message. A relation boundary classifies as `graphql_error_from_callback` does: an error the policy authored (no source) is a refusal, `FORBIDDEN` or `UNAUTHENTICATED`; a propagated error, or one carrying `INTERNAL_ERROR` or `SERVICE_UNAVAILABLE`, keeps that failure class; `AUTHORIZATION_MISCONFIGURED` is kept. It reported everything but a misconfiguration as `FORBIDDEN` ([#450](https://github.com/Dastari/graphql-orm/issues/450)).
  - **Breaking:** under `ExplicitPolicyForAllExposedOperations`, a field that declares a policy key with no `FieldPolicy` installed fails with `AUTHORIZATION_MISCONFIGURED`, as under `DeclaredPoliciesRequired`. It was `FORBIDDEN`. A key-less field without a provider stays readable and writable; the comment that said otherwise is corrected.
  - **Breaking (operations):** diagnostics go through `tracing` instead of standard error. Internal error reports are events on the `graphql_orm::internal_error` target that carry the public code; the diagnostic text is attached as `detail` only while `GRAPHQL_ORM_LOG_INTERNAL_ERRORS` is set. An unhandled post-commit action failure (`graphql_orm::post_commit`), the undeliverable `Changed` event warning (`graphql_orm::subscriptions`) and the preserved SQLite rebuild table notice (`graphql_orm::migrations`) are events too. None of them formats a row. A host without a `tracing` subscriber no longer sees these lines.

- ORM and macros 0.141.0 (placeholder): **breaking/data safety:** reject `#[transform(write = "path")]` at the `write` key with a field-specific diagnostic. If an entity with this attribute compiled, the transform never ran and generated writes stored values as supplied. Use `WriteInputTransform` on `Database` and remediate existing rows. All generated entity combinations now refuse it, including schema-only and skipped fields. Read transforms remain supported. See the [migration guide](MIGRATION.md#01390-to-01410-write-transforms-fail-closed) (#681).

- ORM and macros 0.139.0 (placeholder), catalogue 0.7.4: add optional `stable_type_id` on entity declarations to preserve the entity identity input across Rust module or crate moves (#683, option 2). Field/argument type and relation-target spellings must also remain unchanged to preserve fingerprints; compare catalogues before and after a move. Accept opaque printable-ASCII tokens of 1–1024 bytes without whitespace or control characters. Reject duplicate catalogue identities and retain the real Rust path for diagnostics. Default fingerprints and algorithms are unchanged.
- ORM and macros 0.137.0 (placeholder): no host-visible change. **Internal (compile cost):** `GraphQLOperations` and `RepositoryEntity` emit the per-field UPDATE assignment list once per entity, as a private synchronous helper, instead of splicing it into every generated update path (the GraphQL `update` resolver, `update_by_id`, `update_where`, their transaction forms, the composite-key forms and `compare_and_swap`). On synthetic crates of 8 and 32 fifteen-field entities this removes about 2.5% of the expanded lines and about 2% of rustc's peak memory; rustc time did not change measurably. See [#677](https://github.com/Dastari/graphql-orm/issues/677) for where the rest of the cost is. Generated SDL, operation and semantic catalogues, the generated public Rust API and runtime behaviour are unchanged; no authorization, policy or sanitization code moved. The macro refuses an assignment-list entry that awaits, returns, or uses the resolver context or the database. **Tooling:** `scripts/compare-generated-schema.sh <base-ref>` compares the full-stack fixture's SDL, operation catalogue and semantic catalogue between a base revision and the working tree, for SQLite and PostgreSQL.
- ORM and macros 0.136.0 (placeholder): PostgreSQL backup snapshots, restore emptiness checks and search rebuild fail when row security would filter rows (#526). Full rebuild reads and writes use one guarded transaction. Single-document rebuild completes its guarded read before host callbacks and the index write transaction, allowing callbacks to use a one-connection pool.
- CI and release supply chain: CI's schema-export job now uses the same full-SHA action pins and Rust 1.97.1 toolchain as every other job (#492). The release publisher is split: a read-only, cache-free `release-bundle` job (and `release-scoped-bundle` for the scoped profile) generates the manifest and notes, builds the optional router artifact and checksums the bundle, and the `release`/`release-scoped` publisher, the only job holding `contents`, `id-token` and `attestations` write, runs no compiler, interpreter, cache or repository script. It checks the bundle artifact's ID and digest against the build job's outputs, verifies every file against the checksum list whose digest the build job reported, then attests, tags and publishes as before (#493). Full release validation adds a `schema-export` lane that runs the macros crate tests and the schema-export equivalence check with the lockfile fixed, as CI does (#494). `scripts/check-workflow-supply-chain.py` enforces these rules in CI and release integrity. Release inputs, the protected `release` environment on the entry job, manifest generation and workflow-only tag creation are unchanged.
- Storage 0.9.3: the managed Samba test runner uses a digest-pinned image and refuses tag-only `SMB_TEST_IMAGE` overrides (#495). Test harness only.
- `compose.yaml` is marked local-test-only: PostgreSQL is published on `127.0.0.1` only, the image is digest-pinned, and there is no default password; `GRAPHQL_ORM_TEST_POSTGRES_PASSWORD` must be set (#500).

- ORM runtime/macros 0.133.0: **breaking:** runtime schema validation refuses reserved physical table names: the prefixes `pg_`, `sqlite_` and `pragma_` (database catalogs and pragma functions), `__graphql_orm_` (ORM infrastructure) and `graphql_orm_` (companion-package tables), and the exact names of SQLite's table-valued functions (`dbstat`, `generate_series`, `json_each`, `json_tree`, `jsonb_each`, `jsonb_tree`) and SQL Server's backward-compatibility views (such as `sysobjects` and `syscolumns`). Runtime reads, writes and migrations address their tables by bare name, so these names would address a table the schema does not own. Collection tables report `InvalidPhysicalName` and link tables `InvalidJoinTable`; the lists are exported as `RESERVED_PHYSICAL_TABLE_PREFIXES` and `RESERVED_PHYSICAL_TABLE_NAMES`. **API:** `RuntimeSchema::validate_with_reserved_tables` lets trusted code admit exact `graphql_orm_` table names; the allowance is never part of a stored definition. **Breaking:** a string default that contains a NUL character is refused with `RuntimeDefaultProblem::NulCharacter`. **Fix:** PostgreSQL runtime literal defaults that contain a backslash, and bytes defaults, are written as escape strings (`E'…'`), and owned PostgreSQL introspection interprets deparsed defaults under the session's `standard_conforming_strings`, so default values are preserved exactly, and plans are the same, under either setting. Existing columns created with the earlier spelling plan no change. `physical_table` is documented as authority-bearing: runtime schema definitions must come only from trusted sources. See the [migration guide](MIGRATION.md#01320-to-01330-reserved-runtime-table-names).

- AI 0.131.0: **security:** fail closed on work queued before 0.130.0 with a delegated or sessionless stored principal reference. Workers resolved such a reference to the parent user session and acted as the full user. Every principal-reference resolution now refuses that shape before the host resolver, as it refuses an expired reference, and the run, approved-run, approval-wait and title-work claim paths close these legacy rows with the safe code `principal_reference_delegated` before any lease, provider egress, tool call, approval consumption, decryption or host callback. The runs are not retryable. This enforces the 0.130.0 manual cleanup automatically. No persistent schema or data migration change. See the [AI migration guide](crates/graphql-orm-ai/MIGRATION.md#01300-to-01310-legacy-delegated-stored-principal-references).

- AI 0.130.0: **breaking:** refuse delegated and sessionless principals on session submission and session reads. `sendAiMessage` previously persisted a `SessionBoundDelegation` or `Sessionless` credential's reference as the new run's principal unless the host installed run authorization, and workers resolved it to the parent user session, so the run acted as the full user. Message submission, every other session root (bootstrap, session list and reads, messages, message blocks, session-event pages, stream heads, create, pin, rename, archive, restore and delete), attachment roots and inbox pages now return `AI_REAUTHORIZATION_FAILED` for those credentials, and for one with no grant kind that carries an actor or exact operation binding, before any read or write, whether or not run authorization or submission admission is configured. Pages now match their streams, and these roots cannot be receiver-admission targets. There is no opt-in for delegated submission. No persistent schema or data migration change. See the [AI migration guide](crates/graphql-orm-ai/MIGRATION.md#01290-to-01300-delegated-and-sessionless-session-credentials).

- AI 0.129.0: **breaking:** refuse delegated and sessionless principals on approval and proposal decisions. Approval list, read, decide and revoke, proposal list, read, review and applied-outcome linking, run cancellation, failure retry and acknowledgement, tool-result previews, and session-event and inbox streams now return `AI_REAUTHORIZATION_FAILED` for a `SessionBoundDelegation` or `Sessionless` request credential, as run issuance already does, and for one with no grant kind that carries an actor or exact operation binding. They previously resolved a delegation to its parent user session and acted as the full user. No persistent schema or data migration change. See the [AI migration guide](crates/graphql-orm-ai/MIGRATION.md#01280-to-01290-delegated-and-sessionless-request-credentials).

- AI 0.128.0: **breaking:** enforce AI runtime closure after a snapshot restore. The runtime start gate opens only with a proof minted from a store's live recovery epoch (AI schema module `0.77.0`), and leases are bound to that epoch. Worker claims, lease-fenced writes and approval consumption refuse while the epoch is not live. A restored store stays closed until `OrmAiRuntimeRecoveryService::reconcile_restore` expires its approvals, revokes its egress consents and moves unfinished runs to `RecoveryRequired`, and an operator acknowledges the reviewed result. An existing deployment upgraded in place opens once the operator calls `acknowledge_in_place_upgrade`, which leaves its work unchanged. Restore facts now decode strictly. See the [AI migration guide](crates/graphql-orm-ai/MIGRATION.md#01270-to-01280-enforced-runtime-closure-after-restore).

## 0.133.0

ORM runtime/macros **0.133.0**. Repository limits and offsets count visible rows.

- **Fix (data safety)/breaking:** when the row policy decides rows in Rust (a row policy is installed and its Repository-surface `read_visibility` decision is `CallbackOnly` or `Prefilter`; `search_db`, whose executors do not receive the predicate, also under `Complete`), `limit(n)` and `offset(k)` on `RepositoryQuery`, `tx.query::<T>()` (`fetch_all`, `fetch_one`) and `search_db` (`RepositorySearchQuery`) count visible rows, and a page is either complete or the read fails. The read takes ordered batches of candidate rows, skips `k` visible rows and returns up to `n`. `fetch_first` and `fetch_optional_one` use the same window.
- **New:** a visible-row scan, with or without a limit, takes at most `PaginationConfig::max_scanned_rows` candidate rows from the database (default `PaginationConfig::DEFAULT_MAX_SCANNED_ROWS`, `10_000`; `None` removes the bound), plus one look-ahead row. When the budget runs out before the page is complete and more rows remain, the read fails with `PAGE_LIMIT_EXCEEDED`; the message names no row count. On `RepositoryQuery` and `tx.query::<T>()` an unlimited scan keeps the fail-closed `max_limit` check as well, and whichever bound is reached first refuses it; unlimited `search_db` has no `max_limit` refusal, so under a residual decision only the budget bounds it, and without a scan it stays unbounded. The budget bounds scans only: a read with no row policy, or with an `Unrestricted` (or, outside `search_db`, `Complete`) decision, is one query as before and is never refused by it; if its row callback refuses a row of a bounded page, the rescan counts that first query's rows. Residual in-memory predicates are evaluated per scanned candidate, and a scanning fallback search reads at most `max_scanned_rows + 1` candidates once. `with_max_scanned_rows(n)` overrides the budget for one read on `RepositoryQuery`, `tx.query::<T>()` and `RepositorySearchQuery`, and `PaginationConfig::with_max_scanned_rows` sets it for a handle. **Breaking:** `PaginationConfig` has a new public field, so code that builds it with a struct literal must set `max_scanned_rows`.
- **Change:** `limit(0)` on these builders returns an empty page without reading, after authorization, the read decision and limit validation. Without a row policy, two things also change: SQLite and MySQL render an unlimited read with an offset with a `LIMIT` (`-1`, or the MySQL maximum) because both reject a bare `OFFSET`, and `search_db` resolves the read decision before searching, so under `DeclaredPoliciesRequired` an entity with a declared read policy and no row provider is refused with `AUTHORIZATION_MISCONFIGURED` even when the search would find nothing.
- **Change:** the primary-key tiebreak appended to ordered reads counts only a term that orders directly by the key column, so an order expression that merely mentions the key also gets the tiebreak. Orders with a direct key term are unchanged.
- Unchanged: with no row policy installed, an `Unrestricted` decision, or (on `RepositoryQuery` and `tx.query::<T>()`, which add the 0.130.0 Repository-surface predicate to the base query before both the SQL window and the scan) a `Complete` decision, these reads keep one SQL `LIMIT`/`OFFSET` query; if the row callback still refuses a row of such a bounded page, the read rescans, so the page is still complete. The unlimited `fetch_all` fail-closed check still counts rows before row policy and names no count. Projections refuse residual decisions, and `write_ctx.query::<T>()` and the pool-bound `Entity::query(pool)` apply no row policy, so their `LIMIT`/`OFFSET` stay in SQL. GraphQL connections and search roots keep their own authorized pagination.

## 0.132.0

ORM runtime/macros **0.132.0**.

- **Hardening:** PostgreSQL catalog reads use catalog-defined objects regardless of the session's `search_path`.
  - RLS validation (`validate_target` and `validate_rls_enforcement`) first resolves the declared table paths, the change journal and the registry's declared tables under the session's `search_path`, using queries with no operators. It then reads everything else by OID with `search_path` pinned to `pg_catalog, pg_temp`, inside a transaction of its own that is always rolled back.
  - These reads name every catalog relation, function, type and operator with its schema (`OPERATOR(pg_catalog.=)` and so on) and change no session or transaction state: schema introspection, the dependent-delete gate, the runtime migration's catalog checks, the change-journal recorder check, and the migration-history and search-language checks. They are safe on a connection that holds the caller's transaction, including when their future is dropped. Column types and defaults render with the session's path.
  - The plan's `DO` blocks name catalog objects and operators with their schema; only declared table names resolve through the applying session's path. This covers the change journal's RLS statement, privilege revocation and its check, the trigger installers, stray recorder-trigger and registry cleanup, unique-constraint removal and append-only function ownership. Plans that contain these statements have new statement text and plan hashes; see the migration guide.
- **Fix:** a partial index counts as a closed-set predicate only when it uses default operator classes that, with their operator families, belong to `pg_catalog`; every operator, function, type and collation `pg_depend` records for it belongs to `pg_catalog`; and its predicate renders in the closed-set form. Introspection reports any other partial index without a predicate, and runtime migrations refuse it.
- **Fix:** the change journal is validated where the connection's `search_path` resolves it, in any schema: its policies, RLS enablement and enforcement are checked there. The search-document delete trigger is looked up on the table the base path resolves to ([#461](https://github.com/Dastari/graphql-orm/issues/461)).
- **Docs:** generated policies on extension-type tenant and owner columns. A `citext` column, or a domain over `text`, validates clean: PostgreSQL binds the catalog's `text` equality, so the comparison is case-sensitive. A column whose equality with `text` resolves to an operator outside `pg_catalog` is reported as "could not be compared", never accepted; use a custom predicate that names the operator and type with their schema. Owned-PostgreSQL tests pin both ([#640](https://github.com/Dastari/graphql-orm/issues/640)).
- **Deprecated:** `RlsIntrospectionBackend::introspect_rls`. It keys tables by a flattened `schema_table` name and omits `FOR ALL` policies; nothing in the crate calls it. Use `introspect_rls_target` and `introspect_rls_enforcement`.
- **Internal:** the doc-hidden `LiveRlsEnforcement` replaces `row_security_active` and `policy_names` with `change_journal_table`, and keys `search_delete_triggers` by quoted schema and table. `validate_target` no longer calls `RlsIntrospectionBackend::introspect_rls`.

## 0.131.0

- ORM and macros: native SQLite and PostgreSQL search hide managed search-table
  columns from entity SQL, so names such as `rank` and `entity_pk` work in
  repository and generated GraphQL searches (#659). Entity join keys and
  primary-key tiebreaks are qualified; server-authored default-sort SQL is passed
  through unchanged, including correlated subqueries and typed literals.
  Relevance and result ordering are unchanged. Macros stay version-aligned.

## 0.130.0

ORM runtime/macros **0.130.0**. Authority defaults now fail closed. See the [migration guide](MIGRATION.md#01290-to-01300-fail-closed-authority-defaults).

- **Breaking:** generated single-row reads apply the same row authority as generated list reads. Get-by-key roots, `NodeId` lookups and Federation `_entities` add the SQL predicate from `RowPolicy::read_visibility` to the read, and call `can_read_row` only when the decision is `CallbackOnly` or `Prefilter`. A row outside a `Complete` predicate is now `null` even when `can_read_row` allows it, and a `Complete` or `Unrestricted` decision no longer runs `can_read_row` on these reads.
- **Breaking:** generated create, update and upsert payloads release the committed row under the same authority. After the commit the row is re-read by key under the `GraphqlMutation`-surface predicate, and that re-read row is the one `can_read_row` checks (under a residual decision) and the payload carries. A row outside the predicate or refused by the callback is withheld (`null` entity, `success: true`), as a create's output already was.
- **Breaking:** repository single-row reads (`find_by_key`, `find_by_id`), repository list reads (`RepositoryQuery`) and transactional entity reads (`MutationContext::query().fetch_all` / `fetch_one`) add the `Repository`-surface read predicate to their SQL. They still call `can_read_row` for every row. Typed projections already applied it. Generated writes look up their target rows with an internal locking fetch that checks `can_read_row` only; `MutationContextFindByKey::find_by_key_in_mutation_context` wraps the same fetch for hosts and stays a trusted host surface.
- **Fix:** errors from `EntityPolicy`, `RowPolicy::read_visibility` and `can_read_row` on these single-row reads, including Federation `_entities` and `NodeId`, are sanitized like other callback errors, so a failure propagated with `?` no longer reaches clients as text. `_entities` and `NodeId` resolve the read decision before looking at the loaded row, so a refusal is the same for an existing and a missing key.
- **Breaking:** without a `RowPolicy`, an entity that declares an entity `read_policy` key no longer gets `Unrestricted` read visibility outside `LegacyPermissive`:
  - Under `DeclaredPoliciesRequired` (the default), every read of the entity fails with `AUTHORIZATION_MISCONFIGURED` before its row decision is applied, so on get-by-key, `_entities` and `NodeId` an existing and a missing key answer alike. That covers lists, single-row reads, relation loads, relation filters and counts, generated search roots, aggregates and group pages, typed projections, repository reads and counts, and Keyset roots.
  - Under `ExplicitPolicyForAllExposedOperations`, visibility is `CallbackOnly`, so every read defers to `can_read_row`, which refuses. Single-row reads return `null`, a list or search root with an authorized scan configuration returns no rows, and paths that need SQL-only visibility (offset lists and search roots without a scan configuration, relation filters and counts, aggregates and group pages, projections, repository counts and Keyset roots) fail with `AUTHORIZATION_MISCONFIGURED`.
  - Generated `Changed` subscriptions for such an entity never deliver an event in either strict mode: each event's authorization fails and the event is dropped without an error to the subscriber. The runtime prints one warning per process to standard error the first time this happens.
  - The Keyset registration check refuses such roots with the new reason `KeysetReadPolicyReason::MissingRowPolicy`.
- **Breaking:** typed projections, repository counts, aggregates and group pages that get a residual (`CallbackOnly` or `Prefilter`) decision now fail with the public code `AUTHORIZATION_MISCONFIGURED`. They previously returned an untyped `sqlx::Error::Protocol`.
- **Breaking (macros):** a field `read_policy` on a `GraphQLEntity` + `SimpleObject` entity (`#[graphql(complex)]` or `schema_only = true`) is a compile error unless `#[graphql_orm::graphql_policy_object]` is applied or the field has `#[graphql(skip)]`. The `SimpleObject` getter did not enforce the policy. `graphql_policy_object` now also leaves an inert `#[graphql_orm_policy_object]` marker and a hidden witness trait impl; the ORM derives check the marker and assert the witness, so a hand-written marker without the attribute does not compile. The attribute cannot be used when `SimpleObject` reaches the derive list through a `macro_rules!` argument.
- **Breaking:** `ScopeEntityPolicy` holds a `ScopeRequirement` per access kind (`read`, `write`) instead of `read_scopes`/`write_scopes`. An empty scope list denies; `ScopeEntityPolicy::authenticated()` or `ScopeRequirement::AnyAuthenticated` (through `with_read`/`with_write`) admits any authenticated subject explicitly. `optional` denies a request without a subject. `Default` is removed.
- **Breaking:** `AuthExt::auth_subject`, `auth_subject_opt` and `auth_user_id`, and so generated resolver auth and `ScopeEntityPolicy`, read only a typed `AuthSubject` from GraphQL context data. A bare `String` is no longer upgraded to a subject.

## 0.129.0

ORM runtime/macros **0.129.0**.

- **Breaking (fix):** every field a read argument references passes the same field read policy as a selected field, before any SQL. This now covers `orderBy` on root lists and to-many relation fields, to-many relation `where`, the generated search `where`, and `updateMany`/`deleteMany` filters, which previously checked only the response's selected fields. A denied field returns `FORBIDDEN`; a bulk write reports the refusal in its payload and writes nothing (part of [#602](https://github.com/Dastari/graphql-orm/issues/602)). One public runtime helper, `authorize_read_fields`, now backs every generated `where`, `orderBy` and related-row filter check.
- **Breaking (fix):** `searchable(policy)` and `search_json(policy)` are enforced. The generated `…Search` root and `Entity::search_db` authorize every search source before any SQL: each local field's read policy and search policy key, and for `search_relation` the relation's read policy and search key, the target entity and every copied target field, plus the target's traversal auth mode and scopes on the GraphQL search root. Denying any source refuses the whole search with `FORBIDDEN`. `search_relation` field names are checked against the target's persisted fields at compile time.
- **Breaking:** `GraphQLOperations`' `Entity::search_db(&database, …)` returns the policy-aware `RepositorySearchQuery` (entity access, search sources, and row and field policies on each hit) instead of the unchecked `EntitySearchQuery`. `Entity::search(pool, …)` remains the trusted, unchecked builder.
- **Breaking (fix):** relation traversal enforces the target entity's own `auth` mode and fixed `operation_authorization` scopes: `list` for to-many relation fields, related-row filters and `order_aggregate` counts, and `single_read` for to-one fields. When that category is not declared but another read category (`list`, `single_read`, `keyset_list`, `search` or `subscription`) is, traversal requires every declared read category's requirement. A templated category refuses traversal. `GraphQLEntity` and `RepositoryEntity` emit the new `RelationTraversalAuthorization` implementation for every entity.
- **Fix:** native PostgreSQL and SQLite full-text search refuses an invalid filter, such as an unprepared related-row filter, with `INVALID_INPUT` before any SQL, as the fallback path and `EntityQuery` already did ([#558](https://github.com/Dastari/graphql-orm/issues/558)).
- **Fix:** host callback failures on generated federation `_entities`, list read visibility, scalar field reads, filter authorization, keyset and subscription paths, and the entity-access refusal in `updateMany`/`deleteMany` payloads go through the callback sanitizer. A failure propagated with `?` is reported as `INTERNAL_ERROR` without its text; authored refusals keep their code and message.
- `DatabaseOrderBy` gains `referenced_fields()` (default empty) and `SearchableEntity` gains the hidden `search_relation_source_policies()` (default `None`, which fails closed for entities with relation search sources). Generated inputs and entities implement both.

## 0.128.0

ORM runtime/macros **0.128.0**, AI **0.134.0**. Rust repository limits are exact or refused; SQL Server transport and pool hardening.

- **AI 0.134.0 (fix, data safety):** the runtime's internal reads honour their own bounds through the per-read `with_max_limit`, so lease recovery, approval-wait reconciliation, native checkpoints, session retention and inbox pruning no longer stop at 100 rows on a default database. Client page sizes still follow the host page maximum. See the [AI migration guide](crates/graphql-orm-ai/MIGRATION.md#01330-to-01340-internal-reads-honour-their-own-bounds).

- **Fix (data safety)/breaking:** Rust repository builders no longer reduce an explicit `limit(n)` to the database's maximum page limit. `RepositoryQuery`, transaction `tx.query::<T>()` and `tx.project::<P>()`, projection `P::query(&db)`, `search_db`, write-input `write_ctx.query::<T>()`, the pool-bound `FindQuery` and `EntitySearchQuery::limit` read exactly `n` rows, and fail with `PAGE_LIMIT_EXCEEDED` when `n` exceeds `max_limit` (default `100`) or with `INVALID_INPUT` when `n` is negative. Previously `limit(bound + 1)` returned at most 100 rows with no error, so an overflow check with a bound of 100 or more could never fire and callers worked on a truncated set ([#644](https://github.com/Dastari/graphql-orm/issues/644)).
- **Fix (data safety)/breaking:** without a limit, `RepositoryQuery::fetch_all`, `tx.query::<T>().fetch_all()`, projection `fetch_all` and repository-only `find_all`/`find_many` read at most `max_limit` rows and fail with `PAGE_LIMIT_EXCEEDED` when more rows match. They previously returned the first `default_limit` (50) rows silently. `default_limit` no longer applies to these reads.
- **Fix:** transaction reads apply the page their caller resolved. They previously re-applied the built-in maximum of 100 whatever the database was configured with, so the transaction keyset page (`MutationContext::keyset_page`, `keyset_connection_page`) reported no next page when the page size was 100 or more, a raised `max_page_limit` was ignored inside transactions, and the subscription propagation fan-out budget (256 targets) stopped detecting overflow at 100 targets.
- **Fix:** projection `fetch_optional_one` keeps its two-row uniqueness look-ahead when `max_limit` is `1`.
- **API:** `with_max_limit(maximum)` on `RepositoryQuery`, `tx.query::<T>()`, `ProjectionQuery`, `tx.project::<P>()`, `write_ctx.query::<T>()` and `FindQuery` raises the maximum page limit for one trusted read only. The read stays bounded by `maximum`, and the database-wide maximum that GraphQL clients see is unchanged.
- **Fix/breaking:** generated `search_similar` scores all of its five-per-result candidates instead of stopping at 100, and refuses a `limit` above `SIMILAR_SEARCH_MAX_LIMIT` (`1_000`) with `PAGE_LIMIT_EXCEEDED`, or a negative one with `INVALID_INPUT`.
- **Note:** the fail-closed check of an unlimited `fetch_all` counts rows before row policy, so it can refuse because of rows the caller cannot see. Its message names no row count.
- **API:** `PaginationConfig::checked_explicit_limit` checks a trusted limit without changing it. `paginate(PageInput)` on `tx.query::<T>()` and `write_ctx.query::<T>()` is now `limit` plus `offset`, without clamping. GraphQL connections, `EntityQuery::paginate` and `EntitySearchQuery::paginate` keep clamping, and report the remaining rows through `pageInfo`. Bounded mutations, retention purge and dependent deletes are unchanged; they already used an uncapped `MutationLimit` look-ahead. See the [0.128.0 migration guide](MIGRATION.md#01270-to-01280-repository-limits-are-exact-or-refused) and [Rust repository limits never truncate](docs/reference/graphql-orm/runtime-and-writes.md#rust-repository-limits-never-truncate).

- **Breaking, security:** SQL Server connections use TLS for the whole session and validate the server certificate by default. An ADO connection string without `Encrypt` previously gave login-only TLS, so SQL, parameters and results crossed the network in clear unless the server forced encryption ([#538](https://github.com/Dastari/graphql-orm/issues/538), [#460](https://github.com/Dastari/graphql-orm/issues/460)). The ADO constructors now refuse `Encrypt=false` and `TrustServerCertificate=true`, including `Encrypt=true;TrustServerCertificate=true`, unless the new `MssqlTransportPolicy` opt-ins `insecure_allow_login_only_encryption()` or `insecure_allow_unverified_certificate()` are set in code. They always refuse `Encrypt=DANGER_PLAINTEXT`, `TrustServerCertificate=true` together with `TrustServerCertificateCA` (the driver panicked), enabled integrated security (the driver silently used SQL authentication), and unrecognised security keys such as `Encryption` or `Trust Server Certificate` (the driver ignored them). Pools built from a caller's `tiberius::Config` raise its encryption level to `Required`.
- **Fix:** a pooled connection no longer keeps `SERIALIZABLE` isolation after a state-machine transaction. Commit and rollback reset the session in the same batch, and every new connection starts at `READ COMMITTED` ([#453](https://github.com/Dastari/graphql-orm/issues/453)).
- **Fix:** a server, database or login default of `SET NOCOUNT ON` no longer makes every `rows_affected` 0, which made `update_by_id` return `None` and `insert_if_absent` report `AlreadyPresent` after a successful write. The pool sets `NOCOUNT OFF` and `IMPLICIT_TRANSACTIONS OFF` on each connection ([#631](https://github.com/Dastari/graphql-orm/issues/631)).
- **Breaking:** the SQL Server pool has bounds. `MssqlPoolOptions` sets the acquire timeout (default 30 s, then `sqlx::Error::PoolTimedOut`), connect timeout (15 s), optional statement timeout, idle timeout (10 min), maximum lifetime (30 min) and a reset-and-check of connections idle for 30 s. A non-fatal server error resets the connection and keeps it; transport, protocol and timeout failures, fatal server errors and a failed reset close it ([#454](https://github.com/Dastari/graphql-orm/issues/454)). A timed-out autocommit write has an indeterminate outcome. New constructors: `MssqlPool::connect_ado_with`, `connect_ado_external_writable_with`, `with_options`, `with_options_external_writable`, and `MssqlQueryNotificationConnection::connect_ado_with_transport`, which takes a connect timeout (`connect_ado` uses 15 s).
- **Fix:** a transaction that raw SQL leaves open on a pooled connection can no longer make a later ORM commit be reported as successful and then rolled back. A non-fatal server error, including SQL Server's refusal of a batch that leaves its transaction open (error 266), resets the session before the connection is reused. Each ORM transaction also rolls back a leftover transaction before it begins, and a `COMMIT` that leaves `@@TRANCOUNT` above zero fails and rolls back. The session reset is not a full `sp_reset_connection`: temporary tables, session application locks, `SESSION_CONTEXT`, `USE`, `EXECUTE AS` and other `SET` options persist (see the SQL Server reference).

## 0.127.0

ORM runtime/macros **0.127.0**.

- **Fix:** a mutation hook's deliberate public refusal reaches repository callers with its code and message. Retention purge hooks, and a hook that returns an `OrmPublicError` with `?`, previously surfaced as `INTERNAL_ERROR` with "internal error". A refusal whose `code` extension is an ORM public code was already kept by the other repository writes; tests now pin it for every `MutationContext` write method, both hook phases, standalone repository writes, retention purge and generated GraphQL mutations, on SQLite and owned PostgreSQL ([#565](https://github.com/Dastari/graphql-orm/issues/565)).
- **Breaking:** any other hook error, including one with an application-defined code, has the default `INTERNAL_ERROR` message on repository paths. Its text stays a server-side diagnostic. `into_graphql_error()` and generated GraphQL mutations still render it as authored.
- **Breaking (GraphQL clients):** a callback's typed `OrmPublicError` returned with `?` from a hook, row policy or input transform now reaches GraphQL clients with its code and message in generated single-row and bulk mutations and in `notificationErrors`. For example, `Err(OrmPublicError::with_message(OrmErrorCode::Conflict, "seat taken"))?` renders `"seat taken"` with code `CONFLICT`, where 0.126.0 rendered `"internal error"` with `INTERNAL_ERROR`. Only construct such an error with a message that is safe for clients.
- **Breaking (GraphQL clients):** `INTERNAL_ERROR` always renders its default message, whether it comes from a string `code` extension or a typed `OrmPublicError`. A callback error with an explicit `code: "INTERNAL_ERROR"` previously rendered its own message.
- **Fix:** entity-access and retention row-write refusals on repository reads and writes keep their public code, so a denied entity write reads as `FORBIDDEN`. They were previously wrapped as `INTERNAL_ERROR`.
- **Breaking:** the text of a carried public refusal (`sqlx::Error::to_string()`) is the code's default message and code, for example `error occurred while decoding: forbidden (FORBIDDEN)`, instead of the internal `graphql-orm-public:<CODE>` token. A hook that returns `async_graphql::Error::new(error.to_string())` for a refused nested write no longer shows that token to GraphQL clients. Return the error with `?` to keep its `code` extension. Read refusals with `OrmPublicError::from_sqlx`, not by matching text.

## 0.126.0

ORM runtime/macros **0.126.0**, operation catalogue **0.7.3**, AI **0.133.0**,
tool profiles **0.22.0**, router **0.11.0**.

### Dependencies

- **Breaking (public Auth dependency types):** select published Auth `v0.26.0`
  (tag `04c514280d685a8975d63a8a6b9a1e0db11f503f`, commit
  `7d016f6a6bc3e697382e2e05865f596862189240`) instead of `v0.24.0` for ORM
  `auth-agql`, AI and router `auth-agql`. Hosts that pass Auth types must move
  their own selector to the same tag. No other locked dependency changes. The
  workspace does not use Auth 0.25's fixed-audience delegation or 0.26's
  refresh-reuse grace window, so the Auth update itself changes no ORM, AI or
  router behavior. No data migration. Fixes #443. See the
  [migration guide](MIGRATION.md#agql-auth-0260).

### Row policy on generated writes

- **Security/breaking:** generated writes enforce `RowPolicy::can_write_row` on the row they write. Creates, updates, upserts and bulk updates check the post-image inside the transaction, before the `After` hook, on every key shape and on GraphQL, repository and transaction paths, and roll back with `FORBIDDEN` on refusal. Previously GraphQL and single-key repository creates never checked the row, and updates and upserts checked only the existing row, so a client-writable owner or tenant column could create a row outside the caller's write scope or move one out of it. A refusal also blocks the commit of a caller-owned transaction even if the caller ignores the error. `Database::transaction` then returns `TransactionError::Rejected` with `FORBIDDEN`. `compare_and_swap` rolls back its savepoint on any failure after it opens. A generated write that fails after changing rows, for example in an `After` hook, also leaves its transaction uncommittable.
- **Breaking:** an entity that declares a `write_policy` needs a row policy provider in every authorization mode. Without one, its generated creates, updates, upserts, deletes and bulk writes, retention purges and every entity registered for dependent deletes fail with `AUTHORIZATION_MISCONFIGURED` before any lookup. Create-only and append-only entities with only an entity-level `write_policy` are most affected. In `ExplicitPolicyForAllExposedOperations`, updates and deletes previously returned `FORBIDDEN`, and in `LegacyPermissive` these writes were previously allowed.
- **Security/breaking:** generated GraphQL update and upsert payloads release the committed row only when the caller has entity read access and `can_read_row` allows it, as creates already did. When `can_read_row` refuses, the entity field is `null` and `success` stays `true`. When entity read access is denied, the mutation field fails with an error marked `outcome = "committed"`, so entity-level write-only principals that select the entity now get an error. Previously a caller that could write but not read a row received it in full.
- **Security/breaking:** generated GraphQL updates, deletes, `updateMany`, `deleteMany` and dependent-delete steps treat a row the caller can neither read nor write as missing. Single-row writes answer `success: false`, `error: "Entity not found"`, and bulk writes leave the row out of the matched set, its count and the `notify_with` row bound. Previously such a row produced `FORBIDDEN`, so the response revealed whether a hidden row matched an id or filter. For a dependent delete this applies to the root row; a hidden dependent child still makes the whole delete refuse. A readable row the caller may not write still refuses with `FORBIDDEN`, and repository helpers keep refusing every unwritable row.
- **API:** `Database::authorize_write_target` applies the targeted-row rule in custom resolvers, and `Database::ensure_row_write_provider` applies the row provider requirement. Runtime GraphQL mutations are unchanged: they already check the written row, release payloads only to readers and restrict targets to the caller's read visibility. They use `RuntimeWriteAuthority`, not `RowPolicy`, so the row provider requirement does not apply to them. See the [0.126.0 migration guide](MIGRATION.md#row-policy-on-the-rows-generated-writes-target-write-and-return) and [row policy on generated writes](docs/reference/graphql-orm/strict-authorization.md#row-policy-on-generated-writes).

### PostgreSQL row-level security

- **Fix (security):** `validate_target` no longer reports `Error` drift for RLS policies a plan has just applied. It compared the declared predicate text with PostgreSQL's re-rendered `pg_policies` expression, which drops identifier quotes and adds casts and parentheses, so generated `tenant`/`owner` policies always read as drifted (#590). It also stripped grouping parentheses on search-document tables, so `a OR (b AND c)` matched `(a OR b) AND c`. Validation now resolves each table as the runtime does, then pins `search_path` to `pg_catalog, pg_temp` inside a rolled-back transaction and has PostgreSQL parse both the declared predicate and the live expression against the table, as the `WHERE` clause of a temporary view, and compares the two renderings. With only the catalog on the path, every object outside it renders schema-qualified, so the renderings agree only when both name the same objects with the same structure; neither the session's `search_path` nor a lookalike function or operator in a writable schema can make them agree. An expression PostgreSQL cannot render, including a custom predicate that names a non-catalog object without its schema, is reported as an `Error` instead of matching. Rendering needs the `TEMP` privilege, which PostgreSQL grants to `PUBLIC` by default, and the temporary views fire DDL event triggers and `log_statement = 'ddl'` logging.
- **Security/breaking:** `validate_target` reports as errors, on RLS entity and search-document tables (#525):
  - any policy outside the declared set, including `FOR ALL` and restrictive policies; extra permissive policies are OR-combined with the declared ones;
  - a declared policy with another command, `RESTRICTIVE` mode, or roles other than `PUBLIC`;
  - a missing RLS helper function in `graphql_orm`, or one whose body, result, language, volatility, `SECURITY DEFINER`, `STRICT` or `SET` options differ from the plan's.

  Tables are resolved by the path the plan's SQL uses, so `audit.logs` and `public.audit_logs` no longer shadow each other in policy, enablement and enforcement checks. A deployment with hand-made policies on RLS entity tables now fails validation; fold them into the entity's `#[graphql_rls]` predicates or drop them.
- **Fix:** plans drop the generated policy of every operation `#[graphql_rls]` no longer declares, so removing `update(...)` removes its policy on re-apply (#525).
- **Breaking:** `#[graphql_rls]` `tenant` and `owner` must name a stored column of the entity exactly as declared, quotes included for a quoted `db_column`; anything else is a compile error. Generated predicates write a plain column bare, as the ORM's DDL does, quote any other value in full as one identifier, and wrap each condition in parentheses, so a value that is not a plain column name cannot change the predicate (#527).
- **Fix:** search-document policies, their base-table lookup and the `graphql_orm_search_delete` trigger render table names as the ORM's DDL and writes do. For an unquoted mixed-case table such as `table = "Docs"` the plan referenced `"Docs"` and `"__graphql_orm_search_Docs"` instead of the folded `docs` and `__graphql_orm_search_docs` the ORM creates, so applying failed, or bound to another table of that exact name. Generated `tenant`/`owner` columns and the search key column follow the same rule, and validation compares trigger targets by the folded name (#605).
- **Errors:** a PostgreSQL `42501` (`insufficient_privilege`) error, including a row-level security `WITH CHECK` refusal, maps to `FORBIDDEN` instead of `INTERNAL_ERROR` (#527).
- **Docs:** custom predicates that turn a missing request value into `true`, such as `COALESCE(..., true)` or `IS DISTINCT FROM` tests, are documented as open when the context is missing.

### MSSQL writes

- **Fix:** MSSQL compare-and-swap works on tables with an enabled trigger. It ran `UPDATE ... OUTPUT INSERTED...` without `INTO`, which SQL Server rejects on such tables, so every versioned compare-and-swap against them failed and poisoned the transaction. It now runs the `UPDATE` and, in the same batch, reads the row back by key and by the version it wrote when `@@ROWCOUNT` is 1. The update keeps its exclusive row lock until the transaction ends, and the returned row includes any change an `AFTER` trigger made. An `INSTEAD OF UPDATE` trigger that suppresses the update yields `Conflict` rather than the unchanged row, and a trigger that rewrites the version column yields `Conflict` and rolls the swap back. ([#517](https://github.com/Dastari/graphql-orm/issues/517))
- **Fix/behaviour:** MSSQL `rows_affected` reports the executed statement's own count. It summed every count SQL Server sent, including those from statements inside triggers that do not set `NOCOUNT`, so an audit trigger made a one-row insert, update or delete look like several rows and fail the generated write, and made an `insert_if_absent` that matched nothing report an insert. A trigger's counts arrive before the count of the statement that fired it, and the last count is now used. For a raw batch of several statements, `rows_affected` is now the final statement's count rather than the sum, so a batch ending in DDL or another statement that reports no count yields 0. Under an `INSTEAD OF` trigger it is what SQL Server reports for the replaced statement. ([#459](https://github.com/Dastari/graphql-orm/issues/459))
- **Tests:** the owned MSSQL lane runs generated single-row, compare-and-swap, upsert, bulk, bounded, composite-key, transactional and GraphQL writes against tables with counting `AFTER` triggers, and compare-and-swap against `INSTEAD OF UPDATE` triggers that perform or suppress the update.

### Generated entities and attributes

- **Fix:** an entity without `default_sort` orders by its primary key: every `#[primary_key]` column in declaration order, using `db_column` names, or the `id` field's column when no field declares `#[primary_key]`. It previously ordered by a literal `id`, so `default_order()`, `fetch_all`, generated connections and keyset pages failed at runtime with "no such column" when the key had another name. An entity keyed by `id` keeps its exact default. An explicit `default_sort` is unchanged. Fixes #404.
- **Fix:** a relation without `from` uses the primary-key field as its source in `RelationMetadata` and in `on_delete = "set_null"` validation, as the generated loaders already did. It previously named `id`, so such a relation on an entity with another key did not compile. An upsert target may name the whole composite primary key, and the implicit `id` key matches its `db_column`. A target naming only the first column of a composite key is now rejected, because that column alone is not unique.
- **Fix:** combined `#[graphql(...)]` and `#[serde(...)]` options are read in any order. An option the ORM does not read, written as `key = value` or `key(...)`, no longer hides the options after it: a field's `name` after `deprecation = "…"` or `validator(...)`, a field's serde `rename` after `skip_serializing_if = "…"`, an entity's serde `rename_all` after `tag = "…"`, and `complex` after any such option. Malformed `graphql` metadata is now a compile error instead of being treated as absent. Fixes #192.
- **Fix:** a described custom query or mutation argument with `#[graphql(name = "…")]` and no `desc` gets its generated description merged into a single `#[graphql(...)]` attribute. All its existing options are kept, in order. The macro previously added a second attribute, which failed with ``cannot find attribute `graphql` ``. Fixes #192.
- **Breaking:** `#[backup(...)]`, `#[transform(...)]` and `#[filterable(...)]` reject unknown or malformed options, and `#[backup]` takes one policy. They were previously ignored. See the 0.126.0 migration guide.
- **Fix:** an entity whose create or update input would have no public member now compiles. `GraphQLOperations` previously emitted an empty `async_graphql::InputObject` for it, even under `schema_roots!` `generated_mutations: "none"`, and the build failed. It now omits that input and the roots that accept it: with no publicly writable create field there is no `Create<Entity>Input`, `create` or `upsert`; with no publicly writable update field there is no `Update<Entity>Input`, `update` or `update_many`. `delete` and `delete_many` take no input and are still generated. Reads, relations, `<entity>Changed`, the Rust repository inputs (which keep `skip_input` columns), repository and transaction writes, and hooks are unchanged. An `append_only` entity without a public create field has no public mutation. `mutation_shapes`, `operation_authorization` or `ai_mutations` naming an operation removed this way, and `dependent_deletes` on such an entity, are compile errors that say which input is empty. No entity that compiled before changes. See [read-only and zero-input entities](docs/reference/graphql-orm/macros-and-attributes.md#read-only-and-zero-input-entities) (#228).
- **Feature:** a `#[graphql(complex)]` `SimpleObject` entity with typed computed fields and no relation fields can declare `node_id` and a managed version. Mark it `#[graphql_orm(compose_complex_object)]` and put the handwritten fields under `#[graphql_complex_object]`; `GraphQLEntity` then mounts the generated `NodeId` and `RecordVersion` beside them, with the same encoding, `@key(fields: "NodeId")` and `Changed` snapshot rules as a generated object; `RecordVersion` checks the managed column's read policy. The `SimpleObject` scalar fields of this shape apply column read policies only with `#[graphql_policy_object]`, as before. `GraphQLRelations` is no longer needed for this, and deriving it on such an entity adds nothing. `node_id` on a plain `#[ComplexObject]` entity is still refused, now with a diagnostic that names the composed declaration instead of a missing hidden constant; a `compose_complex_object` entity with `node_id` but no `#[graphql_complex_object]` impl is refused too, so the `NodeId` key is never declared without its field. Fixes #395.
- **Breaking:** a `#[graphql_complex_object]` resolver or derived field named `NodeId`, or `RecordVersion` on an entity with a managed version, is a compile error; it was served in place of the generated field. Remove it. Struct and derived struct fields with these names are refused the same way, and `name` and `rename_fields` values in a composed impl must be string literals. The generated fields are now mounted ahead of handwritten composed fields, including flattened ones, and remain the advertised definitions. A `compose_complex_object` entity with relations and `node_id` whose impl uses plain `#[ComplexObject]` is also a compile error now; use `#[graphql_complex_object]`. See the 0.126.0 migration guide.

### Operation catalogue and Keyset roots

- **Fix/breaking:** the generated operation catalogue now reads each root field's arguments from the generated resolver signature, the same source as the SDL, so the two cannot drift. Update and delete descriptors for an entity with a managed `version` now include the optional `expectedVersion` argument the SDL already had. An authenticated router previously refused the descriptor at startup with `authorization argument metadata is stale`. Catalogue argument descriptions now match the SDL: subscription `key` and `keys` arguments use the resolver text, and the scan root's `where` and `page` arguments are now described in the SDL as well. The hidden mutation field that an entity without generated writes contributes is now `gom{Entity}WritesUnavailable`, one per entity, and is described in the catalogue: category `Unavailable` (new in operation catalogue 0.7.3), the new `GraphqlAuthorizationRequirement::unsatisfiable()` requirement, AI `Prohibited`, and a resolver that refuses every request. Routers accept its descriptor whether or not they exempt underscore root fields, and strict assurance registries record it as exempt. The SDL, and every generated catalogue fingerprint, change: regenerate router descriptors and AI capability manifests, and re-approve pending AI approvals. See the [migration guide](MIGRATION.md#generated-catalogue-arguments-follow-the-sdl).
- **Fixed:** a generated Keyset root no longer stays registered when the host has explicitly declared that its entity's read policy is residual-only. New `RowPolicy::read_visibility_capability` declares, per entity and policy key, whether `read_visibility` can return `Complete` or `Unrestricted`. The default, `Unknown`, keeps today's behaviour. When a policy declares `ResidualOnly` for an entity whose Keyset root is exposed, the new generated `try_schema_builder` and `try_schema_builder_with_limits` return a `KeysetReadPolicyError` naming the root, entity and policy key with both fixes, and `schema_builder` and `schema_builder_with_limits` panic with it. They also refuse a Keyset root whose entity metadata is missing. Fixes #396.
- **API:** `Database::keyset_read_policy_diagnostics` reports exposed Keyset roots whose entity is still `Unknown`, so hosts can log them or make the check strict. `Database::validate_keyset_read_policies` runs the registration check for schemas built directly with `Schema::build`, which skip it. Operation catalog descriptors record `GeneratedGraphqlQueryShape` through `query_shape()`, so Keyset roots are told apart from bounded Scan roots structurally. Catalogue fingerprints are unchanged.
- **Behaviour change:** a GraphQL Keyset request refused for `CallbackOnly` or `Prefilter` read visibility keeps code `AUTHORIZATION_MISCONFIGURED`, but now carries `KEYSET_READ_VISIBILITY_REFUSAL_MESSAGE` and the `reason` extension `KEYSET_REQUIRES_COMPLETE_READ_VISIBILITY`, pointing to a bounded scan or a `Complete` declaration. Neither says anything about the policy itself. See the [0.126.0 migration guide](MIGRATION.md#keyset-read-visibility-declarations-and-diagnostics).

### AI capability discovery

- **Fix (AI 0.133.0, tool profiles 0.22.0):** capability discovery applies current host policy before its result ceiling, so a restricted principal receives the highest-ranked candidates it may use instead of few or none. When policy calls succeed, policy work no longer depends on which candidates are permitted. Batch decisions are bound to each ranked entry's identity, the default runs at most 16 policy calls at once, and `with_maximum_discovery_authorizations` optionally refuses over-broad queries. One search can make up to the index set's `maximum_entries` policy calls. Hosts can override the new provided `AiCapabilityAuthorityPolicy::authorize_discovery` to decide the batch in one lookup. `AiCapabilityIndexSet::ranked_matches` returns the complete ranking. No schema or data migration change. Refs #551. See the [AI migration guide](crates/graphql-orm-ai/MIGRATION.md#capability-discovery-applies-policy-before-its-result-ceiling).

### Router

- Router 0.11.0 turns introspection off by default, refuses root fields without
  authorization metadata, restricts values substituted into scope templates to
  one plain segment, and splits WebSocket admission into pending and
  authenticated budgets. See the [router changelog](crates/graphql-orm-router/CHANGELOG.md)
  and [router migration guide](crates/graphql-orm-router/MIGRATION.md).

## 0.125.0

ORM runtime/macros **0.125.0**.

- **Security/breaking:** PostgreSQL change-journal entries are recorded by the database instead of the runtime role. With the `change-journal` feature, `plan_schema_target` and new `Database::ensure_change_journal_recorder(&entities)` create `graphql_orm.record_change_journal_entry()` as a `SECURITY DEFINER` function with `search_path = pg_catalog, pg_temp`, revoke its `EXECUTE` from every role other than its owner, and install an `AFTER INSERT OR UPDATE OR DELETE` row trigger, `graphql_orm_change_journal`, on every single-key table. Each trigger's arguments record the journal's schema and the table's own schema and name when it is created. The recorder raises unless it fires as that trigger, on that table, keyed by the table's sole primary-key column, on a non-temporary table its owner owns, through a binding in the owner-maintained registry `graphql_orm.change_journal_tables`, which fixes the entity name, key column and table identity for each journal schema and declared table. Plans write each binding by rendering the declared path exactly as the ORM's writes do, so every naming form binds to the table the ORM writes: `Users` to `users`, `"Users"` to `"Users"`, `app."a.b"` to that table. A bare name must resolve in the journal's schema, and the plan fails for a bare-named table elsewhere. Only the owner can write the registry; every role can read it, and plans grant `USAGE` on schema `graphql_orm` to `PUBLIC` so every runtime role can, including in deployments without RLS entities. It also raises unless the journal it appends to is owned by the same role, carries no trigger or rule, and grants `TRIGGER` to no other role; it locks the journal before checking for triggers. Plans drop any trigger that calls the recorder under another name on tables the applying role has the owner's privileges on. Partitioned journals and partitioned entity tables are not supported and fail closed. The journal requires PostgreSQL 13 or later. The entry's key comes from the changed row and its action from the operation, so every entry corresponds to a row change in the recording transaction. ORM write transactions on a journal-enabled `Database` turn recording on with a transaction-local setting, which selects transactions but cannot name an entity, key or action. Restore imports set it `off`, so they never journal, even under a role default. Previously `record_change_journal_event` inserted entries from the mutation transaction, so the runtime role needed `INSERT` on the journal.
- **Security/breaking:** the journal now has RLS enabled and no policies, and plans revoke every table or column privilege other than `SELECT` that any role other than its owner holds on it, `PUBLIC` included, raising and rolling back if one survives. A role subject to RLS can neither read nor write entries, and no other role can add the trigger that would run as the owner when the recorder appends. Plans drop the `INSERT ... WITH CHECK (true)` policy that 0.120.0 added; `ensure_change_journal_table` drops it when the caller owns the journal. `SELECT` grants are kept: they cannot write or add triggers, and read nothing unless the role also has `BYPASSRLS`, as a dedicated export role does. There is no mixed-version window: stop 0.124.x writers, apply as the owner, revoke what remains, validate, then start 0.125.0. See the 0.125.0 migration guide.
- **Breaking:** a PostgreSQL ORM write with the journal enabled fails, and rolls back, unless:
  - the table carries an intact recorder trigger: enabled for ordinary sessions, of the exact type, the table's only trigger calling the unmodified recorder (owned by the table owner), and naming the table, entity and key;
  - the journal carries no trigger or rule;
  - the current role, unless it acts as the journal owner, holds no table or column privilege on the journal beyond `SELECT` and no `EXECUTE` on the recorder, directly, through `PUBLIC` or through any role it is a member of, `NOINHERIT` memberships included.

  The error names the reason, including a transaction with recording switched off, a `session_replication_role = replica` session, a missing registry, no `USAGE` on schema `graphql_orm`, and a registry another role can write. Each journal-enabled PostgreSQL write transaction issues one `SET LOCAL` and one catalog check per table it journals, in place of one `INSERT` per changed row. Previously such a write journaled without database setup. SQLite still writes the journal from the ORM, MSSQL and multi-backend builds still do not journal, and composite-key entities are still refused.
- **API:** `MutationContext::check_change_journal_recorder(metadata)` runs that check for one entity, so hosts can fail fast at startup. A role acting as the journal owner (its owner, a member of it, or a superuser) passes the privilege part.
- **Fix:** an update that changes a primary key journals a `delete` of the old key and a `create` of the new one. `changed_at` comes from the database clock (`clock_timestamp()`, whole seconds). `actor_id`, `transaction_id`, `row_hash` and `correlation_id` stay `NULL`, as before.
- **Validation:** `validate_target` reports:
  - any policy on the journal;
  - with the feature, a missing journal, or a missing or modified recorder function;
  - a missing or writable registry, and for each single-key table a missing binding, or one bound to another table than the ORM's writes reach, or to another entity or key;
  - a recorder trigger that records into a journal other than the one the connection resolves;
  - a current role that acts as the journal owner, whether or not the target has RLS entities;
  - any role other than the owner holding a table or column privilege on the journal beyond `SELECT`, or `EXECUTE` on the recorder;
  - any trigger or rule on the journal;
  - any trigger calling the recorder under another name;
  - as a warning, any correctly named recorder trigger on a table that is not a single-key table of the target. It can only record that table's own changes, and nothing drops it, because another schema target may manage it;
  - a missing, disabled, conditional, retargeted or differently owned recorder trigger on any single-key table in the target.

  Static dependent deletes exempt the recorder trigger from their trigger refusal only when it has the ORM's name, fires in ordinary sessions, is an unconditional `AFTER INSERT OR UPDATE OR DELETE` row trigger with seven arguments, and calls the unmodified `SECURITY DEFINER` recorder owned by the table owner. They do not inspect its arguments, the journal or privileges; journaled dependent rows still pass the ORM check, so cascades and set-null dependents journal through the recorder.

## 0.124.0

ORM runtime/macros **0.124.0**.

- **Security/breaking:** runtime GraphQL evaluates operation assurance before request preparation. For every query and mutation root the operation executes, after alias, fragment and `@skip`/`@include` expansion, the request guard applies the same requirement, evaluator and strict or default rules as the resolver. A refused collection root, `node`, mutation or `RuntimeHostField` root is not prepared, so it causes no database read, host `RuntimeReadAuthority::authorize` call, payload or child-layer authorization, write authorization or host resolver call. Previously query roots and `node` read and called the host read authority before the resolver refused them, and mutations authorized their payload and child layers. The resolver still checks as defence in depth and returns the pre-pass refusal with the same code and error shape. Other roots in the operation are unaffected. Fixes #556.
- **Breaking:** `AssuranceRequirementEvaluator::enforce` takes a new `&AssuranceCheck<'_>` instead of an `async_graphql::Context`. The check exposes `data_opt`, plus the root's `kind` and `field_name`, so the same decision can run before any resolver. In a resolver it searches `run_with_data` data, then request, session and schema data, as `Context::data_opt` does; the runtime pre-pass sees only request, session and schema data, so install `AssuranceEnforcement` there. Runtime query and mutation roots are evaluated twice and run only if both evaluations admit, so evaluators must have no side effects. `AgqlAssuranceEvaluator` reads the principal from the check as before. See the 0.124.0 migration guide.

## 0.123.0

ORM runtime/macros **0.123.0**.

- **Fix/breaking:** runtime GraphQL rejects date-time and JSON input whose canonical form cannot be read back, with `invalid_graphql_input` before any write.
  - A `RuntimeDateTime` input is rejected when offset normalization or microsecond rounding moves its UTC year outside 0001 to 9999. For example, `9999-12-31T23:59:59.9999995Z` rounds into year 10000. The leap second `9999-12-31T23:59:60Z` is rejected too, because PostgreSQL stores it as year 10000. Previously SQLite stored `+10000-01-01T00:00:00.000000Z`, which its reader could not parse, so every page containing the row failed for every caller.
  - A `RuntimeJson` input is also bounded by the length of the text SQLite and PostgreSQL return, computed with the serde_json the reader uses. serde_json re-serializes `1e15` as `1000000000000000.0`. PostgreSQL `jsonb` prints numbers without exponents: `1e16` returns as `10000000000000000`, and `1e42` as 43 digits that read back as `9.999999999999999e+41`. Previously only the input text was bounded, so a document of about 20 KB could be stored, return as about 74 KB, and then fail every read of the field. A document containing the largest f64 values, such as `1.7976931348623157e308`, is rejected too: `jsonb` returns them as digits serde_json cannot read, so previously every read of the row failed on PostgreSQL.
- **Fix:** output renders every stored value the reader can decode, without the input limits. Date-times outside 0001 to 9999 that earlier releases stored, such as `+10000-01-01T00:00:00.000000Z` or `0000-12-31T23:00:00.000000Z`, and JSON, bytes or strings longer than the input limits, are returned as stored, including in NOT NULL fields. Previously one such value failed the whole connection or mutation payload. `max_response_bytes` still bounds each response. `RuntimeScalarLimits` now applies to input only.
- **Fix:** keyset cursors over such a date-time can be reopened. PostgreSQL binds a year above 9999 without its sign, and year 0000 and below as ` BC` years.
- **Breaking:** a `RuntimeHostField::argument` default typed with a runtime scalar, directly or in a list, must decode with the composer's scalar input limits. `query_field` and `mutation_field` fail with `invalid_composition` otherwise. Host field arguments and host input-object fields keep the runtime scalar input bounds, except for values inside a list or input-object argument that also uses an omitted, defaulted variable: async-graphql does not validate those, so hosts must bound them themselves. `RuntimeHostField` defaults typed with a host input object, and argument defaults on host objects added with `register()` or `subscription_root()`, are not checked at composition.
- **Breaking:** `RuntimeDateTime::parse` and `parse_exact` return `InvalidValue` for a canonical year outside 0001 to 9999 or the leap second `9999-12-31T23:59:60`. Deserialization still accepts values a backend may hold. New `RuntimeDateTime::is_portable` reports whether a value is in range, and runtime create and update requests refuse to write one that is not.
- Static entity date and JSON fields are unaffected: they are stored without canonicalization or an output bound. See the 0.123.0 migration guide.

## 0.122.0

ORM runtime/macros **0.122.0**.

- **Breaking:** operation assurance now covers every root it can see.
  - Runtime create, update, delete and link mutations, `RuntimeHostField` roots, runtime collection roots, `node` and live subscriptions enforce the installed `AssuranceEnforcement` with their own coordinate. Mutations check before any write, and subscriptions before registration.
  - A mutation missing from the registry is evaluated against the configured interactive mutation default instead of being allowed. Strict mode still refuses it.
  - Federation entity resolvers enforce the entity's get-by-id root (for example `Query.zone`) as well as any classification registered for `Query._entities`.
  - `#[graphql_orm_custom_operations]` installs `DeclaredAssuranceGuard` with the root's own kind on each described root, composing an existing host guard, or the root attribute's guard, after it with `.and`. A described root with more than one `#[graphql(...)]` attribute, a `flatten` or `derived(...)` resolver, or a `rename_fields` root attribute fails to compile. So does a described root whose async-graphql root attribute is missing, renamed through an import alias (for example `#[GqlObject]`), of the wrong kind for `kind`, duplicated, or placed above `#[graphql_orm_custom_operations]`.
- **API:** `RuntimeGraphqlComposer::register_assurance_operations`, `OperationAssuranceRegistryBuilder::{new, register_runtime, contains}`, `OperationAssuranceRegistry::{config, unregistered_mutation_roots, ensure_mutation_roots_registered, ensure_schema_sdl}` and `AssuranceEnforcement::finish_schema`.
  - `OperationAssuranceOrigin` gains `Runtime`; `AssuranceRegistryError` gains `UnregisteredMutations` and `InvalidSchema`.
  - Under strict classification, `RuntimeGraphqlComposer::finish` with enforcement installed through `data` fails with `assurance_unclassified` unless every mutation root is registered and classified. `finish_schema` does the same for static schemas.
- **Upgrade warning:** with a default interactive mutation policy, runtime mutations, host mutation fields and unregistered custom mutations now demand step-up too. See the 0.122.0 migration guide.

## 0.121.0

ORM runtime/macros **0.121.0**.

- **Data safety/breaking:** schema stage application now runs the same step-risk gate as planned migrations. `apply_schema_stages` applies with `ApplyOptions::default()`, so a pending stage containing a destructive step, such as `DropTable` or `DropColumn` for a live table or column absent from the stage target, is refused before any stage SQL runs. The error names the stage version and the step. Previously these steps were applied without any opt-in, so starting an older binary against a newer database, or staging alongside unmanaged host tables, could drop tables, columns and rows. Fixes #442.
- **API:** `SchemaStageRunner::apply_schema_stages_with_options(stages, ApplyOptions)` applies stages with `allow_destructive`, `additive_only`, `dry_run` and `expected_current_schema_hash`, returning one `AppliedMigrationReport` per pending stage. `record_history: false` is refused for stages. `PlannedSchemaStage::steps()` and `highest_risk()` expose the classified steps, and `MigrationRisk` now implements `Ord` from `Additive` to `Destructive`. Custom `SchemaStageRunner` implementations must add the new method.
- **Fix:** `SchemaManager::apply_upgrade` and stage application risk-check every pending stage before applying any, so a stage refused for a destructive or non-additive step no longer leaves earlier stages applied. `apply_upgrade` also checks `expected_current_schema_hash` once against the live baseline before any stage runs. Previously it compared the hash with every stage's planned source, so a multi-stage upgrade applied its first stage and then failed on the second.

## 0.120.0

ORM runtime/macros **0.120.0**.

- **Security/breaking (GHSA-hr7v-hr83-2jj4):** a generated GraphQL resolver that reaches a PostgreSQL `#[graphql_rls]` entity without `DbAuthContext` now fails closed with `AUTHORIZATION_MISCONFIGURED` before database work, as `DbAuthContext` documented, under `DeclaredPoliciesRequired` (the default) and stricter modes. Root queries, search, aggregates, mutations, subscriptions, federation lookups, relation fields, relation filters and relation count ordering into RLS targets, and every entity a static dependent delete touches are covered, including from non-RLS roots. `SystemAccess` in the request data is the named trusted bypass; `LegacyPermissive` keeps the previous behaviour. Repository paths are unchanged.
- **Security/breaking (GHSA-hr7v-hr83-2jj4):** `validate_target` reports `Error` diagnostics when the connection role would bypass RLS: a superuser, a `BYPASSRLS` role, or the owner of a table where `row_security_active` is false (for example `force = false`). New `SchemaManager::validate_rls_enforcement` performs the same check at service start under every schema policy. Healthy deployments validated with a superuser pool now report errors.
- **Security/breaking (GHSA-7gw2-974x-qhrh):** `plan_schema_target` (via new `build_schema_target_rls_plan`) enables and, matching the entity, forces RLS on each RLS entity's `__graphql_orm_search_<table>` with policies tying documents to the visible base row, and installs a `BEFORE DELETE` trigger removing documents while the base row is visible. The trigger records the search table's schema and name when it is created, its function pins `search_path = pg_catalog` and raises when the table is missing, and dropping a search index drops it; binary keys compare in the ORM's `bin:<hex>` encoding. Only privileged roles can read the change journal: RLS with an `INSERT`-only policy (the runtime role can still append), applied by the plan and by `ensure_change_journal_table`; `export_changes` runs with `row_security = off` and fails for roles subject to RLS instead of exporting nothing. `validate_target` covers both tables, including unexpected extra policies. Validation and dependent deletes accept the ORM's search trigger only when it fires in ordinary sessions with the exact type, arguments, target and function definition. Dependent deletes of RLS entities require a `BYPASSRLS` pool; see the PostgreSQL RLS reference.
- **Performance:** PostgreSQL cannot use the search GIN index beneath a row-security policy because `@@` is not leakproof, so searches on RLS entities scan the search table. Measured on 200,000 documents: selective searches about 8 ms → 470 ms, broad searches about 85 ms → 430 ms. Non-RLS entities are unchanged; an index-preserving design is tracked in [#555](https://github.com/Dastari/graphql-orm/issues/555). See the 0.120.0 migration guide.

## 0.119.0

ORM runtime/macros **0.119.0**.

- **Fix:** PostgreSQL and MSSQL migration plans order foreign keys by the target dependency graph. Removed FKs, and retained FKs whose referenced key is removed or replaced (a dropped or replaced unique index, a removed UNIQUE constraint, or a changed primary key or column UNIQUE flag), are dropped before the structural steps. Added and detached FKs are attached after every table, primary key, unique constraint and unique index exists. MSSQL also detaches FKs whose referenced or source columns are altered or dropped, because SQL Server refuses `ALTER COLUMN` on them. PostgreSQL detaches an FK only for a paired type change, where one of its source columns and the corresponding referenced column both change type (such as uuid to text on both sides). The first `ALTER COLUMN TYPE` would otherwise fail with mismatched member types. Re-adding the FK forces a full revalidation of the referencing rows. Every other PostgreSQL column change keeps FKs attached: default and nullability changes, binary-coercible one-sided widenings such as `varchar(50)` to `varchar(100)` (which stay metadata-only), and incompatible one-sided changes, which fail either way. A new table keeps an inline FK when its referenced key already exists at `CREATE TABLE`. Cycles, self references to a separately created index, and references to keys established later in the plan become trailing `AddForeignKey` steps. PostgreSQL managed apply runs the plan in one transaction. Rendered MSSQL plans and migration files carry no transaction or `XACT_ABORT` wrapper, so MSSQL atomicity depends on the host's executor. SQLite keeps its per-table rebuild order, which installs FKs with enforcement suspended and checks them after the last statement. PostgreSQL introspection now treats an index as constraint-owned only for PK, UNIQUE and EXCLUDE constraints. An ordinary unique index that an FK references stays visible, so replans stay empty. Fixes #409.

## 0.118.0

ORM runtime/macros **0.118.0**.

- **Security (GHSA-f7xj-6695-ww2q):** `order_aggregate` count ordering now counts only target rows the request may read. Generated list and relation resolvers authorize each requested count term before any SQL: the relation field must be readable, the target entity's read access is checked, and the target's complete SQL row visibility is ANDed into the correlated `COUNT(*)`, as for `relation(filter)`. Callback-only and prefilter (residual) target policies fail closed with the sanitized `AUTHORIZATION_MISCONFIGURED` error; a denied target fails with `FORBIDDEN`. Previously a caller who could list parents could order by the count and infer how many unreadable children each parent had. PostgreSQL RLS deployments covering the target were not affected.

- **Breaking:** `order_aggregate` on a relation with a `read_policy` is now a compile error, because a per-record field policy cannot be enforced on an `ORDER BY` count. Generated order inputs with count terms gain a hidden `__gom_relation_counts` field, so construct them with `..Default::default()`. `DatabaseOrderBy` gains `validate_order` (default `Ok`). An unauthorized count term renders a count of no rows and every `EntityQuery`-based builder (repository, projection, write-hook and fallback search queries) refuses it, so programmatic use of a count order now fails instead of returning unfiltered counts. See the 0.118.0 migration guide.

- **Audit:** relation `totalCount`, relation filters, grouped aggregates, search counts and runtime-IR relation predicates and counts already applied target visibility. The runtime IR has no relation-count ordering.

## 0.113.0

ORM runtime/macros **0.113.0**.

- **Fix:** SQLite introspection matches the reserved `sqlite_` prefix literally and case-insensitively. Populated lookalikes such as `sqliteXcustomer` and `sqlite-customer` remain visible to migration planning; SQLite internal tables remain excluded. Fixes #416.

- **Upgrade warning:** newly visible lookalikes such as `sqliteXcustomer` are treated like other unmanaged tables: default strict plans propose `DROP TABLE` when they are absent from target entities. Default apply refuses, but `allow_destructive: true` deletes the tables and rows. Use `managed_tables_only()` or rename those tables before upgrading, and review plans for renamed unmanaged tables too. The general ownership safeguard is tracked in [#441](https://github.com/Dastari/graphql-orm/issues/441) and is not implemented here. See the 0.113.0 migration guide.

## 0.112.0

ORM runtime/macros **0.112.0**.

- Consumer fixture builds respect the caller’s `CARGO_TARGET_DIR`, sharing its `fixtures` subdirectory; the default remains workspace `target/fixtures`. Fixes #407. No runtime or macro behavior changes.

## 0.111.0

ORM runtime/macros **0.111.0**.

- **Data safety/breaking:** column/check/FK SQLite rebuilds now patch live DDL through the transactional preservation executor. Unchanged generated columns, collations, FK actions, indexes/triggers, rowids and AUTOINCREMENT history survive. Ambiguous targeted clauses, unsupported physical tables/dependencies and reserved-name collisions refuse with sanitized errors before mutation. Mixed search-index/enforcement changes require separate plans. Previously successful lossy rebuilds may now refuse. Exact primary-key rewrites are permitted only for transactionally proven empty tables without incoming foreign keys or AUTOINCREMENT, retaining empty legacy placeholder migrations. Fixes #412.

## 0.110.0

ORM runtime/macros **0.110.0**.

- **Tests:** validate the MSSQL composite-key lookup SDL structurally, preserving ordered `legacyObjectType`, `refNo`, and `lineNum` non-null `Int` arguments and the nullable `LegacyLabourEntry` result while tolerating argument descriptions. CI now executes this test target in the MSSQL backend lane. Generated SDL and runtime behavior are unchanged. Fixes #118. No data migration.

## 0.109.0

ORM runtime/macros **0.109.0**.

- Refresh standalone fixture lockfiles for router 0.8.3, including federation
  and GraphQL feature checks run with `--locked`. No runtime or macro behavior changes.

## 0.108.0

ORM runtime/macros **0.108.0**.

- **Fixed:** normalize PostgreSQL composite UNIQUE declarations subsumed by the same ordered primary key, so annotation-only changes remain migration no-ops. SQL Server retains its separate constraint; reversed key order remains a real difference.

- **Breaking/API:** add `SetCompositeUniques` to migration steps. Managed migrations now add, remove and change composite uniques; SQLite composite-unique-only rebuilds rewrite the live table DDL and preserve rowids, generated values, collations, AUTOINCREMENT history, indexes, triggers and FK actions; unsupported shapes and mixed changes refuse. Reserved-name collisions refuse without cleanup or unproven drops, retaining the diagnostic-only preparation from #417. Other pre-existing rebuild operations are tracked in #412. Duplicate data refuses the change atomically with a sanitized error. Fixes #393.

ORM runtime/macros **0.107.0**.

- **Breaking/security:** add required `has_source_condition` and `has_target_condition` flags to `RelationMetadata`. Both static-to-runtime conversion entry points refuse conditional relations with field-scoped `UnsupportedCapability` diagnostics: the runtime relation IR cannot represent their guards/filters exactly. Ordinary unconditional relations retain their behavior. Fixes #427.

ORM runtime/macros **0.106.0**.

- **Data safety:** SQLite migration preparation no longer drops tables based on a wildcard name guess. Literal reserved-name candidates without ownership evidence are preserved with a diagnostic; unrelated migrations continue, while a conflicting rebuild refuses. Interrupted transactional rebuilds still roll back atomically. Fixes #414.

ORM runtime/macros **0.105.0**.

- **Breaking/security:** add effective `is_readable` to `RelationMetadata`. Default static-to-runtime conversion refuses hidden relation declarations with field-scoped `UnsupportedCapability` diagnostics, independently of key/target visibility or host authority. Trusted repository conversion retains them only behind the existing non-served `repository_only` marker. Generated relation implementation skips remain distinct from user output restrictions. Rebuild matching derives and regenerate stored catalogs; no data migration. Fixes #399.

- **Security:** prove relation getter ownership using witnesses from exposed generated output. Delegated/handwritten complex objects cannot reopen skipped relations without an exposed generated getter; unmounted composed wrappers fail closed. Cover the reviewer's permissive-authority reproduction, generated output controls and schema-only/composed paths. Audited scalar ownership: complex scalar output already honors delegated skips; regression coverage retains that protection.

ORM runtime/macros **0.104.0**.

- **Fixed:** entities with an empty generated order input now receive a macro-owned diagnostic naming the entity and recommending `#[sortable]`, instead of async-graphql empty-input and cascading trait errors. The diagnostic covers incoming relation consumers as well as operation roots. Existing sortable schemas, catalogues and primary-key keyset ordering are unchanged. Fixes #401.

ORM runtime/macros **0.103.0**.

- **Breaking/security:** add `is_readable` to `ColumnDef` and `FieldMetadata`; derives retain `private`, `read = false`, and `input_only` restrictions plus effective persisted scalar output eligibility. Delegated SimpleObject/complex output honors effective `skip`, `skip_output`, flattening and unrepresented guards/visibility; ORM-owned getters retain their actual behavior. Metadata-only repository declarations retain ordinary readable columns and conservatively honor explicit output restrictions. Static-to-runtime conversion rejects unreadable columns with scoped `UnsupportedCapability` diagnostics, before a host authority can expose direct, related, or aggregate reads. Ordinary readable schemas are unchanged. An explicit `from_static_entities_for_repository` conversion retains internal columns behind a serialized, fingerprinted `repository_only` marker that runtime GraphQL compilation always refuses. Runtime schema literals require that new marker. AI 0.125.0 adopts this path for bounded session activity with private keys. Fixes #390.

ORM runtime/macros **0.102.0**.

- **Security/parity:** ordinary, single-key append-only and composite-key append-only GraphQL creates now emit the same resolver pipeline. `before_create` runs once with GraphQL request context before field-write authorization of transformed inputs; mutation hooks also receive the request context. Transaction failures use ordinary structured GraphQL error mapping. Configured create notifications run after commit and output authorization, with no notification on rollback. Fixes #408.

- **Breaking/security:** append-only GraphQL creates enforce declared field-write policies using the ordinary create checks. Strict mode refuses missing field providers before inserting. Ordinary and append-only creates now share entity/row read authorization before disclosing committed output: missing read providers fail closed, entity denial refuses output, and row denial withholds the entity. Output errors identify the committed outcome and forbid retry. Fixes #408.

- **Fixed (ORM/macros 0.102.0):** append-only composite-key entities generate one create-only mutation surface with complete-key readback and Changed event identity. Fixes #394.

ORM runtime/macros **0.99.0**.

- **Security:** bounded static dependent deletes repeat full physical catalogue validation after hooks before every prepared write and before commit, comparing the physical schema snapshot to the verified plan. Trigger, FK, cascading-table and other invalidating catalogue changes refuse with sanitized errors and atomic rollback, including changes between writes. Authorization, bounds and after-commit events remain intact. Fixes #391.

ORM runtime/macros **0.100.0**.

- **Fixed:** the shared CacheSync/generated Changed authority timer recognizes locked agql-auth service/store failures (`AUTH_SERVICE_UNAVAILABLE`) and host configuration failures (`INVALID_CONFIGURATION`) as sanitized `SUBSCRIPTION_CONTINUITY_LOST`, including source-free extended errors. Unknown codes remain fail-closed denials. No data migration.

- **Security/breaking:** CacheSync rechecks partition authority on the Database's independent, bounded jittered subscription timer. Event failures silently suppress initial/committed/gap signals; only timer failures emit sanitized `FORBIDDEN` for definite denial or `SUBSCRIPTION_CONTINUITY_LOST` for infrastructure failures. `changes_with_authority` separates subscriber checks from event filtering; existing `changes` uses its callback for both. Denied reconciliation stays pending until a later authorized signal. No data migration; macros stay version-aligned. Fixes #384.

ORM runtime/macros **0.98.0**, operation catalog **0.7.2**.

- **Breaking:** reject `private`, `read = false`, and `input_only` fields on `GraphQLEntity` + `SimpleObject` entities unless skipped from the object; use `#[graphql(skip)]` or `graphql_policy_object`. Fixes #292.
- **Breaking/security:** a readable relation is a compile error if any local or target key or source/target condition column is unreadable (`private`, `read = false`, `input_only`), including composite keys and cross-crate targets. Unresolved target physical columns (including self-targets) and targets without column-readability metadata also fail compilation. Make the relation unreadable or its columns readable; regenerate older targets or explicitly implement the metadata contract for hand-written targets.
- **Defence in depth:** `GraphQLEntity(schema_only = true)` also emits the target-column assertions backed by hidden `GeneratedRelationReadability` and `generated_relation_column_is_readable`. Readable relations to unreadable target columns fail compilation on this path too; valid combinations with `GraphQLRelations` remain supported. No data migration is needed.
- **Breaking/security:** unreadable primary keys with `GraphQLOperations` are compile errors because Changed events expose them. `search_relation` on an unreadable relation, `order_aggregate` on an `input_only` relation, `searchable` on an `input_only` field, and `search_json` on `read = false` / `input_only` fields are compile errors. Remove the declaration or make the field readable.
- **SDL/security:** remove `input_only` relation getters, aggregate/group-by fields and semantic catalogue entries. Automatic read surfaces share the same readability predicate; explicitly granted scalar filters/sorts on `read = false` remain independent capabilities.
- **Compatibility:** operation catalog 0.7.2 preserves legacy default descriptions/fingerprints for 0.95/0.96 consumers; 0.98 macros explicitly provide corrected descriptions.
- **Security:** internal journal and result SQLx adapters now preserve safe error categories and operator reporting without publishing raw diagnostics. Fixes #354.
- **Fixed:** generated single-record descriptions use singular wording, and bounded scans share the exact SDL description with their catalogue entries; SDL and derived fingerprints change. Fixes #289. No data migration is needed.

ORM runtime/macros **0.97.0**.

- **Added (ORM/macros 0.97.0):** opt-in bounded static GraphQL dependent deletes run declared cascade and set-null writes through dependent GraphQL authorization and hooks in one transaction, with Changed events after commit. SQLite/PostgreSQL single-column keys only; unsupported dependencies refuse. Dependents enforce their own assurance coordinates (including strict mode), and catalogue validation refuses incompatible FK types/affinities/collations before both discovery and referencer rechecks. Root row admission precedes graph locks, binds the exact registered entity type, and preserves direct not-found/conflict errors. Declared non-`multiple` and `multiple, emit_fk = true` cascade/set-null parents must opt in through a cross-crate compile-time assertion. PostgreSQL inheritance, partitions, rewrite rules, views and foreign tables refuse in both planning and write rechecks. Hook DDL adding triggers, FKs or cascading tables is unsupported and not detected; the PostgreSQL per-write guard covers relation kind, inheritance/partitions and rules only (Related to #391). Database-only FKs under opted-out parents still delete children physically without their hooks/events; refusal/success inherently reveals unwritable dependence. AI effect-policy coverage is tracked separately in #389. No data migration is needed. Fixes #283.

AI **0.124.0**, ORM runtime/macros **0.96.0**, tool profiles **0.21.1**, operation catalog **0.7.1**.

- **Breaking (ORM/macros 0.96.0):** generated static/derived Changed operations admit exactly one root occurrence, counting aliases, fragments and skipped fields, before opening sources or reading records (`subscription_root_limit_exceeded`). Hand-built schemas must install `GeneratedSubscriptionAdmission`; missing or misconfigured admission refuses with `SUBSCRIPTION_ADMISSION_REQUIRED`. Subscriber authority is rechecked independently of events on a configurable timer: a 30-second base by default, clamped to 1–300 seconds, plus 0–10% jitter. Definite denial ends with one sanitized `FORBIDDEN`; typed host/DB failures and infrastructure categories end with `SUBSCRIPTION_CONTINUITY_LOST`. Per-event subscriber, row, key, field and provenance denials stay silent; unlisted/action-filtered events do no entity-policy work. Stream polling, host latency and backpressure can delay timer delivery; closure/continuity signals retain their existing timing. Split multi-root operations, install admission on hand-built schemas, and handle reauthentication versus resubscription. No data migration. Runtime-IR denials and CacheSync remain unchanged (Related to #384). Fixes #375. Fixes #276.
- **Fixed (ORM/macros 0.96.0):** discard host-policy diagnostics buffered during every dropped Changed event, including row, field and entity denials and delivery-fence failures. Hidden-write diagnostics can no longer attach to a later visible event or lag frame. Lag itself still reveals shared-channel capacity loss before filtering, including from hidden write volume (Related to #387). No data migration.
- **Fixed (ORM/macros 0.95.1):** in-memory SQLite helpers disable pooled connection retirement and retain a shared-cache keeper so replacement connections see the same schema and rows. Memory pools remain isolated unless the URL explicitly opts into `cache=shared`. No data migration is needed. Fixes #376.
- **Tests (router 0.8.2):** wire children bind port zero and report their owned public/admin addresses, eliminating released-port races. The executable smoke test transfers an already-bound socket through the additive `run_with_listener` hook and Unix `--listen-stdin` option; default startup is unchanged. Fixes #366.
- **Tests (ORM/macros 0.95.1):** OS-backed coordination prevents concurrent trybuild projects from truncating a live generated lockfile; timeout sanitization setup uses normal deadlines and blocked acquisitions use virtual time. Fixes #380. Fixes #381.

- **Breaking (ORM/macros 0.95.0; AI 0.124.0 fixture compatibility):** runtime collections gain optional `node_identity` and `live_subscriptions` declarations. Opted-in objects preserve `id`, expose canonical `NodeId` and managed `RecordVersion`, join local `node(NodeId:)`, and provide bounded keyed `Changed` streams through a host committed-event source. Every event rechecks the current registration and delivery grants together; a revoked grant withholds events quietly, matching derived subscriptions. Deleted images default-deny and are explicitly unsupported on native-RLS profiles; a loaded NULL in a nullable field counts as image evidence, an unloaded field does not. Queue loss, restart and revision signals use the existing uppercase continuity frames without keys or counts, independent of which keys were ever visible. A subscription operation selecting a generated live root must select exactly one root field, or it is refused with `subscription_root_limit_exceeded` before any source is opened. Existing version columns need no data migration; ordinary migrations apply when a host adds one. Serialized opt-out schemas, their fingerprints and derived APIs are unchanged. Rust code building `RuntimeCollection` literals must add the two fields. Fixes #368.
- **Fixed (ORM/macros 0.95.0):** searchable runtime list operations now describe `search: RuntimeSearchInput` in their semantic catalog, so runtime-schema hosts can compose them for the AI query compiler. Non-searchable lists and relation pages are unchanged. The single-backend runtime GraphQL example is also gated out of mixed SQLite/MSSQL builds with `runtime-graphql`; CI now checks and lints that configuration.
- **Added (AI 0.124.0):** optional, typed and bounded Codex read-marker tolerance; default wire bytes and strict admission are unchanged. Unknown effect-capable methods remain refused. See the AI migration guide for opt-in, method-only tracing and counters, and version readiness (related to #333). No data migration. Fixes #369.
- **Tests (AI 0.124.0):** media fixtures sharing the bounded decoder pool are isolated, so CPU contention no longer turns unrelated tests into overload tests. Production busy-pool refusal, authorization, budgets and effect replay rules are unchanged. Fixes #367.

AI **0.123.0**, ORM runtime/macros **0.94.0**, tool profiles **0.21.1**, operation catalog **0.7.1**.

- **Breaking (ORM/macros 0.94.0):** generated single-row mutations and default propagating `notify_with` failures now mask callback errors that have a source and no string `code`, matching bulk. Generated entity and field policy callbacks and single-row row-read callbacks follow the same rule. Authored refusals keep their message and extensions. Fixes #328.
- **Fixed (ORM/macros 0.94.0):** a paginated relation returned `INTERNAL_ERROR` when its target had a column read through a SQL expression, such as MSSQL `mssql_money`. This was a regression in 0.92.0 and 0.93.0 from the narrow relation window. Fixes #265 (found by restoring the native MSSQL lane).
- **Verification (ORM/macros 0.94.0):** native MSSQL integration coverage restored and backend-specific fixtures gated, so PostgreSQL, MSSQL and SQLite with MSSQL compile and lint for all targets. CI and release run the complete owned MSSQL lane and the receiver admission tests. Fixes #89. The SQLite compatibility tests now serialize on the shared query counter. Fixes #360.
- **Breaking (ORM/macros 0.94.0):** `GraphQLRelations` without `#[graphql(complex)]` on an entity with relation fields reports one error naming the fix, instead of three conflicting-implementation errors. A `GraphQLSchemaEntity` with relation fields now needs `schema_only = true`. An entity with `node_id`, a managed `RecordVersion` and a generated collection relation has one documented, tested declaration.
- **Docs and tests (ORM/macros 0.94.0):** `tests/link_relation_patterns.rs` compiles and runs link-table patterns: string-kind `source_condition` relations, `#[relation(..., multiple, filter)]` with `inList`, and a nested create over `Database::transaction`. The filter example in the docs used `in`; the member is `inList`.
- **Added (ORM/macros 0.94.0, catalog 0.7.1, tool profiles 0.21.1, AI 0.123.0):** opt-in `receiver-admission` admits one exact registered named query against a token's signed purpose claim, before scopes or request context are available. A token carrying that claim cannot skip admission through the ORM's identity paths. Private-remote hashes keep their encoding, and credentials without the claim are unchanged.
AI 0.123.0 adds opt-in authoritative Codex exact-version readiness and fresh launch admission, preserving strict cleanup during refusal. Related to #333. No data migration or persistent schema change.

AI **0.122.0**, ORM runtime/macros **0.93.0**, tool profiles **0.21.0**.

- **Docs and tests (ORM/macros 0.93.0):** `MutationContext::defer` is the after-commit callback for mutation hooks and `Database::transaction`. It already existed, so there is no new API and no behaviour change. It now has rustdoc, a documented ordering (commit, `Changed` events, deferred actions, then `notify_with`), and tests for each write path, for never running after a rollback, and for failures reaching `PostCommitErrorHandler`. The new `defer-after-commit` lane in `scripts/run-owned-database-lanes.sh` runs them on SQLite, PostgreSQL and MSSQL. The usage guide and write-path reference describe the "durable job in the transaction, wake a worker after commit" pattern and say the callback is at most once and lost by a crash after commit. No migration is needed. Fixes [#338](https://github.com/Dastari/graphql-orm/issues/338).
- **Breaking (ORM/macros 0.93.0):** run repository insert transforms once and bulk update transforms once per existing row, retaining independent row outputs and atomic refusal. Correct `InternalMutationHook` to identify hook execution only. No data migration. Fixes [#284](https://github.com/Dastari/graphql-orm/issues/284).
- **ORM/macros 0.93.0:** `#[graphql_entity(mutation_shapes = "create,update")]` selects which generated mutations an entity exposes (`create`, `upsert`, `update`, `update_many`, `delete`, `delete_many`), like `query_shapes` for queries. A disabled mutation leaves the SDL and the operation and semantic catalogues, with the input and result types only it used. Repository writes, relations, hooks, `Changed` subscriptions and authorization are unchanged. Omission keeps every mutation with byte-identical SDL, so nothing changes unless you opt in. An unknown, repeated or empty selector, or a category the entity does not generate, is a compile error. Fixes #286.
- **Behaviour change (ORM/macros 0.93.0):** child deletes now deliver `PROPAGATED`
  invalidations to authorized current parents for `propagate_change = "up"` (#290).
  Parent-only readers receive them; deleted child identities and values are omitted.
  Bulk deletes emit one event per row/path, matching updates. Remove redundant
  parent-touch workarounds and detect the change via `DELETED`/`PROPAGATED` with
  null `sourceEntity`/`sourceId`. No data migration is needed.
- AI 0.122.0 / tool profiles 0.21.0: preserve proven pre-transport audit failure classification and expose closed GraphQL retry refusal reasons (Fixes #327). Possible provider effects remain uncertain.
- AI 0.122.0: retain ORM-backed regression coverage for added tools, removed tools, changes to tool schemas, and changes to unused tools across retained session cleanup/rebind. The reported stranded-conversation defect did not reproduce; no runtime or persistence change.
- ORM/macros **0.93.0:** reduce generated scalar-filter SQL and semantic metadata construction. Emitted clauses, bind order, NULL handling and SDL are unchanged. No consumer changes or data migration are needed. Fixes #334.

AI **0.121.0**, ORM runtime/macros **0.92.0**. Released as `workspace-2026.11.06.2`.

AI **0.121.0**, ORM runtime/macros **0.92.0**.

- **ORM/macros 0.92.0:** existing `notify_with` handlers now run once per committed GraphQL bulk row. Search for `notify_with` (or `notify`) on entities using bulk mutations and review handler cost and idempotency. A GraphQL bulk write matching more than 1,000 rows on an entity with a handler used to succeed and now refuses before writing, with `success: false` and `errorCode: INVALID_INPUT`, so split larger batches. Otherwise bulk writes with handlers return committed counts plus nullable `notificationErrors`; do not retry when `success` is true. Single-row callers can opt into `notify_on_error = "report"` and install `PostCommitErrorHandler`. Refresh pinned SDL/client types and initialize `notification_errors` in bulk result struct literals (or use constructors). No data migration is needed. Fixes #330 and #331.
- ORM runtime/macros **0.92.0**: fix [#313](https://github.com/Dastari/graphql-orm/issues/313). Static offset connections use the selection (including fragments and aliases) to omit unused count or row queries. Page probes use one extra row when `hasNextPage` is selected without a total. Residual row authorization still scans readable rows before computing totals.
- ORM runtime/macros **0.92.0**: fix [#306](https://github.com/Dastari/graphql-orm/issues/306). Paginated static relation connections rank only primary keys and parent identities, then join back for page rows. Wide payloads no longer pass through the full partition window; grouped counts, batching, ordering and read authorization are preserved. Relation page row/count statements now participate in query-count instrumentation.
- ORM runtime/macros **0.92.0**: opt-in macro `schema-export-only` replaces generated
  async/Future/Stream operation and relation bodies with panics for dedicated SDL
  builds, preserving schema declarations. Normal expansion is untouched. Fixes #317.
- **Fix (ORM/macros 0.92.0):** preserve application refusal messages and codes in bulk update/delete payloads, including field denials. Bulk results add nullable `errorCode`; refresh pinned SDL. Private typed transport prevents driver text from forging public errors. No data migration. Fixes [#316](https://github.com/Dastari/graphql-orm/issues/316).
- **Breaking (ORM/macros 0.92.0):** generated create, update, upsert and bulk update reject policy-denied field writes before writing, naming the field. `FieldPolicy::ignore_denied_field_write` explicitly restores legacy omission for selected fields; required create/upsert fields remain mandatory. Fixes [#307](https://github.com/Dastari/graphql-orm/issues/307).
- ORM/macros 0.92.0: implicit String timestamp defaults and generated `updated_at` writes use RFC 3339 UTC; integers retain epoch seconds. Preserve legacy stored defaults and rows on reopen. Catalog equivalences apply only during comparison; existing canonical default-expression hash inputs retain their identities. MSSQL introspection now retains NVARCHAR lengths to avoid false reopen plans; regenerate saved MSSQL plans whose physical hashes included these columns. Fixes #308.
- AI 0.121.0: **provider session cleanup now converges.** Real Codex `thread/list` pages carry `readStates`, which its generated schema omits, so the strict absence scan refused every real page on 0.165.0 and 0.166.0. A delete that returned `-32600` ("no rollout found for thread id") could therefore never settle, and retried about once a minute forever. The scan now admits `readStates` as a bounded, discarded map; arbitrary `-32600` is still not success. Backoff also grows, and a retry budget ends in visible `cleanup_exhausted` (fixes #309). Codex cleanup now requires a fresh bounded, content-discarding absence scan even after a successful deletion acknowledgement.
- **Fix:** keyset cursors are now bound to everything they must resume on (#326). Cursors for a configured order were not bound to their entity, so a cursor from one entity with the same key columns resumed on another. `recordsScan` continuations and the `MutationContext::keyset_page` / `keyset_connection_page` methods ignored the caller's filter, so a changed filter resumed silently. All generated paths now share `bind_keyset_fingerprint` and refuse a mismatch with `CURSOR_INVALID`. Protected runtime GraphQL cursors authenticate a digest of the caller's `where` input as well. Cursors never contain the filter, only a hash. Cursors issued before the upgrade are refused once; clients restart from the first page. No data migration.
- Document that value drift while paging is inherent to keyset, and pin it with a test. See [what a keyset cursor is bound to](docs/reference/graphql-orm/pagination-migration.md#what-a-keyset-cursor-is-bound-to).
- Reject related-row filters in every bulk write entry point and fail closed in direct in-memory matching (#314). Qualify generated relation reads on owned MSSQL and PostgreSQL.
- Add opt-in `#[relation(filter)]` collection predicates to static generated `WhereInput` and list, keyset, and aggregate count roots (#314). Parameterized `EXISTS` intersects complete target/link SQL read visibility before paging and counting; empty alternatives match no rows. Residual authorization and spatial combinations fail closed.
- Bind filtered keyset cursors to the caller-visible typed filter input. Previously issued filtered cursors must restart; no data migration is needed.
- ORM runtime/macros **0.92.0** adds `ReadPredicate::column` with persisted-column validation and bound values, and SQL NULL-preserving `ReadPredicate::not`. Private policy columns remain absent from public filters. Keyset shape guidance covers residual-only policies. Fixes #318.
- ORM/macros **0.92.0**: callback-denied to-one relations now resolve like absent targets, without existence-revealing errors; generated relation-field `read_policy` checks fail closed before target loading (#325).
- Fix generated `NodeId` and managed `RecordVersion` on relation-bearing complex entities, including composed objects and Changed snapshots (#270, #288).
- **Test:** the federation end-to-end fixture now queries the generated aggregate root of each of its two aggregate-enabled subgraphs through the router and asserts the exact seeded count, so serving counts from two aggregate subgraphs is proven as well as composing them (#323). The collision itself was fixed earlier by the shareable aggregate result objects (0.82.0). The fixture's zones row policy now also declares its read decision as a complete SQL predicate, because a generated aggregate over a callback-only row policy is refused by design. This adds a test and changes no runtime behaviour; ORM and macros 0.92.0 change no generated code or public API.

AI **0.120.2**, ORM runtime/macros **0.91.1**. Released as `workspace-2026.11.06.1`.

- **Fix:** every Codex 0.166.0 turn reaching a `reasoning`, `dynamicToolCall`, `webSearch` or `sleep` item failed closed with `provider_rejection`, because 0.166.0 adds nested `item.startedAtMs` and `item.completedAtMs`. The actor now admits them as optional, typed, bounded metadata and keeps every other check. It also opts out of `turn/moderationMetadata` and `thread/readState/changed`, which real turns showed on 0.165.0 and 0.166.0. See the [AI changelog](crates/graphql-orm-ai/CHANGELOG.md#01202).
- ORM and macros 0.91.1 change no code. Fixture lockfiles record the AI version, so they need a new package identity to release alongside it.

ORM runtime/macros version: **0.91.0**. Runtime full-text search: ranking isolated from hidden rows, and a fenced backfill. See [runtime search](docs/reference/graphql-orm/runtime-search.md).

- **SQLite ranking is per document.** SQLite now scores a match from the row's own document only: `1.0·nA + 0.7·nB + 0.4·nC + 0.1·nD`, the weighted number of matched phrase instances in each weight, counted with FTS5's built-in `highlight()`. It replaces `bm25`, whose row count, average length and document frequencies covered rows the host's grant hides. That let a caller estimate how many hidden rows contain a term, and made the order of visible rows and `minScore` results depend on hidden rows. Now a visible row's score, the order of visible rows, `minScore` results and `totalCount` do not change when hidden rows are added or removed. PostgreSQL `ts_rank_cd` already depended only on the row and is unchanged. See [ranking isolation](docs/reference/graphql-orm/runtime-search.md#ranking-isolation) for the parity analysis.
  - SQLite scores have a new scale; existing `minScore` thresholds on SQLite need review.
  - Relevance-ordered search cursors bind the scoring function, so one issued before 0.91.0 is refused with `cursor_invalid` on both backends. Clients restart from the first page. Cursors of searches with `orderBy`, and of reads without search, are unaffected.
- **Search in the read check.** `RuntimeReadCheck` gains `search: Option<RuntimeReadSearchCheck>`, with the requested `mode`, whether `minScore` is supplied, and whether results are relevance-ordered. A host can withhold modes, for example `WEB` (which SQLite evaluates like `PLAIN`), by denying in `authorize`. `RuntimeSearchRequest` gains `mode()` and `min_score()`.
- **Fenced backfill.** `backfill_runtime_search` compares the stored index configuration with the supplied schema's declaration in every batch's transaction. On any difference (fields, weights, JSON paths, language, tokenizer, `remove_diacritics` or minimum token length) it returns one `RuntimeMigrationDiagnosticCode::SearchConfigurationMismatch` diagnostic per differing part, and writes nothing. Previously a stale schema could write documents from the old fields and mark a recreated index `ready`. A backfill with the current schema completes and resumes as before.
- **Porter with accent folding.** `remove_diacritics` is accepted with the `porter` tokenizer (`porter unicode61 remove_diacritics 2`), so stemmed English keeps accent folding. It is still refused with `ascii`. `RuntimeSchema::from_static_entities` maps that tokenizer.
- **Test lanes.** CI, the release workflow and `scripts/run-owned-database-lanes.sh` now run the runtime search tests on owned SQLite and PostgreSQL.
- **Breaking for exhaustive matches:** `RuntimeMigrationDiagnosticCode` gains `SearchConfigurationMismatch`. `RuntimeReadCheck` is `#[non_exhaustive]`, so its new member is additive.
- No change to static entity search, its generated API or its storage. No change to persistent schema, migration planning or search storage.

- Repair the locked Tokio, ctutils and zstd-family advisory versions across workspace fixtures. Update the compatible S3 SigV4a owner to use corrected RustCrypto arithmetic. Retain only the owner-accepted, version-specific RUSTSEC-2026-0387 disposition for crypto-bigint 0.5.5 through JWT verification dependencies; no advisory ignore or gate changes. Refresh router binary notice bindings for the corrected lock.

- Advance AI to **0.120.1** so its provider-files documentation update has a new immutable package identity. No AI runtime or persistence change. All other packages with existing current-version tags retain byte-identical source trees; the prepared ORM/macros 0.90.1 versions have not yet been tagged.

- Refresh the lockfile/SBOM-bound router notice review for the approved Linux `auth-agql` archive: 698 dependency components and 11 exact source archives, preserving all six existing source-only notice dispositions and all existing supplemental notice hashes. New component identities since the previous review are Auth 0.24.0 and the already-adopted WebPKI/root distrust patches. No dependency, feature, runtime or schema change is introduced by this evidence refresh.

ORM runtime/macros version: **0.90.1**, including the unpublished 0.90.0 changes below.

- Fix [#298](https://github.com/Dastari/graphql-orm/issues/298): composite-key and external-read-only Changed roots accept declared subscription operation authorization, enforce its scopes, and export the requirement through catalogue, federation SDL and router descriptors. This addresses the authorization-declaration gap related to #191, not its entire qualification scope.
- Remove unexpanded interpolation comments from generated descriptions, including literal `#struct_name_str` in Changed subscriptions. Existing interpolated operation descriptions remain canonical.
- The 0.90 series adds no schema-generation or migration-planning changes over 0.89.0 for unchanged entity declarations. Existing older-version drift still requires reviewed pending work under a fresh migration version; see MIGRATION.md.

The preceding **0.90.0** preparation introduced breaking filter semantics, included in 0.90.1.

- Fix [#294](https://github.com/Dastari/graphql-orm/issues/294): generated Changed operation descriptors now append the optional configured-case `keys` argument after `filter` and `key`, with GraphQL type `[<scalar key>!]` for a single key or `[<Entity>KeyInput!]` for composite keys. The list itself is nullable. Catalogue, operation and router export fingerprints change; regenerate descriptors and remove adapters that append this argument. Served SDL is unchanged.
- Fix [#295](https://github.com/Dastari/graphql-orm/issues/295): empty `in_list` and empty `or` match no rows across generated SQL backends, bounded writes and in-memory evaluation. The public `FilterExpression::Or` builder follows the same empty/empty-branch rules; runtime schema filter builders already follow these rules. Empty `not_in` remains a no-op. An OR branch with no conditions is true, including when mixed with restrictive branches; empty AND and NOT SQL behavior is unchanged.
- Fix [#296](https://github.com/Dastari/graphql-orm/issues/296): propagated Changes retain distinct provenance paths and reauthorize each path. Relation declaration order no longer drops an authorized alternate path. Cycles are rejected within each path; existing hop and event budgets remain. Subscribers authorized for multiple paths may receive multiple events for the same target/origin pair.

- Review expired documentation governance dates for the release gate and clarify that MSSQL retention remains unsupported even for external-writable entities.
- Add the `graphql-orm-usage` agent skill, which teaches a consuming project how to choose between generated operations, relations, transforms, hooks and custom resolvers, with anti-patterns and a review checklist. Install it with [`skills/install.sh`](skills/README.md#install) at the tag you pin. A standalone crate, `skills/verify`, compiles and runs every Rust block in the skill and pins the library behaviours it states.
- Document the hook order, write-path coverage and deferred-action limits, the `WriteInputTransform` call counts, what `on_delete` does and doesn't observe, when `DELETED` reaches subscribers, the `BatchLoadEntity` requirement, and custom-root name collisions. These describe existing behaviour.
- Correct the macro reference: `schema_policy = "external_read_only"` keeps the generated `Changed` subscription.
- Known gaps recorded while verifying the skill: [#282](https://github.com/Dastari/graphql-orm/issues/282), [#283](https://github.com/Dastari/graphql-orm/issues/283), [#284](https://github.com/Dastari/graphql-orm/issues/284), [#285](https://github.com/Dastari/graphql-orm/issues/285), [#286](https://github.com/Dastari/graphql-orm/issues/286), [#287](https://github.com/Dastari/graphql-orm/issues/287), [#288](https://github.com/Dastari/graphql-orm/issues/288), [#289](https://github.com/Dastari/graphql-orm/issues/289), [#290](https://github.com/Dastari/graphql-orm/issues/290), [#291](https://github.com/Dastari/graphql-orm/issues/291) and [#292](https://github.com/Dastari/graphql-orm/issues/292).

## 0.89.0 (unreleased)

Companion runtime/macros version: **0.89.0**, after 0.88.0 ([#277](https://github.com/Dastari/graphql-orm/pull/277)). AI: **0.120.0**.

- Runtime collections support full-text search ([#273](https://github.com/Dastari/graphql-orm/issues/273)); see [runtime search](docs/reference/graphql-orm/runtime-search.md).
  - **IR.** `RuntimeCollection::search: Option<RuntimeSearch>` declares weighted `String` and JSON-path fields, a PostgreSQL text search configuration, a SQLite tokenizer, `remove_diacritics` and a minimum token length. It is validated with the new `InvalidSearchField` and `InvalidSearchConfiguration` diagnostics. It is structural, but schemas without it keep their exact bytes and fingerprints. `from_static_entities` now maps static search indexes, and still refuses relation-sourced or policy-gated search fields and the fallback strategy.
  - **Migrations.** Runtime migrations create the static search storage. On PostgreSQL, planning refuses an unknown text search configuration before any DDL (`UnknownSearchLanguage`).
  - **Readiness and backfill.** Runtime search storage records `building`/`ready` readiness in `__graphql_orm_search_metadata`. `apply_runtime_migration` backfills existing rows before returning, unless `RuntimeSearchApplyOptions::deferred()`. `backfill_runtime_search` is the resumable, batched host-run backfill: 500 rows per transaction, `FOR UPDATE` on PostgreSQL, returning progress.
  - **Writes.** Every runtime create, update, delete and dependent write maintains the search document in the same transaction.
  - **Runtime GraphQL.** Root lists of searchable collections take `search: RuntimeSearchInput { query, mode, minScore }`, ordered by relevance unless `orderBy` is given. Edges get a nullable `score: Float`; its name and the input's members follow the configured field case. Search is refused with `search_index_not_ready` until the index is ready. Each searchable field is reported to the host as a `"search"` filter operation. Queries are bounded by `RuntimeSearchLimits`: 256 bytes and 32 tokens by default.
  - **Rust API.** `ValidatedRuntimeSchema::runtime_search_request`, `RuntimeReadRequest::with_search` and `RuntimeEdge::score` / `RuntimeAnchoredEdge::score`.
- Breaking for Rust code that builds these with struct literals:
  - `RuntimeCollection` gains `search` and `SearchIndexModel` gains `readiness_tracked`;
  - `RuntimeEdge` and `RuntimeAnchoredEdge` gain `score`;
  - `RuntimeSchemaDiagnosticCode`, `RuntimeMigrationDiagnosticCode` and `RuntimeQueryErrorCode` gain variants.
- `graphql-orm` adds the `unicode-normalization` dependency, which was already in the lockfile.
- Known follow-up: a row-level restore can leave a runtime search index marked `ready` with stale documents, because it does not update documents or reset readiness. Whole-database copies taken from one consistent snapshot are not affected. Until [#280](https://github.com/Dastari/graphql-orm/issues/280) adds a reset, rebuild the index by migrating the collection without search, then with search again; see [backup and restore](docs/reference/graphql-orm/runtime-search.md#backup-and-restore).

## 0.88.0 (unreleased)

Companion runtime/macros version: **0.88.0**. Router: **0.8.1**.

- Generated `<Entity>Changed` subscriptions no longer skip events silently when a subscriber falls behind. Continuity is checked at the event channel, before any `Key`, `Keys`, `Actions`, row-policy or provenance filter. A subscriber that fell further behind than the channel holds receives one error with `extensions.code` `SUBSCRIPTION_CONTINUITY_LOST`, and its stream ends. A closed channel ends the stream with `SUBSCRIPTION_SOURCE_CLOSED`. The error carries no skipped count, key or payload. Through graphql-orm-router, the client receives a `next` frame carrying the error, then `complete`. Clients subscribe again, then refetch. See [delivery and continuity](docs/reference/graphql-orm/macros-and-attributes.md#delivery-and-continuity). ([#225](https://github.com/Dastari/graphql-orm/issues/225))
- Add `graphql_orm::graphql::subscriptions::{SUBSCRIPTION_CONTINUITY_LOST, SUBSCRIPTION_SOURCE_CLOSED}`, the public `DEFAULT_EVENT_CHANNEL_CAPACITY` (256), `Database::event_channel_capacity`, `Database::set_event_channel_capacity` and `DatabaseBuilder::event_channel_capacity`.
- Breaking for Rust callers: generated subscription resolver methods return `Stream<Item = async_graphql::Result<ChangedPayload<_>>>` instead of `Stream<Item = ChangedPayload<_>>`. The GraphQL SDL is unchanged.
- graphql-orm-router 0.8.1 adds a test of the frames a client receives when a subgraph subscription ends with an error item; router behaviour is unchanged.
- Mid-stream authority revocation is still skipped silently; [#276](https://github.com/Dastari/graphql-orm/issues/276) tracks ending the stream instead.

## 0.87.0 (unreleased)

Companion runtime/macros version: **0.87.0**. Operation catalogue: **0.7.0**.

- Entities with a managed record version (`#[graphql_orm(version = "managed")]`) also serve it as a read-only `RecordVersion: Int!` field. The name is fixed: it does not follow the `field-case-*` features, so a client recognises each type's record version from the SDL or introspection alone. It resolves the declared field's value and applies that field's read policy and subscription availability. Generated `Changed` payloads may select it. CAS-only `version` fields get no `RecordVersion`. See [managed versions](docs/reference/graphql-orm/macros-and-attributes.md#managed-versions). ([#264](https://github.com/Dastari/graphql-orm/issues/264))
- Add `graphql_orm::graphql::orm::RECORD_VERSION_FIELD` and `Entity::managed_version_field()`, which returns the declared field's GraphQL name for a managed version, and none otherwise.
- A field whose GraphQL name is `RecordVersion` on an entity with a managed version is a compile error.
- graphql-orm-operation-catalog 0.7.0 adds `GraphqlEntitySemanticMetadata::record_version: Option<GraphqlRecordVersionMetadata>`. It holds the declared field's wire name and a `GraphqlRecordVersionMode` of `managed` or `cas`, and is validated to name a scalar field of the entity. It is omitted from the wire and fingerprint when absent, so semantic wire 3 is unchanged. Struct literals must add the member. graphql-orm-ai-tool-profiles 0.20.0 and graphql-orm-ai 0.119.0 adopt catalogue 0.7.0.
- Entities without a managed version keep unchanged SDL. Catalogues without versions keep unchanged payloads and fingerprints.
- Fix: an entity with a managed version on a read-only generated surface, for example SQL Server `schema_policy = "external_read_only"`, no longer fails to compile with a missing `__gom_runtime_error`. This was broken since 0.84.0.

## 0.86.0 (unreleased)

Companion runtime/macros version: **0.86.0**. Router: **0.8.0**.

- Add the opt-in global record identity `#[graphql_entity(node_id)]`. It generates a read-only `NodeId: ID!` field: base64url, without padding, of the compact JSON array `[typename, ...primaryKeyParts]`, with non-ASCII left unescaped, matching graphql-orm-client. It also generates a resolvable `@key(fields: "NodeId")` entity resolver. A malformed, non-canonical or mismatched NodeId resolves to null before any read. A decoded key follows the primary-key resolver's authorization, batching and null-on-deny chain exactly. Subgraph SDL declares no `interface Node`. See [global NodeId](docs/reference/graphql-orm/federation.md#global-nodeid). ([#260](https://github.com/Dastari/graphql-orm/issues/260))
- Add `graphql_orm::graphql::federation::{NODE_ID_FIELD, encode_node_id, decode_node_id}`.
- `node_id` is a compile error on repository-only, schema-only and `#[graphql(complex)]` entities, and on float, nullable or read-policy primary-key fields.
- graphql-orm-router 0.8.0 adds the router-hosted `node(NodeId)` / `nodes(NodeIds)` lookup that composes and serves `interface Node` in-process; see the [router changelog](crates/graphql-orm-router/CHANGELOG.md).
- `base64` becomes an ordinary dependency of `graphql-orm`. It was already in the lockfile, behind `runtime-graphql`.
- Additive. Entities without `node_id` produce unchanged SDL and behaviour.

## 0.85.0 (unreleased)

Companion runtime/macros version: **0.85.0**.

- Generated `<Entity>Changed` subscriptions accept an optional `Keys` list, with the same element type as `Key`: the key scalar, or `<Entity>KeyInput` for composite keys. An event is delivered when its complete key is in the set, and a `PROPAGATED` parent event matches the parent's key. Duplicates are ignored and an empty list matches nothing. Every delivered event is authorized at delivery time, exactly as for `Key`. See [generated `Changed` subscriptions](docs/reference/graphql-orm/macros-and-attributes.md#generated-changed-subscriptions). ([#261](https://github.com/Dastari/graphql-orm/issues/261))
- `Key` and `Keys` together, or a `Keys` list longer than `Database::subscription_key_limit()`, fail with `INVALID_INPUT` at admission. The limit defaults to the new `DEFAULT_SUBSCRIPTION_KEY_LIMIT` (500). Set it with `Database::set_subscription_key_limit`, `Database::with_subscription_key_limit` or `DatabaseBuilder::subscription_key_limit`.
- Additive. Subscriptions that do not pass `Keys` behave as before; their SDL gains only the optional argument.

## 0.84.0 (unreleased)

Companion runtime/macros version: **0.84.0**.

- Add managed record versions: `#[graphql_orm(version = "managed")]` on an `i64` field. Every generated write advances it by one in the same statement:
  - generated GraphQL `Update`;
  - repository and transaction `update_by_id` and `update_where`;
  - `update_by_key`, `update_if` and `update_where_bounded`;
  - the update branch of `upsert`;
  - `compare_and_swap`.

  Inserts write the declared default. Rollback leaves the value unchanged. Advancing past `i64::MAX` is refused with an error on every path and backend, including the update branch of an upsert. Concurrent writers lose no increments; on PostgreSQL and SQL Server a racing writer may instead get a retryable `SERVICE_UNAVAILABLE` that writes nothing. `Changed` events carry the committed version. The value stays out of inputs. See [managed versions](docs/reference/graphql-orm/macros-and-attributes.md#managed-versions). ([#259](https://github.com/Dastari/graphql-orm/issues/259))
- Generated GraphQL `Update` and `Delete` for a managed entity accept an optional `ExpectedVersion`. A mismatch fails with `CONFLICT` and writes nothing.
- Add `render_upsert_sql_with_managed_version` and `build_upsert_sql_with_managed_version`. The existing functions are unchanged.
- Document that a bare `#[graphql_orm(version)]` is compare-and-swap only: ordinary writes do not advance it. That is the existing behaviour, and `version = "cas"` names it explicitly. Entities without `version = "managed"` produce unchanged SDL and SQL.

## 0.82.0 (unreleased)

Companion runtime/macros version: **0.82.0**.

- Include the shareable aggregate-result fix from the 0.81.2 source checkpoint:
  generated federation SDL marks `GraphqlAggregateResultRow` and
  `GraphqlAggregateResultValue` `@shareable`, as `PageInfo` already is. This lets
  two aggregate-enabled subgraphs compose without duplicate wrapper-field
  ownership errors. The SDL regression and two-subgraph aggregate fixture are
  included; no stored-data migration or aggregate-policy change is required.

- Select published Auth `v0.24.0` instead of its full revision selector. The tag
  peels to the same `1f4b4bfb6d052d04e6c80b80cb7aa3d0c2ebaacd` source; align
  direct host selectors to retain one Auth type universe. See the
  [selection migration](MIGRATION.md#published-agql-auth-0240-selection).

- ORM/macros 0.82.0 add opt-in exact static signed64 filters (`int64`,
  `Int64Filter`, `Signed64`) with canonical metadata, bounded lists and exact SQL
  bindings/matching with SQL three-valued NULL composition and predecode list
  admission. Existing 32-bit filters remain unchanged.
- Generated GraphQL list (including composite keys), keyset and authorized-scan
  reads preflight the read policies of all static predicate fields before SQL,
  including recursive predicates whose protected fields are absent from the
  response selection. Hosts must authorize predicate intent without a row.

- ORM/macros 0.82.0 allow generated upward Changed propagation between public
  entities in different Rust crates, retaining trusted provenance and subscriber
  authorization. Enforce each source/hop's own Changed auth mode, scopes and
  explicit assurance coordinate at current/final release. Repository-only hooks
  remain private.

- Add `RuntimeReadAuthority::release_mutation_result` and `RuntimeMutationRelease` (`Release`, `Withhold`), so a host can withhold a committed mutation's record that the caller may not view: the payload is `{"record": null}` with no error, and no record value, projected host object or relation is serialized. The default releases exactly when `authorize_mutation_result` succeeds, so existing hosts are unchanged. See [post-commit record release](docs/reference/graphql-orm/runtime-graphql.md). Additive; no migration.

## 0.81.0 (unreleased)

Companion runtime/macros version: **0.81.0**.

- Add host-supplied actor stamps for runtime fields, the analogue of Spring Data's `@CreatedBy`/`AuditorAware`: `RuntimeField::auto_actor: Option<RuntimeAutoActor>` with `OnCreate`. Valid only on nullable, non-primary-key `Uuid` or `String` fields that are not generated, have no default and do not declare `auto_timestamp` (`InvalidAutoActor` with a value-free `RuntimeAutoActorProblem` detail). See [actor stamps](docs/reference/graphql-orm/runtime-schema-ir.md#actor-stamps).
  - `RuntimeWriteGrant::with_actor(RuntimeValue)`: the value every runtime create writes into each actor stamp the request does not supply (SQL NULL without it). It is checked against each stamp's value kind before any SQL write (`invalid_input` on mismatch). Updates never write the actor.
  - `RuntimeWriteGrant::with_actor_assignment()`: permits the request itself to supply actor stamps on create and update. Without it, a supplied value (explicit `Null` included) fails with `field_denied` after `authorize_intent` and before any SQL write. Request construction accepts the field so the decision can use the grant. Both survive the GraphQL restriction of a grant.
  - Dependent `SetNull` updates and link mutations never change an actor stamp, except that the referential action clears a stamp that is itself the relation key being nulled.
  - Staged and preimage images always include actor stamps, so result checks and `before_commit` hooks see them; client output stays the return projection.
  - Runtime GraphQL omits actor stamps from `Create<T>Input` and `Update<T>Input`. The new opt-in `RuntimeGraphqlMutationOptions::actor_inputs` (default `false`) adds them as optional inputs of both, for hosts that assign actors explicitly.
  - Migrations: the column is plain nullable with no default; adding it leaves existing rows NULL, and switching the flag plans no DDL. Backup/restore keeps stored values.
  - `RuntimeField::is_client_writable` is `false` for actor stamps; `RuntimeFieldHandle::auto_actor` exposes the flag.
  - Identity: the member is omitted when absent and joins the field's ID-bearing and structural canonical lines (`|auto_actor=on_create`) only when present. Schemas without it are byte-identical (pinned by `tests/runtime_identity_goldens.rs`).
- **Breaking (Rust):** `RuntimeField` struct literals must add `auto_actor: None`.
- **Breaking (Rust):** `RuntimeSchemaDiagnosticCode` gains `InvalidAutoActor`; exhaustive matches need an arm. (`RuntimeSchemaDiagnosticDetail` and `RuntimeGraphqlMutationOptions` are `#[non_exhaustive]`; the new `AutoActor` variant and `actor_inputs` field are additive.)
- **Breaking (behaviour, opt-in schemas only):** a runtime create or update that supplies an actor-stamped field is refused with `field_denied` unless the write grant permits assignment, and `RuntimeField::is_client_writable` is `false` for such fields. Schemas that declare no actor stamp are unaffected.
- **Breaking (serialized IR):** readers older than 0.81.0 reject a schema document that declares `auto_actor` (`deny_unknown_fields`). Documents without it are unchanged and the format version stays 1.

## 0.80.0 (unreleased)

- Add producer-owned typed filter contracts in operation catalog 0.6.0 (wire 3),
  ORM/macros 0.80.0, profiles 0.19.0 and AI 0.118.0. Canonical operator meanings,
  exact operand types and closed auxiliary objects must match finished SDL;
  arbitrary record inputs, extra operators, wrong object operands and stale
  metadata fail readiness. Include `NotIn`, `Similar` and distinct spatial
  operators without deriving authority from SDL or helper names.
- Preserve public scalar filters beside private relation prefixes, transitive
  custom-input privacy, keyset primary keys and runtime order field restrictions.
  Backup 0.12.0 adopts the optional ORM public type identity only. No data or
  schema migration; regenerate semantic payloads and capability bindings. The
  spatial/JSON planning codec gate remains outstanding; structural parity alone
  is not geometry planning qualification.

Companion runtime/macros version: **0.80.0**.

- **Breaking (Cargo defaults and public reexport):** async-graphql defaults are
  disabled. ORM retains `dataloader`, `uuid` and `decimal`; `runtime-graphql`
  still explicitly enables `dynamic-schema`. Incidental email validator,
  GraphiQL/playground and tempfile APIs are no longer supplied by
  `graphql_orm::async_graphql`. `UploadValue.content` is byte-backed
  (`bytes::Bytes`) without `tempfile`, rather than `std::fs::File`. Hosts using
  those APIs must explicitly enable the matching async-graphql features; see
  [migration examples](MIGRATION.md#0780-to-0790-explicit-graphql-features).
- AI 0.117.0 keeps UUID and base GraphQL APIs; profiles 0.18.0 keeps base
  parser/errors without adding UUID. Macros remain aligned at 0.79.0; the
  explicit feature change itself does not alter generated operation behavior.
  Static/runtime SDL, scalar behavior,
  query/relation batching, federation, authorization and persistence formats
  are unchanged; typed semantic metadata changes are described above.
- **Breaking (Auth source identity):** ORM, AI and optional router 0.7.0 Auth
  integration now use reviewed merged Auth 0.24.0 full revision
  `1f4b4bfb6d052d04e6c80b80cb7aa3d0c2ebaacd`, not a published release tag.
  Hosts must use the identical selector to pass Auth types across public APIs.
- Add independent locked minimal/additive host feature and public-constructor
  probes. No data migration is required; no legacy umbrella is enabled.

## 0.78.0 (unreleased)

Companion runtime/macros version: **0.78.0**.

- Runtime GraphQL mutation payloads may select the record's to-many and many-to-many (join) relations as `…Connection` pages with `first`/`last`, `after`/`before`, `where`, `orderBy`, `pageInfo` and `totalCount`, as queries do. Before, such a selection failed with `unsupported_capability`; to-one relations were already supported. Each page is planned, bounded and authorized like a query relation layer before the write (`RuntimeReadAuthority::authorize` with the relation), and read after commit through the batched relation executor, so a create or update with attached link changes, or a link mutation, shows the committed links. Rows hidden by the layer's grant are omitted from the page and its count; a layer denied outright refuses the mutation before the write; a failure while reading the page after commit is the existing committed, non-retryable `mutation_payload_failed`. The payload's record layer also loads the relation's source key fields (the join's source fields), which the host's read and output checks see.
- SDL is unchanged: the payload `record` type already declared these connection fields. Clients that read links back with a second request can select them in the mutation instead.

## 0.77.0 (unreleased)

Companion runtime/macros version: **0.77.0**.

- Back up runtime `DateTime` columns: `BackupValueKind::DateTime` (schema string `date_time`). Values are `BackupValue::String` holding canonical `RuntimeDateTime` text (UTC, six fractional digits) on both backends. SQLite text is canonicalized through the record engine's parse; PostgreSQL `TIMESTAMPTZ` is read typed, independent of the session `TimeZone`, and bound through `CAST(? AS TIMESTAMPTZ)`. Export refuses values without a portable form (unreadable SQLite text, PostgreSQL `±infinity`, UTC years outside 0001–9999, including after offset normalization or rounding, and leap-second text). Restore refuses non-canonical or out-of-range text before any write, including in `DryRun`, and writes stored values; no default, clock or automatic timestamp applies. Opt-in: only descriptors that declare the kind use it. Static derived date fields keep `BackupValueKind::String`, so their descriptors and schema hashes are unchanged. See [backup runtime](docs/reference/graphql-orm/backup-runtime.md#logical-rows).
- Fix: restore of a single-key table whose rows form a self-reference cycle (A → B → A) no longer fails with "contain a self-reference cycle". Rows parent-first order cannot place are inserted last with placeholder self-reference values (`NULL` when nullable, the row's own key otherwise) and then updated to their stored values in the table's import transaction. A `UNIQUE` nullable self-reference (one-to-one) cycle restores without a transient collision. Acyclic restores are unchanged.
- Add `BackupSelfReferenceCycle` (`from_error`): the named refusal when a staged cycle cannot take its stored values, for a non-null `UNIQUE` self-reference cycle or an append-only table's update trigger. The table's import transaction rolls back. Constraint violations and explicit trigger refusals retain the original SQLx/database cause in the standard error source chain; operational errors such as PostgreSQL cancellation/deadlock propagate unchanged.
- Restore now preflights every table's row shapes and value kinds before importing any table. Late invalid DateTime, type or shape input leaves all target tables untouched; runtime database failures retain per-table transaction boundaries.
- Add `Database::database_identity()` returning `DatabaseIdentity` (PostgreSQL `current_database()` and its OID; SQLite main file path), for hosts to check a restore target before writing to it.
- **Breaking (Rust):** `BackupValueKind` gains `DateTime` and is now `#[non_exhaustive]`. Matches outside the crate need a wildcard arm; later kinds will not break them again.
- **Breaking (GraphQL):** any schema that exposes `BackupValueKind` gains the `DATE_TIME` enum value.

## 0.76.0 (unreleased)

Companion runtime/macros version: **0.76.0**.

- Fix: generated SQL delimits identifiers that need it, so a table, column, index or constraint named with an SQL reserved word (`order`, `join`, `select`, `user`, `group`, …) or containing other characters works on SQLite, PostgreSQL and MSSQL. This covers create-table column definitions, primary-key, unique and foreign-key lists and referenced tables, add/drop/alter column, SQLite rebuild copies, append-only triggers, constraint comments, RLS statements, SQLite introspection `PRAGMA`s, the repository and loader select/count/delete DML, primary-key ordering, and the derive-generated insert, update, delete, filter, sort, check-constraint and relation SQL. Before, SQLite refused `CREATE TABLE … (join TEXT …)` with `near "join": syntax error`.
  - A plain identifier that is not reserved on the backend is still written bare and byte-identical, so statement text, plan hashes, migration history, schema hashes, the 0.50.2 compatibility goldens and PostgreSQL case folding are unchanged for every name that worked before. Reserved words are the ones each engine rejects as bare names, measured against SQLite 3.46 and PostgreSQL 17 (`pg_get_keywords()` categories `R` and `T`), plus the documented T-SQL reserved keywords. Introspection already read delimited names, so identities do not change.
  - New `DatabaseBackend::render_identifier` and `render_identifier_path` expose the rule for hosts that compose SQL fragments.
  - Mutation preimage predicates and pool-based insert rereads render physical primary-key names too; auto-generated key inserts use the declared physical column instead of assuming `id`. Qualified names split only at unquoted dots, preserving quoted segments and escaped closing delimiters in both runtime and derive SQL.
- **Breaking (plan text):** seven SQLite keywords were accepted bare in some positions and are now always delimited: `cast`, `constraint`, `current_date`, `current_time`, `current_timestamp`, `if` and `raise`. A new plan that names such a table or column has different statement text and plan hash than 0.75.0 would render; the objects' identities are unchanged. MSSQL derive DML now brackets the table name, as `TABLE_NAME` already did. No other previously accepted name renders differently.

## 0.75.0 (unreleased)

Companion runtime/macros version: **0.75.0**.

- Add server-managed timestamps for runtime fields: `RuntimeField::auto_timestamp: Option<RuntimeAutoTimestamp>` with `OnCreate` and `OnCreateAndUpdate`, like Rails/Django `created_at`/`updated_at`. Valid only on non-key `DateTime` fields that are not generated and have no literal default (`InvalidAutoTimestamp` with a value-free `RuntimeAutoTimestampProblem` detail). See [automatic timestamps](docs/reference/graphql-orm/runtime-schema-ir.md#automatic-timestamps).
  - Every runtime insert sets the field from the database clock, with the same expression and encoding as `CurrentTimestamp`. Every runtime update of the row refreshes `OnCreateAndUpdate` fields in the same `UPDATE`, also when no stored value changes and for the dependent `SetNull` updates of an opted-in delete. A standalone many-to-many link mutation writes no row field and leaves them unchanged; link changes attached to a create or update ride on that row write.
  - Create and update requests refuse a supplied value, explicit `Null` included, with `field_denied`. Runtime GraphQL omits the field from `Create<T>Input` and `Update<T>Input`. A required auto field is never `missing_required`.
  - The column's physical default is `CurrentTimestamp` (`RuntimeField::effective_default`), so adding the column to a table with rows backfills them and the replan is a no-op. Switching the flag is catalog-only when the column already has a `CurrentTimestamp` default; otherwise it plans the default change.
  - `RuntimeField::is_client_writable` and `RuntimeFieldHandle::auto_timestamp` expose the rule to hosts.
  - Identity: the member is omitted when absent and joins the field's ID-bearing and structural canonical lines only when present. Serialized documents, canonical bytes, fingerprints, physical targets, plans and SDL of schemas without it are byte-identical (pinned by `tests/runtime_identity_goldens.rs`, captured at 0.74.0, and the 0.50.2 compatibility goldens).
- Runtime GraphQL builds collections without client-writable fields: `create<Object>` takes no `input` when nothing is writable, and `update<Object>`/`Update<Object>Input` are omitted when no non-key field is writable. Previously such a module failed with `unsupported_capability`.
- Staged mutation images always include the collection's automatic timestamps, independent of the host's write-grant projection, so `before_commit` hooks see the stamped values.
- Fix: owned SQLite plans apply an `AddColumn` with a non-constant default (such as a `CurrentTimestamp` datetime column) as a controlled table rebuild, because SQLite refuses `ALTER TABLE … ADD COLUMN` with such a default. These plans failed at apply before; the rebuild fills existing rows from the default. Constant defaults still use `ADD COLUMN`, so other plans and their hashes are unchanged. The single-step `render_migration_step` emits a `-- sqlite requires table rebuild` marker for such a step, as for column drops.
- **Breaking (Rust):** `RuntimeField` struct literals must add `auto_timestamp: None`.
- **Breaking (Rust):** `RuntimeSchemaDiagnosticCode` gains `InvalidAutoTimestamp`; exhaustive matches need an arm. (`RuntimeSchemaDiagnosticDetail` is `#[non_exhaustive]`; its new `AutoTimestamp` variant is additive.)
- **Breaking (serialized IR):** readers older than 0.75.0 reject a schema document that declares `auto_timestamp` (`deny_unknown_fields`). Documents without it are unchanged and the format version stays 1.

## 0.74.0 (unreleased)

Companion runtime/macros version: **0.74.0**.

- **Breaking (generated names):** derived ordinary, unique, spatial and
  search-vector index names longer than 63 bytes are now shortened
  deterministically: a 46-byte prefix, `_`, and the 16-digit FNV-1a hash of the
  complete name, the scheme the runtime already uses for generated index,
  foreign-key and check-constraint names. PostgreSQL silently truncated those
  names, so every later strict or owned plan proposed a create plus a drop and
  `runtime_mutation_environment` reported `BaselineMismatch`. Names of 63 bytes or
  fewer are byte-identical.
- **Breaking (PostgreSQL plans):** a PostgreSQL ordinary or owned plan that would
  create an explicitly declared index, search index, foreign-key or check
  constraint whose name exceeds 63 bytes is refused with a diagnostic naming the
  identifier (`InvalidPhysicalContract` with the name as `subject` for owned
  plans) instead of being truncated. SQLite and MSSQL plans are unchanged.

## 0.73.0 (unreleased)

Companion runtime/macros version: **0.73.0**.

- Create and update requests can carry many-to-many link changes, applied to the written row in the same transaction: `RuntimeMutationRequest::with_link_change(relation, change, max_link_keys)`, one change per join relation of the request's own collection.
  - After the row write and its result check, each change runs as a complete link mutation of the written row, exactly like `runtime_link_request`: its own `authorize_intent` and locked, authorized preimage (a source hidden from it is `not_found`), `link_target_visibility` scoping, uniform `denied` for hidden or missing targets, `authorize_links`, and `authorize_result`.
  - `RuntimeWriteIntent::link_parent()`, `RuntimeLinkCheck::link_parent()` and `RuntimeLinkVisibilityCheck::link_parent()` return `Create` or `Update` for those link mutations, and `None` for a standalone link mutation.
  - Any refusal rolls back the row write too. A create's links bind to its generated key. `RuntimeMutationEffect::links` lists every change, and `attached_link_changes` lists the requested changes.
  - Runtime GraphQL counts each attached change against the statement budget as one more write.
- Runtime GraphQL `create<Object>`/`update<Object>` take an opt-in `links: <Object>LinksInput` argument (`RuntimeGraphqlMutationOptions::links_in_writes`, default off). It has one `<Object><Relation>LinkChangeInput` per many-to-many relation, with `set`, or `add`/`remove`, keyed like the link roots. It requires updates to be enabled. The semantic catalogue lists the argument when it exists.
- **Breaking (Rust):** `RuntimeWriteIntent::links()` returns an iterator over every requested link change of the write instead of an `Option`: the one change of a link mutation, or the changes attached to a create or update. The outer intent of a create or update therefore shows its attached changes.
- **Behaviour:** the effect of a create or update can now carry `links()`. Journal hooks that read links only from field-less `Update` and `Delete` effects miss them. The GraphQL option defaults to off, so SDL and catalogues are unchanged unless enabled. See [MIGRATION](MIGRATION.md#0720-to-0730-atomic-link-changes-in-writes).

## 0.71.0 (unreleased)

Companion runtime/macros version: **0.71.0**.

- Back up and restore composite-key tables. `EntityBackupDescriptor` gains `primary_key_columns`, the complete key in key order when it has more than one column (empty otherwise). Export orders by the whole key. A row's identity (`BackupRow::primary_key`) is the canonical JSON array of every key part, for example `["1","2"]`: JSON parts are canonical, and a null part is refused at export. Restore verification and full-snapshot row maps use that identity. Self-reference ordering still applies to single-column keys only. Static entities with a composite key now describe their whole key; before, only the first column was described and their rows collided.
- Add `ValidatedRuntimeSchema::join_table_backup_descriptors`. It returns one descriptor per ORM-owned many-to-many link table. Exact inverses share one, described from the side with the lowest (collection ID, relation ID), so collection order never changes it. Each has the composite (source, target) key, `Cascade` dependencies on both endpoint tables, and an export/restore order after both endpoints' orders as the host supplies them. `RuntimeLinkBackupError` reports an unordered or unknown endpoint, an unknown key field, or an unsupported `DateTime` key.
- `restore_backup_rows` refuses an invalid backup before writing anything, in both `EmptyDatabase` and `DryRun` modes:
  - every verbatim dependency value must name a row of the backup's own target table, otherwise it fails with a typed `BackupOrphanReference` (`BackupOrphanReference::from_error`);
  - in `EmptyDatabase` mode every table must be empty.
  Before, earlier tables were committed when a later table failed.
- The change journal refuses to record a mutation event for a composite-key entity, because its entity-id row identity cannot name the row. The ORM generates no write path for composite-key entities, so this applies only to events a host emits through `run_mutation_hook`. `compact_chain` refuses incremental changes to a composite-key table. Back such tables up with full snapshots.
- **Breaking (Rust):** struct literals of `EntityBackupDescriptor` must add `primary_key_columns: Vec::new()` (or the key).
- **Breaking (restore of older backups):** a schema that contains a static composite-key entity has a new `stable_schema_hash` (composite descriptors add `|key:…`). Restore requires an exact schema hash, so a backup of such a schema taken before 0.71 cannot be restored with 0.71. To migrate, restore it with 0.70, upgrade, and take a new backup. Schemas whose keys are all single-column keep their hashes, manifests and row identities unchanged.
- **Breaking (behaviour):** with the change journal enabled, a host-emitted mutation event for a composite-key entity fails its transaction instead of recording an unusable journal row. A backup with an orphan reference, or a restore into a non-empty table, is now refused before any table is written instead of partway through.

## 0.70.1 (unreleased)

Companion runtime/macros version: **0.70.1**.

- Select agql-auth by its published release tag `v0.23.0` instead of the reviewed merged revision `158c79e27117adc5a95d01d58c39c39fd44e6a1a`. The tag resolves to that same commit, so no source, behaviour or version changes. Consumers that selected the revision must switch to the tag as well: Cargo treats the two selectors as different sources. See [MIGRATION](MIGRATION.md#agql-auth-0230-selection).
- Fix: replacing a foreign key with the same columns and target (for example changing its delete policy) now drops the old constraint before adding the new one. The generated constraint name does not include the policy, and on PostgreSQL the add could run first and fail because the name was still in use. That happened when the new policy sorted before the old one: `Cascade`/`SetNull` to `Restrict`, or `SetNull` to `Cascade`. Every drop of a table now precedes every add. SQLite rebuilds are unaffected.

## 0.70.0 (unreleased)

Companion runtime/macros version: **0.70.0**.

- Refuse hidden and missing link targets uniformly. A link mutation now requires every requested target (`set`, `add`, `remove`) to be an existing row admitted by `RuntimeWriteAuthority::link_target_visibility`. PostgreSQL locks it `FOR KEY SHARE`. Any other target fails with `denied` before `authorize_links`, hooks or any write. A host that allows such a target can no longer leak a hidden link's existence through `RuntimeLinkEffect::added`, or a row's existence through `constraint_violation` versus success.
- **Breaking (behaviour):** a link mutation naming a missing target returns `denied` instead of `constraint_violation`, even when the host returns no visibility predicate. Removing a missing or hidden target is refused instead of being a no-op. Adding or removing an already-linked hidden target is refused instead of leaving the link unchanged.
- Add SQLite and PostgreSQL link tests for UUID and text primary keys (exact matching of existing links, duplicate refusal, inverse side and deletes).

## 0.69.0 (unreleased)

Companion runtime/macros version: **0.69.0**.

- Select agql-auth 0.23.0 at reviewed merged revision `158c79e27117adc5a95d01d58c39c39fd44e6a1a` (OAuth 2.0 userinfo providers, `form_post` and signed client credentials). No ORM, AI or router source change; `auth-agql` users see the 0.23.0 Auth types. See [MIGRATION](MIGRATION.md#agql-auth-0230-selection).
- Add many-to-many runtime relations. `RuntimeRelation::join: Option<RuntimeJoinTable>` names an ORM-owned link table mapping the source and target primary keys; it is validated (`InvalidJoinTable`), structural in both fingerprints, and planned as one table with a pair primary key, two cascading foreign keys and a target index. Inverse relations may share it. Join relations read as batched, filtered, paged and counted to-many layers through `EXISTS` on the link table.
- Add link writes. `runtime_link_request` takes `RuntimeLinkChange::{Replace, Delta}` and is scoped to targets the new `RuntimeWriteAuthority::link_target_visibility` admits: hidden links are never read, counted, replaced or removed. The new `RuntimeWriteAuthority::authorize_links` receives every requested target as well as the exact delta, so outcomes do not depend on hidden link state. Both hooks deny by default and run before any write. Writes are batched (`DELETE … IN`, multi-row `INSERT … ON CONFLICT DO NOTHING`), so a concurrent duplicate from the inverse side is idempotent or retryable. Deletes remove all of a row's links explicitly after the same authorization, with their own bound (`with_max_delete_links`, default 10 000). `RuntimeMutationEffect::links()` exposes link effects to hooks. Runtime GraphQL adds `update<Object><Relation>Links` roots with `RuntimeGraphqlMutationOptions::{max_link_keys, max_delete_links}`. SQLite and PostgreSQL behave alike.
- **Breaking (Rust):** `RuntimeRelation` gains the public field `join` (struct literals add `join: None`), and `RuntimeSchemaDiagnosticCode` gains `InvalidJoinTable`.

## 0.68.0 (unreleased)

Companion runtime/macros version: **0.68.0**; operation catalog **0.5.0**; AI tool profiles **0.17.0**; AI **0.110.0**.

- Add `RuntimeCollection::label_fields`: ordered, validated label (display) fields. They are part of the ID-bearing fingerprint when declared (rendered as a JSON array of stable IDs), but not of the structural fingerprint, the physical schema or migration plans. Ineligible members fail with the new diagnostic `InvalidLabelField`, and unknown fields with `UnknownFieldReference`. Runtime GraphQL publishes them in the semantic catalogue, but omits a label that has a projector-served member.
- Operation catalog 0.5.0: `GraphqlEntitySemanticMetadata` gains `label_fields: Vec<String>`. Each entry must name a selectable, exportable, non-secret, non-relationship scalar or enum field; at most `GRAPHQL_SEMANTIC_MAXIMUM_LABEL_FIELDS` (4). The member is omitted from the wire and fingerprint when empty.
- **Breaking (Rust):** struct literals of `RuntimeCollection` and `GraphqlEntitySemanticMetadata` must add the field. Generated derives and every workspace construction are updated.
- **Breaking (wire, older readers):** `RuntimeSchema` documents and semantic catalogues keep format versions 1 and 2, following the convention for optional members. Bytes and fingerprints are unchanged only while every label is empty. A document or catalogue that declares a label is rejected with an unknown-field error by readers older than this release, because both types are `deny_unknown_fields`. That includes routers or AI hosts that parse a subgraph's catalogue. Upgrade readers before writers declare labels.
- AI tool profiles 0.17.0 and AI 0.110.0: no behaviour change. They take and re-export operation-catalog types in their public API, so the catalog's breaking 0.5.0 makes them breaking minors.

## 0.67.0 (unreleased)

Companion runtime/macros version: **0.67.0**.

- Add runtime relation predicates. `runtime_relation_filter` and `RuntimeFilterBuilder::relation` take `RuntimeRelationQuantifier` (`is`/`isNot` for to-one, `some`/`none`/`every` for to-many) and lower to a correlated `EXISTS` on SQLite and PostgreSQL. The target layer's grant is always applied inside the subquery, so hidden related rows never match. `RuntimeGraphqlOptions::with_relation_filters()` (opt-in) exposes them in every `…WhereInput` through generated `…ToOneFilter`/`…ToManyFilter` inputs. Each target is authorized with `RuntimeReadPurpose::RelationFilter`; a denied target is non-matching, not an error. Offline record matching rejects relation predicates.
- Runtime GraphQL mutation payloads may select to-one relations, which are authorized like query layers and read after commit through the relation batch executor. Adds `ValidatedRuntimeSchema::runtime_relation_parents`.
- Relation filters can be admitted per relation (`with_relation_filters_for`) and have a dedicated budget (`with_relation_filter_limits`; defaults 16 per operation, depth 3). Each predicate's correlation key fields appear in the source layer's `filter_operations` as `relation`, so field-level host rules apply to them. Payload relation reads share request budgets.
- **Breaking (Rust):** `RuntimeReadCheck` is now `#[non_exhaustive]` and gains `relation_filters` and `purpose`, so it can no longer be constructed outside the crate. `RuntimeRelationQuantifier` and `RuntimeReadPurpose` are `#[non_exhaustive]`.

## 0.66.0 (unreleased)

Companion runtime/macros version: **0.66.0**.

- Runtime deletes can opt in to incoming `Cascade` and `SetNull` foreign keys with `RuntimeMutationRequest::with_dependent_changes(RuntimeDependentLimits)` (validated, opaque limits, at most 65 536 rows and depth 64) (GraphQL: `RuntimeGraphqlMutationOptions::dependent_changes`). The ORM locks every dependent row, authorizes each change through `RuntimeWriteAuthority` with `RuntimeWriteIntent::cause()` before any write, performs the SetNull updates and the deletes itself in dependency order, and lists them in `RuntimeMutationEffect::dependents()` for hooks. Restrict references to a deleted row, denied dependents, data cycles, non-runtime modifying sources and exceeded row/depth limits refuse the whole delete before any write. SQLite and PostgreSQL behave alike; a concurrently inserted referencing row is blocked or fails the delete as `retryable_transaction`. Without the opt-in such deletes still refuse with `unsupported_cascade`. `RuntimeMutationEffect` also gains `collection()` and `cause()`.

## 0.65.0 (unreleased)

Companion runtime/macros version: **0.65.0**.

- Add `#[graphql_policy_object]` for the `SimpleObject` plus generated-relations
  path. Selected scalar getters now call current ORM entity, row and field
  policies, including `read_policy`, while preserving existing host guards.
- Preserve async-graphql guard expressions when parsing entity naming metadata.

## 0.64.1 (unreleased)

- SQLite classifies a statement refused by a foreign-key `RESTRICT` action as `CONSTRAINT_VIOLATION` (runtime mutation code `constraint_violation`), matching PostgreSQL. SQLite raises `RESTRICT` through its internal trigger program with extended code `SQLITE_CONSTRAINT_TRIGGER` (1811), which sqlx reports as an unclassified error, so such deletes previously surfaced as `INTERNAL_ERROR` / `database_failed`. The rule requires the SQLite driver error, extended code 1811 and SQLite's exact `FOREIGN KEY constraint failed` message; every other trigger failure keeps its previous classification. PostgreSQL, MSSQL and `NO ACTION` (787) are unchanged.

## 0.64.0 (unreleased)

Companion runtime/macros version: **0.64.0**.

- Revalidate the complete current release contract after successful selected
  snapshot serialization; source, target, key or field revocation suppresses
  the prepared event without a frame or private diagnostics.

- Prepare selected Changed snapshots before stream release. Discard late scalar
  refusals and private policy diagnostics without emitting even an error frame,
  and preserve clean delivery of subsequent authorized events.

- Seed cycle detection with the same complete typed target identity and origin
  encoding used by propagated events, including composite and append-only sources.

- Generate real composite-key and read-only projection subscriptions with typed
  complete-key filters and current row/field/provenance authorization.
- Emit complete typed composite direct witnesses only after committed generated
  repository mutations; preserve trusted rollback and propagation fences.
- Expose generated subscriptions under read-only schema policy while retaining
  empty mutation roots and rejecting arbitrary extra subscriptions.
- Read-only and composite `GraphQLOperations` entities require both serde derives
  for generated Changed observer events, matching writable scalar-key entities.
- Keep private repository sources closed to GraphQL notification release pending
  a separate explicit projection-authorization contract.

## 0.63.0 (unreleased)

Companion runtime/macros version: **0.63.0**.

- Seed propagated-event cycle detection with the complete typed target identity and origin so cycles cannot re-emit the direct source.
- Resolve generated upward propagation using all declared relation from/to components, independent of either entity's primary key.
- Revalidate source/target tuple witnesses and current relation-field authority before subscriber delivery; reject fanout that exceeds the remaining budget.

## 0.62.0 (unreleased)

Companion runtime/macros version: **0.62.0**.

- Add typed runtime `orderBy` to generated keyset queries and repository forward/backward reads. Whitelist persisted scalar order fields, append the complete primary-key suffix, bind dynamic cursors to the exact order, authorize all raw cursor fields, and retain existing configured cursor bytes when `orderBy` is absent.

## 0.61.0 (2026-10-23)

Companion runtime/macros version: **0.61.0**.

- Generated single-primary-key `…Changed` subscriptions accept an optional typed `key` argument. Omission retains all authorized changes; a key composes with the action filter. Registration and every delivered event retain current authorization and provenance checks. The generated operation catalogue includes the same argument.

## 0.60.0 (unreleased)

Companion runtime/macros version: **0.60.0**.

- Add explicit `query_shapes = "list,keyset,scan"` selection on generated entities. Omitted query shapes disappear from both the GraphQL root and operation catalogue, allowing a host to retain a differently shaped legacy singleton during migration. Default generation remains unchanged; repository APIs, relations, mutations and subscriptions are unaffected.

- Extend explicit `timestamp = "host"` to public GraphQLEntity fields and ordinary String timestamps (including optional forms). Caller-supplied source timestamps stay unchanged instead of receiving the ORM mutation clock. Existing automatic timestamp behavior remains the default. Policies, hooks and field restrictions still apply.

- Revalidate configured cursor ordering and all composite key fields before SQL
  and before release, independently of the selected payload. Reject hidden raw
  cursor values on GraphQL keyset and scan surfaces.

- Generate static keyset reads and authorized scans for entities with composite primary keys. Configured order must end with the complete primary-key tuple in declaration order, with no duplicate order fields. Every component is retained in cursor ordering; existing authorization, limits and read-only schema posture remain in force.

- Omit unselected query resolver methods entirely, including their GraphQL-only
  argument attributes, so list-only generated roots compile beside legacy roots.

## Operation catalog 0.4.2 / AI tool profiles 0.16.2 (unreleased)

- Carry entity classification and structural export disposition into generated
  result metadata, including paged, keyset, scan and Changed wrappers.
- Keep the complete operator SDL and semantic catalogue while excluding secret
  or entirely nonexportable results from provider capabilities.
- Omit provider relationship selections that cannot reach an exportable scalar
  within their depth and cycle bounds; retain all public capabilities.
- Add a real generated-schema regression for mixed public inventory and secret
  settings, absolute field export bans and private relation rejection.

## 0.57.0 (2026-10-22)

- Reject private configured cursor fields despite explicit read/order surface
  overrides, before GraphQL keyset or scan reads; preserve repository policy.

Companion macros crate: `graphql-orm-macros` **0.57.0**.

- Add the SQL Server-only `#[graphql_orm(mssql_money)]` field domain for exact
  native `MONEY` values as `rust_decimal::Decimal`. Generated entity, relation,
  bulk and typed projection reads cast the selected column to `DECIMAL(19,4)`
  under its original alias before TDS decoding. Validation enforces scale four
  and the signed 64-bit scaled mantissa without rounding. Portable decimal
  precision remains capped at 18; existing declarations and cursor formats are
  unchanged. SQL Server typed read projections are now supported.
- The MONEY domain is static-only: runtime schema validation rejects a runtime
  decimal field declaring it, `RuntimeSchema::from_static_entities` reports a
  static MONEY column as `UnsupportedCapability` instead of converting it to a
  runtime decimal, and a JSON `RuntimeValue::Decimal` carrying it is rejected.
- Test-only: the owned SQL Server fixture never pulls by default
  (`GRAPHQL_ORM_TEST_MSSQL_PULL=missing` opts in), runs without swap, with a
  1024-task PID limit, one bounded 16 MiB json-file log, and an optional
  writable-layer bound (`GRAPHQL_ORM_TEST_MSSQL_STORAGE_SIZE`, for example
  `8G`, rejected by Docker where the storage driver cannot enforce it).
  An optional `GRAPHQL_ORM_TEST_RUN_TOKEN` adds a `graphql-orm.test-run`
  label, and `GRAPHQL_ORM_TEST_MSSQL_EVIDENCE_DIR` receives each exact
  container ID by exclusive creation, so an external controller can clean up
  only the containers its run created.

- Refresh affected workspace/fixture TLS locks to `rustls-webpki` 0.103.16
  and `webpki-roots` / `webpki-root-certs` 1.0.10 for the published emSign
  certificate-distrust fixes. Router 0.6.4 records the changed binary behavior;
  existing dependency findings and their conditions remain independently
  tracked in the [security disposition](docs/reference/dependency-security-disposition.md).

## 0.56.0 (2026-10-21)

Companion macros crate: `graphql-orm-macros` **0.56.0** (version alignment only).
Backend-neutral semantic owner: `graphql-orm-operation-catalog` **0.4.1**.

- Runtime collections, fields and relations accept an optional `description`,
  `classification` and (fields/relations) `export`, validated with the static
  rule (`validate_semantic_description`) and defaulting exactly as the static
  derives: humanized names, `internal`, inherited classification that may only
  strengthen, and `never_export` for `secret` members. Present members join
  `canonical_bytes()`/`fingerprint()`; structural fingerprints, physical
  targets, plan hashes and DDL are unaffected. Absent members are omitted, so
  existing serialized schemas and identities are byte-identical.
- Runtime GraphQL describes every generated type, field, relation, enum value,
  input member and root. Collection, field and relation text is the effective IR
  description; roots and arguments use the static derive wording.
  Introspection returns the same text.
- `RuntimeGraphqlComposer::finish_with_semantic_catalog` returns the schema with
  its `GraphqlSemanticCatalog`: every installed collection's entity (scalar
  fields and to-one relations with filter/order facts, classification and
  export) and generated roots (`KeysetList` queries; prohibited-by-default
  mutations). Host roots declare descriptors with `RuntimeHostField::semantic`,
  `subscription_semantic` and `semantic_entity`; undescribed host roots fail
  closed. The catalogue compiles as an AI query-capability catalogue and
  capability index without separate input.
- Add `RuntimeValueKind::Decimal(DecimalDef)` and
  `RuntimeValue::Decimal(DecimalValue)` with the static precision ceiling (18):
  scaled `i64` storage declared `INTEGER_DECIMAL(p,s)` on SQLite and
  `NUMERIC(p,s)` on PostgreSQL. Values are never rounded. Text input uses one
  strict grammar (`parse_runtime_decimal`); output, GraphQL and JSON use the
  canonical string with exactly `scale` fractional digits
  (`canonical_runtime_decimal`). Add `RuntimeDefault::{decimal, decimal_text}`
  and `RuntimeRecord::{decimal, decimal_value}`.
- Runtime GraphQL serves decimals as the `Decimal` scalar (string or integer
  input, float numbers rejected) with `RuntimeDecimalFilter` (`eq`, `ne`, `in`,
  `notIn`, `lt`, `lte`, `gt`, `gte`, `isNull`); queries also support `between`,
  ordering and keyset cursors. The scalar and filter are registered only for
  compositions containing a decimal field.
- Owned/runtime planning compares PostgreSQL `numeric(p,s)` constant defaults by
  value and refuses, with `InvalidPhysicalContract`, any in-place change of a
  decimal column's definition or a change to or from a decimal type.
- `RuntimeSchema::from_static_entities` converts static decimal columns to
  `RuntimeValueKind::Decimal` (previously `String`) with their defaults.
- Operation catalogue 0.4.1 adds `validate_semantic_description`,
  `GraphqlSemanticOperationDescriptor::generated` and
  `GraphqlSemanticCatalog::compose_descriptors`.

## 0.55.1 (unreleased)

- Refresh the `backend-coexistence` fixture lock for `graphql-orm-ai` 0.109.0.
  Align ORM/macros at 0.55.1. No source, public API, generated code, SDL,
  persistence or behavior change.

## 0.55.0 (unreleased)

- Add typed, bounded runtime literal defaults for UUID, JSON (canonical JSON v1),
  canonical base64 bytes and canonical UTC microsecond date-times, with typed
  constructors (`RuntimeDefault::{boolean,integer,float,string,uuid,json,bytes,datetime}`)
  and `RuntimeDefault::typed`. Full `i64` and finite `f64` domains are preserved;
  float literals keep a fraction or exponent and negative zero follows
  `RuntimeFloat`. Literal encoding and every previously accepted literal are
  unchanged, so serialized schemas, canonical bytes, fingerprints and physical
  targets of existing schemas are byte-identical.
- Rejected defaults report `RuntimeSchemaDiagnostic::detail` with a value-free
  `RuntimeDefaultProblem`; diagnostic messages no longer echo the literal.
- The owned/runtime planner (`plan_owned_migration`, `validate_owned_schema`,
  `runtime_mutation_environment` and owned apply rechecks) compares recognized
  PostgreSQL constant defaults by value (`'…'::uuid`, `'-1'::integer`,
  `'2147483648'::bigint`, `'-1.5'::numeric`, `jsonb`, `bytea` and session-zone
  `timestamptz` text), so typed defaults replan as no-ops and certify. Rendering,
  physical/plan hashes and history are unchanged; static planning and the 8e1
  owned-planning replay recipes keep the pinned comparator byte-for-byte.
- Add owned semantic field formats: `RuntimeField::format` with
  `RuntimeFieldFormat`/`RuntimeFormatNamespace`/`RuntimeFormatPayload`,
  `RuntimeFieldHandle::format` and `ValidatedRuntimeSchema::format_fingerprint`.
  A descriptor participates in `canonical_bytes`/`fingerprint` only when present,
  never in the structural fingerprint or physical target, so adding or editing a
  descriptor is a new schema revision with no DDL.
- Add reviewed runtime GraphQL projections: `RuntimeGraphqlOptions::project_field`
  binds a `RuntimeFieldProjector` by stable field ID to an ORM-registered output
  object; `scalar_format` serves a format as its plain scalar with identical SDL.
  Missing or incompatible projections fail compilation. Projectors run only after
  row/field authorization (and after commit plus output release for mutation
  payloads), batched per layer under `RuntimeProjectionLimits`, and fail the whole
  operation with value-free codes.
- Add `RuntimeMutationEffect::{staged,preimage}` so trusted hooks can stage related
  typed writes atomically from the authoritative transaction image.
- New public fields: `RuntimeField::format`, `RuntimeSchemaDiagnostic::detail`,
  `RuntimeReadCheck::projections`, `RuntimeMutationOutputCheck::projections`,
  `RuntimeGraphqlLimits::projection` and `RuntimeGraphqlCost::projection_calls`;
  new `RuntimeSchemaDiagnosticCode::InvalidFormat`. Align ORM/macros at 0.55.0;
  no generated code, stored-data or automatic database migration.

## 0.54.1 (unreleased)

- Pair ORM/macros 0.54.1 and AI 0.107.4 with published Auth `v0.22.1` at
  `86fabec09eea1eec0f675bc150e90e3809734567` for its finite signed-role-catalogue
  correction. Align direct host fixtures and regenerate their real Cargo locks.
  No generated code, public ORM API, SDL, persistence or AI schema change.

## Router 0.6.3 (unreleased)

- Increase the finite default signed role-catalogue response bound to 2 MiB,
  retaining exact signature/content binding and explicit smaller host bounds.
  This requires the coordinated Auth signature-envelope correction; no new
  router artifact or workspace release is implied by the source change.

## 0.54.0 (unreleased)

- Add backend-neutral `cache_sync` durable generation/freshness metadata, refresh
  policy, bounded collection budgets, existing fenced-lease transitions with
  live clock rechecks after awaits, coalesced worker claims and atomic
  ORM projection/status promotion. Generated status-entity subscriptions reuse
  post-commit delivery and current row/field/entity authorization. Document the
  required subscribe/query and reconnect refetch protocol; no durable replay is
  claimed. Add mandatory-authorizer cache streams with initial registration
  barriers, explicit lag/refetch signals and an owned Auth bridge adapter.
  Align ORM/macros at 0.54.0; no automatic database migration.

## 0.53.0 (unreleased)

- Re-read original status and current authority after binding and successful
  business-policy awaits, retaining every observed expiry ceiling and denying
  any grant narrowing without changing the accepted request.
- Add owned custom operation/release checks with mandatory owned binding and
  business policies, for streams/effects that outlive GraphQL Context.
- Select published Auth v0.22.0 at
  `997707e060bcf5fabf879a5f98146dc2732b1c99` for explicit validated
  current-session proofs that retain the exact original credential reference.
- Align ORM/macros at 0.53.0. No generated code, SDL, stored-data or schema change.

## 0.52.0 (unreleased)

- Add an explicit source-only `replay-scoped-positive-v1` release profile with
  source-qualified evidence reuse and retained audit/runtime gaps. Full release
  validation remains the default; policy merge and publication need separate review.

- Add explicit `OwnedPlanningReplayRecipe` selection, capture and decode APIs for
  new native Rust 1.98.1 evidence using `graphql-orm-owned-planning-8e1-rust1981-v1`.
  The released historical APIs and `graphql-orm-owned-planning-8e1-v1` still
  require exact qualified 1.97.1 and retain their original behavior/defaults.
- Keep format 1 and compiler-native ordinary plan hashes unchanged; distinct
  per-recipe library admission prevents silent compiler-driven recipe selection.
  Rust 1.98.1 Unicode Debug differences are preserved, with separately accounted
  work and fresh independent SQLite/PostgreSQL producer fixtures.
- Align ORM/macros at 0.52.0; generated code, default-off feature graph, external
  dependencies and database schemas are unchanged. No historical receipt upgrade,
  consumer update or release publication is included. Refusal/security/failure
  properties remain inspection-only and runtime-unverified.

## 0.51.0 (unreleased)

- Add explicit sensitive-metadata capture and bounded offline verification of a
  complete successful owned migration plan behind default-off
  `owned-planning-replay`. SQLite/PostgreSQL share the existing engine and
  source-qualified `graphql-orm-owned-planning-8e1-v1` recipe; replay returns
  passive facts. Existing planning/apply behavior, hashes, limits and costs are
  unchanged. No new dependencies, stored-data migration or automatic capture.
- Add complete v1 JSON mapping, private checked limits, per-call custody consent,
  separate redacted errors and cumulative conservative work/text admission.
  Authentic 8e1 positive fixtures retain the actual original observation/output.
- Enforce the qualified Rust/compiler/platform profile inside all replay library
  operations before work begins. Preserve strict nested object/string wire
  shapes after tagged-step buffering, including valid `kind`-last objects; prepay
  their additional decoder work without changing ordinary planning or hashes.
- Align ORM/macros at 0.51.0; generated code is unchanged. Security and failure
  properties have code inspection only, without runtime verification.

## 0.50.2 (unreleased)

- Select the published `agql-auth` `v0.21.3` tag in the shared dependency and
  direct-auth compatibility fixture. The locked source remains
  `59c0a38f82e8c75459474238799a81f29d4ffa58`; authentication behavior is unchanged.
- Align ORM/macros at 0.50.2. Consumers declaring Auth directly must use the
  same tag selector, since Cargo distinguishes a tag from a revision even when
  they resolve to the same commit. No stored-data or schema migration is added.

## 0.50.1 (unreleased)

- Enforce ordinary runtime read/count bind budgets against actual emitted slots
  before database I/O, including SQLite suffix duplication and lexicographic
  cursor expansion on SQLite/PostgreSQL. Preserve SQL and cursor semantics;
  over-budget reads now return the existing `resource_limit` error.
- Align ORM/macros at 0.50.1; no public API or stored-data/schema change.

## 0.50.0 (unreleased)

- Add owned checked filter construction, complete value-free original Boolean and
  field/operator intent, and explicit-profile immutable RuntimeRecord matching.
  GraphQL lowering shares the checked construction path; existing canonical
  error codes, configured scalar limits and null-skip execution semantics remain.
- Conservative SQLite/PostgreSQL profiles use SQL three-valued logic and reject
  unsupported semantics before evaluation; all original dependencies must be
  loaded even when SQL constant folding removed them. No data/schema migration,
  raw JSON parser, native-RLS proof or new automatic authorization is introduced.

## 0.49.0 (unreleased)

- Include the 0.48.1–0.48.3 PostgreSQL migration corrections and SQLite CHECK
  introspection correction described below; those sections retain their separate
  migration and repair requirements.
- Add checked owned-runtime complete-key reads with an explicit required row
  restriction, exact projection and bounded multiplicity detection. Nonfilterable
  keys remain internal identity rather than a public Filter grant. SQLite and
  PostgreSQL reuse current read/auth-context paths; MSSQL rejects before I/O.
- Align ORM/macros at 0.49.0. Hosts still authenticate current authority, authorize
  fields/key-read intent and bind the current trusted catalog before each call.
  No filter-matching API, schema change or stored-data migration is introduced.

## 0.48.3 (unreleased)

- Fail closed before managed PostgreSQL target DDL or history rows for unsupported
  primary-key flags, constraint-backed unique flags and same-type spatial
  metadata alterations. Explicit standalone unique indexes with unchanged flags
  and changed SQL type expressions retain their existing renderer paths.

## 0.48.2 (unreleased)

- PostgreSQL static/owned migration plans now apply real SET, replacement and
  DROP DEFAULT changes, preserve existing row values, and detach old defaults
  before actual type conversion. Unchanged type/nullability/default properties
  emit no unnecessary DDL; canonical schema/hash formats are unchanged.

## 0.48.1 (unreleased)

- Compare complete PostgreSQL TEXT literal defaults with their deparsed `::text`
  spelling without creating a spurious column alteration or rejecting an
  unchanged runtime mutation environment. Preserve exact string values,
  physical/plan hashes, guarded baselines and migration history identities.
- Align ORM/macros at 0.48.1; generated macros and datetime/timestamp contracts
  are unchanged.
- Avoid a SQLite CHECK introspection panic when Unicode appears before the
  constraint, retaining the complete CHECK expression.

## 0.48.0 (unreleased)

- Use the maintained native TLS transport for SQL Server while retaining the
  reviewed Tiberius revision and its query-notification API. MSSQL builds now
  need platform TLS build/runtime dependencies; upgrade trust-store configuration
  as described in the migration guide. SQLite and PostgreSQL are unchanged.
- Add an owned SQL Server certificate/hostname validation regression, including
  an explicit private CA and rejection of an untrusted CA or mismatched hostname.

## 0.47.0 (unreleased)

- Declare the SQLite quickstart example's required backend feature so alternative
  backend target checks select supported examples.

- Default database authorization to `DeclaredPoliciesRequired`; missing marked
  policy providers fail closed. Sensitive applications should select
  `ExplicitPolicyForAllExposedOperations` and install tenant/business policies.
- Add immutable `AgqlAcceptedRequest` injection and coherence validation across
  all request, connection and schema data layers. Preserve the full accepted
  principal separately from safe subject/database projections.
- Add mandatory-dependency current-authority policy, enforcement and custom guard.
  Recheck status, identity, grants, bindings and expiry for each sensitive operation;
  deny changed authority and context-free repository bypasses.
- Align ORM/macros at 0.47.0 and adopt a reviewed full external auth revision.

## 0.46.0 (unreleased)

- Bound generated ordinary/append-only residual-policy offset lists using the
  existing explicit AuthorizedScanConfig. Fetch finite candidate batches, retain
  only the requested visible window and publish exact totals only on exhaustion.
  Missing/exhausted budgets fail closed without partial nodes or totals.
- Decode PostgreSQL native relevance scores as double precision, matching the
  public f64 score contract; exercise real ranked SQL alongside SQLite FTS5.
- Bound protected search with native ranked SQL batches or finite raw fallback
  batches and bounded score-window retention. Require explicit budgets for every
  generated fallback, including hosts using only database RLS without RowPolicy.
  Apply declared SQL read visibility,
  preserve complete native SQL count/page behavior and stable primary-key ties.
- Align ORM/macros at 0.46.0. Add portable actual-row/callback-budget regressions
  and document host configuration, exhaustion errors and AI consumer handling.

## 0.45.0 (unreleased)

- Reauthorize generated ordinary and append-only subscriptions for every event
  using the original request context, current entity/row/selected-field policy,
  scopes and assurance. Release current authorized live rows instead of queued
  snapshots; deny/error events release no identifiers or other metadata.
- Require bounded typed provenance for propagated invalidations, authorizing
  every source row, identifier and relationship field. Suppress missing proofs,
  missing sources and SQL-protected deleted preimages. Add explicit default-deny
  `RowPolicy::can_read_deleted_preimage`; ordinary read permission and RLS-only
  visibility cannot authorize missing-row snapshots. Bound propagation to
  16 hops and 256 examined targets per originating change.
- Separate generated GraphQL emissions from raw in-process event observers.
  Honor action filters and reject the unsupported legacy dummy filter.
- Align ORM/macros at 0.45.0; document subscription migration and delete semantics.

## 0.44.1 (unreleased)

- Refresh compatible TLS/QUIC dependencies and yanked patch releases. Require
  the Decimal release that removes its old optional rkyv bridge; consumers of
  that upstream archival feature must migrate it as described in MIGRATION.md.
  Other unresolved transitive advisories retain explicit reachability review.
- Route generated query, search, keyset, mutation and Federation backend failures
  through the public error sanitizer. Relationship/Federation loaders retain
  safe public categories in a code-only String transport; SQL/schema/record diagnostics stay private.
- Sanitize legacy update/delete/bulk mutation error payloads without changing
  their GraphQL shape. Preserve established safe SQLx public categories and
  log internal detail only through the existing opt-in server diagnostic path.
- Align ORM/macros at 0.44.1. Add portable generated-resolver leak regressions
  covering missing tables, constraints, preloaded/loader relations, Federation,
  pool failure and decode diagnostics with test-owned databases.

## 0.44.0 (unreleased)

- Add opaque structural authorization partitions retaining all identity, policy,
  assurance and claim fields for relation/Federation equality and batch grouping.
  Delimiters, optional empty fields and distinct scope sets cannot merge callers.
- Replace ambiguous external authorization keys and short claim fingerprints with
  versioned `auth-v2:` SHA-256 digests. Normalize role/scope order and duplicates;
  redact structural partition Debug output. Hosts must invalidate prior cache keys.
- Retain SQL type identity and lossless binary bytes in relation keys, separating
  strings, numbers, typed nulls and composite boundaries. Align generated source
  keys and projected SQL row keys on SQLite, PostgreSQL and MSSQL.
- Align ORM/macros at 0.44.0. Existing relationship target authorization checks
  remain covered by cross-crate policy, preload, cache and pagination regressions.

## 0.43.1 (unreleased)

- Require test-owned disposable PostgreSQL/PostGIS/SQL Server infrastructure in
  legacy integration fixtures. Remove ambient URL/default-port access and silent
  no-URL passes; add explicit container opt-in and pool-lifetime cleanup checks.
  Make the operation-catalog example use the selected backend. Library APIs and
  package versions are unchanged by these development-only changes.

- Refresh compatible TLS/QUIC lockfile dependencies to address published
  advisories. Dependency review remains required for unresolved transitive
  packages; this maintenance changes no library API or data format.

- Pin external CI/release actions to full revisions and restrict workflow tokens
  to read-only contents outside the publication job. No library API or migration
  change; all backend/provider validation lanes remain enabled.

- Fix generated repository inserts for host-supplied integer primary keys. Reuse
  typed key binding for bare-pool readback and related single-key lookups/updates,
  preserving supplied keys and String/UUID/binary/composite key behavior.
- Add framework-neutral external-consumer compilation and owned SQLite/PostgreSQL/
  MSSQL execution for exact keys, transaction rollback and MSSQL read-only gates.
- Align ORM/macros at 0.43.1. No new API, feature, physical schema or data migration;
  externally writable SQL Server sources remain outside ORM schema management.

## 0.43.0 (unreleased)

- Reconcile accepted runtime strings/binding and A–D source at aligned 0.43.0.
  Retain immutable reviewed predecessors and execute PostgreSQL prefix/suffix
  CAS, SQLite empty suffixes and opt-in GraphQL conformance in owned backend lanes.

- Add opt-in typed runtime GraphQL mutations delegating to guarded B transactions,
  with atomic host journals and independently authorized selected outputs.
- Add an object-safe, default-deny post-commit record-release callback, explicit
  deleted preimages and sanitized committed-outcome denial/failure/timeout errors.
- Execute repeated runtime GraphQL mutation response-key groups once, merging
  payload projections, preserving distinct-alias order and sharing terminal
  outcomes after failure or cancellation. Budgets charge each group once.
- Align ORM/macros at 0.43.0; static APIs/SDL and default features remain unchanged.

## 0.42.0 (unreleased)

- Fix runtime GraphQL alias swaps and scalar/relation output collisions; retain requested filter capabilities before empty-list constant lowering so authorization cannot be bypassed.

- Runtime GraphQL composition explicitly rejects unsupported custom-directive factories before publication; built-in directives, bounded host extensions and checked subscription roots remain supported.

- Add opt-in runtime GraphQL reads, checked dynamic composition, whole-operation
  host authority preflight and bounded runtime relation batches.
- Add lossless JSON-text/full-i64 scalars and host-managed authenticated encrypted
  cursor framing with trusted schema/order/authorization/parent bindings.
- Align macros at 0.42.0; static derives/SDL and default feature behavior are unchanged.
- C remains an unpublished dependent review checkpoint; custom-directive forwarding
  requires the documented contract decision. D is not included.

## 0.41.0 - unreleased

Companion macros crate: `graphql-orm-macros` **0.41.0** (alignment only).

- Add schema-bound single-record runtime create, patch and exact-key delete on
  SQLite/PostgreSQL pinned state-machine transactions. Reuse existing values,
  handles, typed predicates and row decoding; preserve static repository APIs.
- Require explicit host intent/preimage/result authorization, structural CAS,
  exact return projection grants and verified physical dependency environments.
- Add cancellation guards for runtime mutations and before-commit host journal
  hooks. Caught errors/timeouts cannot commit partial runtime work. Static-only
  transaction behavior and ambiguous commit limitations remain unchanged.
- Count backend-emitted predicate bindings before runtime target reads, including
  duplicated SQLite suffix values and reused PostgreSQL native slots.
- Add conservative transaction commit-outcome inspection and safe runtime error
  codes retained through the public transaction runner.
- Document unsupported modifying cascades and backend capabilities; include a
  disposable create/update/delete/journal example and framework-neutral consumer.

## 0.40.0 - unreleased

Companion macros crate: `graphql-orm-macros` **0.40.0** (alignment only).

- Add owned canonical physical targets from `ValidatedRuntimeSchema`, checked
  static/system composition, explicit `ManagedTableSet`, read-only owned planning
  and validation, and separate guarded `apply_owned_migration` on SQLite/PostgreSQL.
- Keep the public legacy `MigrationStep` enum and struct literals unchanged.
  Static and owned storage share validation, hashing, diffing, risk classification,
  rendering and transactional history application; runtime conversion and owned
  catalog introspection do not leak index storage.
- Bind immutable plans to complete live baselines and ownership, retain destructive
  and additive guards, preserve host PostgreSQL RLS, and reject unsupported live
  preservation cases. Add a verified incoming-dependency environment for later
  runtime mutations; record mutation and dynamic GraphQL are still later slices.
- Reject unrepresentable live FK actions, deferral and dependency scopes with
  `UnsupportedForeignKey`, including incoming system/unowned sources. Recheck on
  the pinned apply transaction so SQLite rebuilds cannot drop omitted constraints
  and runtime dependency certificates cannot mistake `SET DEFAULT` for `Restrict`.
- Preserve a single-connection SQLite in-memory database after a successful owned
  rebuild; discard suspended connections on cancellation or failed restoration.
- Respect SQLite composite primary-key ordinal order during introspection.
- Compare PostgreSQL `TIMESTAMPTZ` and its introspected `timestamp with time zone`
  spelling as equivalent in the shared static/owned planner. DateTime targets now
  replan to no-op and certify after application; timezone-free timestamps and
  precision changes remain distinct. Physical metadata and hash formats are unchanged.
- Reject legacy epoch-second DateTime default conversion with scoped structured
  diagnostics while preserving static storage and supported Integer defaults;
  preserve escaped quote semantics in converted literal defaults.

No automatic application, host catalog, transport, durable delivery or release
publication is introduced. See the owned runtime migration reference and example.

## 0.39.1 - unreleased

Companion macros crate: `graphql-orm-macros` **0.39.1** (alignment only).

- Correct runtime SQLite `ends_with("")` to match every non-null string;
  preserve case-sensitive literal, Unicode and SQL NULL semantics.
- Preserve repeated native PostgreSQL parameter identity at complete-statement
  binding boundaries, including authenticated count/read pairs and transaction
  reads/writes. Retain anonymous/mixed occurrence rebasing and static fragment
  semantics; no public signature, cursor, schema or migration changes.
- Add owned SQLite/PostgreSQL execution, evaluator/page/count/RLS parity,
  normalization and dependency-minimal external-consumer regressions. MSSQL
  runtime capability gates remain unchanged; no live MSSQL claim is made.

## 0.39.0 - 2026-10-03

Companion macros crate: `graphql-orm-macros` **0.39.0**.

- Add repository-only per-field `#[graphql_orm(timestamp = "host")]` for ordinary
  signed Integer timestamp persistence, including Rust/physical aliases, nullable
  inputs, existing conditional/bounded writes and upsert conflict updates.
- Compare the known PostgreSQL-deparsed form of existing ORM epoch-second
  defaults without false column alterations; preserve their values, units and
  runtime Integer/DateTime distinctions.
- Preserve legacy timestamp generation, GraphQL SDL, physical metadata/defaults,
  explicit UUID keys and normal authorization/hooks. Fix generated repository
  upsert field-policy calls to use the repository authorization surface.
- Add owned SQLite/PostgreSQL exact-value, migration no-op, policy, rollback and
  whole-transaction cancellation regressions plus standalone SQL-free consumers.
  No unfinished-operation poisoning or runtime A–D capability is introduced.

## 0.38.0 - 2026-10-03

Companion macros crate: `graphql-orm-macros` **0.38.0**.

- Add PostgreSQL `fetch_group_page` execution for one TEXT/VARCHAR grouping
  field, preserving exact original values with explicit bytewise equality and
  ASCII-only case-insensitive ordering plus a binary tie-breaker. Preserve
  SQLite native group equality, ordering, cursor wire format and page bounds.
- Let ordinary aggregates, including ungrouped COUNT/SUM and GraphQL aggregate
  roots, consume current explicit unrestricted or complete SQL read visibility.
  Intersect policy and caller predicates before grouping; retain entity/field
  authority and reject residual authorization before query execution.
- Add a provided aggregate entity-identity hook, generated by the macro without
  requiring changes to existing handwritten aggregate-field implementations.
- Execute over 1,000-group SQLite/PostgreSQL parity, nullable/metric/cursor/policy
  regressions and a framework-neutral standalone event-enumeration example.
  MSSQL pages remain explicitly unsupported. Separate reads do not constitute a
  cross-request snapshot under concurrent writes; see the aggregate reference.

## 0.35.2 - 2026-10-01

- Keep standalone fixture lockfiles aligned with this package set and execute
  transaction count authorization regressions on test-owned PostgreSQL as well
  as SQLite during release validation.

Companion macros crate: `graphql-orm-macros` **0.35.2** (alignment only).

- Allow transaction-bound repository `count` and `exists` with an installed
  row policy when its current visibility is explicitly unrestricted or complete
  SQL authorization. Intersect complete predicates with caller filters before
  the database count, on the same pinned transaction as surrounding writes.
- Retain entity authorization and reject callback-only/prefilter policies,
  wrong-entity predicates and residual filters under an installed row policy.
  Counts do not decode rows or invoke per-row callbacks. Existing no-policy
  behavior, public APIs, GraphQL SDL and stored data are unchanged.

## 0.35.1 - 2026-10-01

Companion macros crate: `graphql-orm-macros` **0.35.1**.

- Allow typed repository projections under an installed global row-policy provider
  when current visibility is explicitly unrestricted or complete SQL authorization.
  Apply entity-bound predicates before limits and primary/unique-key lookups,
  including pinned transactions, without selecting excluded private fields.
- Retain fail-closed callback-only/prefilter policies and normal entity/selected-field
  checks. Add a provided generated-identity hook without changing existing trait
  implementation requirements, static GraphQL SDL, storage or cursor formats.
- Add executable private repository projection/group examples and isolated
  SQLite/PostgreSQL regression evidence; observe actual projection query counts.

## 0.35.0 - 2026-10-01

Companion macros crate: `graphql-orm-macros` **0.35.0** (alignment only).

- Add complete bounded SQLite text-group enumeration through the existing
  aggregate builder, with native group equality, stable NOCASE/BINARY ordering,
  typed host-context cursors and continuation after grouping.
- Apply current complete SQL row visibility before grouping, retaining field/entity
  authorization and fail-closed residual policies; bound pages to 1–1,000 plus
  one lookahead. Keep ordinary aggregate APIs/GraphQL SDL/cursors unchanged.
- Add a standalone private repository consumer example without query SQL/direct
  async-graphql and SQLite execution/unsupported-backend regressions. Other scalar
  groups/backends and computed SQL Server summaries remain separate capabilities.

This static feature remains independent of runtime A–D. The 0.35.0 package
identity preserves the independently reviewed 0.33.4/0.33.5 compatibility fixes;
release coordination does not imply completion of the broader static query plan.

## 0.33.5 - 2026-10-01

Companion macros crate: `graphql-orm-macros` **0.33.5**.

- Fix generated relationship resolvers returning preloaded objects before current
  target authorization and complete ownership-key resolution. Resolve the
  authoritative database target even when a snapshot/cache was populated.
- Check current entity, row and selected-field policies. Nullable target denial
  preserves the parent and reports a sanitized error on the relationship path.
  Missing or ownership-mismatched targets ordinarily return null without errors.
- Apply SQL visibility before relation pages/counts, partition batches by current
  predicate and database identity, and retain the existing uncached loader type.
  Callback-only to-many policies require an explicit host scan budget and fail
  closed on exhaustion; retain only the bounded visible page in memory.
- Add executable cross-crate SQLite/PostgreSQL regressions and a SQL-free
  authoritative relationship example. Existing SDL/cursor formats are unchanged.

No stored-data migration or release publication is included.

## 0.33.4 - 2026-10-01

Companion macros crate: `graphql-orm-macros` **0.33.4**.

- Fix generated relationships to an entity defined in another Rust crate.
  Placeholder rendering uses the existing public `OrmBackend` contract, avoiding
  calls to the target's crate-private helper. Existing helper visibility, backend
  SQL, static SDL and application APIs remain unchanged.
- Add an external two-crate fixture and a SQL-free disposable example. SQLite
  execution tests complete tenant/endpoint bindings, historical snapshots,
  missing/optional targets, independently checked target entity denial and native
  statement tracing of batched reads. PostgreSQL/MSSQL lanes are compile checks.

No stored-data migration or release publication is included.

## 0.33.3 - 2026-10-01

Companion macros crate: `graphql-orm-macros` **0.33.3**.

- Fix `RepositoryEntity` aggregate-field enums retaining the async-graphql
  `Enum` derive and helper attribute. Repository consumers compile without a
  direct async-graphql dependency; plain Rust aggregate enums/builders retain
  their existing traits, fields, SQL behavior and authorization checks.
- Preserve ordinary `GraphQLEntity` aggregate SDL. Add external scalar/composite
  consumer fixtures and disposable SQLite/PostgreSQL aggregate evidence.
- Correct package identities after the independently merged compatibility-fixture
  release bump had already consumed 0.33.2. Runtime and macros remain aligned.

No data migration is required. This maintenance change publishes no release.

## 0.33.2 - 2026-10-01

Companion macros crate: `graphql-orm-macros` **0.33.2**.

- Advance runtime and macros together after the external migration compatibility
  fixture changed the runtime package source tree. This preserves immutable
  package-release identities and refreshes fixture lockfiles.
- Runtime and generated-code behavior, public APIs, GraphQL SDL and stored-data
  contracts remain unchanged. No data migration is required.

## 0.33.1 - 2026-09-28

Companion macros crate: `graphql-orm-macros` **0.33.1**.

- Align the owned Federation integration fixture with router 0.5.3 and Hive
  0.2.19, including the fixed `domain` 0.12.3 dependency.
- Advance runtime and macros together to preserve immutable package source
  identities after the fixture change. Runtime and generated-code behavior,
  public APIs, GraphQL SDL, and stored-data contracts remain unchanged.

No data migration is required.

## 0.33.0 - 2026-09-21

Companion macros crate: `graphql-orm-macros` **0.33.0**.

- Added request-local `RowPolicy::read_visibility`, typed parameterized
  predicates, explicit complete/partial/callback/unrestricted decisions, and
  validated EXISTS relations. Existing policies default to callbacks.
- Generated ordinary lists apply complete visibility before SQL pagination
  and exact counts. Legacy callback-visible offsets and totals remain exact.
- Added opt-in bounded authorization scans with encrypted continuation,
  explicit budget/exhaustion states, and no implicit exact count work.
- GraphQL and standalone repository keyset connections accept complete SQL
  visibility and constrain counts/probes consistently. Residual authorization
  uses the scan API; transaction keyset helpers retain their existing contract.
- Added query observation, integration coverage, and a reproducible large-data
  release benchmark. All callback-required columns remain loaded.

See the [pagination guide](docs/reference/graphql-orm/pagination-migration.md)
for backend/capability boundaries, consistency, and migration details. The new
scan fields extend keyset-enabled GraphQL SDL and operation catalogues.
No stored-data migration is required.

## 0.32.0 - 2026-09-17

Companion macros crate: `graphql-orm-macros` **0.32.0**.

- Added `#[graphql_entity(federation_key)]`, which opts an entity into being a
  resolvable Apollo Federation v2 entity. The bare form keys on the primary
  key; `federation_key(fields = ["A", "B"])` names exported GraphQL fields, and
  the attribute repeats for several keys. `assume_unique = true` accepts a
  unique constraint enforced by an externally managed schema the ORM does not
  declare. Declarations are validated when the macro runs, and a key declared
  without `GraphQLOperations` is now a compile error.
- Generated one `#[graphql(entity)]` resolver per key. It reproduces the
  generated single-row read authorization chain unchanged, so a missing row and
  a row-policy denial both resolve to `null` while entity-policy, scope,
  assurance, and authentication denials fail the `_entities` fetch. All
  representations of one entity type in one fetch collapse into one statement.
- **`PageInfo` is now `@shareable`.** Every subgraph's exported SDL changes
  from `type PageInfo {` to `type PageInfo @shareable {`, whether or not it
  uses Federation. Without it, two `graphql-orm` subgraphs could not compose.
- **Declaring a key adds `_entities` and `_service` to introspection**, because
  async-graphql enables federation as soon as a resolvable key exists. The
  federation SDL export still omits them.
- Added `federation: true` to `schema_roots!` for a subgraph whose only
  Federation participation is an `unresolvable` reference stub and which
  therefore has no resolvable key to enable federation by itself.
- The operation catalogue and the router-protocol descriptor are unchanged by a
  key declaration, and `_entities`/`_service` remain outside the router's
  root-field authorization contract. A joined field is authorized only by the
  subgraph that owns the entity; see
  [ADR-0011](docs/decisions/ADR-0011-generated-federation-entity-keys.md).
- No persisted schema or data migration.

## 0.31.1 - 2026-09-15

Companion macros crate: `graphql-orm-macros` **0.31.1** (version alignment only).

- Upgrade the optional SQL Server notification parser to quick-xml 0.41, which
  fixes RUSTSEC-2026-0194 and RUSTSEC-2026-0195. The existing 16 KiB notification
  bound, plain-reader parsing and explicit Broker authority remain unchanged.
  Explicit decode/unescape retains control-character rejection rather than
  silently normalizing literal attribute whitespace with the newer XML API.
- No public API, generated GraphQL, persisted schema or data migration changes.
  The independently distributed router retains its separate dependency review.

## 0.31.0 - 2026-09-07

Companion macros crate: `graphql-orm-macros` **0.31.0** (version alignment only).

- Added native SQL Server Query Notification registration and a dedicated,
  explicitly authorized Service Broker connection. Existing entity pools and
  external schema ownership remain unchanged; no Broker objects are created.
- Added bounded parsed invalidation, expiry, and invalid-registration messages,
  plus disposable SQL Server integration coverage for aggregate subscriptions.
- Pinned the Tiberius notification feature to an immutable Git revision.

## 0.30.0 - 2026-08-31

Companion macros crate: `graphql-orm-macros` **0.30.0**. Backend-neutral
semantic owner: `graphql-orm-operation-catalog` **0.4.0**.

- Corrected generated calendar filters to use sargable half-open timestamp
  ranges. `IsToday`, `InFuture`, `RecentDays`, `WithinDays`, and relative upper
  bounds now include or exclude complete calendar dates as documented without
  applying a function to the persisted column.
- Added one recursive `DatabaseFilter::validate` execution seam. Generated
  filters reject zero, negative, or excessive calendar spans, excessive signed
  offsets, malformed or reversed ranges, and GraphQL `Between` inputs missing
  either required bound. Invalid direct rendering produces a false predicate;
  query execution returns `INVALID_INPUT` before database work.
- Made SQLite spatial fallback evaluate every date operator with one
  deterministic UTC calendar anchor and SQL three-valued NULL semantics across
  direct, `And`, `Or`, and `Not` expressions.
- Corrected semantic date-field detection to take precedence over the Rust
  backing type. Date fields now advertise the exact generated `DateFilter`
  grammar, including calendar and relative operators and excluding unsupported
  membership and string operators.

PostgreSQL derives today from the connection session's `CURRENT_DATE`; SQL
Server uses the server-local date from `GETDATE()`; SQLite uses UTC through
`date('now')`, with the same UTC basis in spatial fallback. Exact comparison
values remain unchanged and unnormalized. No table, column, constraint, or
stored-data migration is required. Regenerate SDL, semantic catalogues, and
dependent fingerprints.

## 0.29.0 - 2026-08-31

Companion macros crate: `graphql-orm-macros` **0.29.0**.

- Added repeatable, server-defined `order_expression` declarations for safe
  computed-field ordering. GraphQL clients select only direction; fixed SQL
  expressions are validated at compile time and never accepted from requests.
  Expressions may declare `:named` binds resolved by an entity-owned function
  from GraphQL server context; raw placeholders and missing values fail closed.
- Generated entity and relation ordering appends missing primary-key columns as
  ascending tie-breakers, keeping equal computed values stable across
  limit/offset pages.
- Added opt-in `order_aggregate(name = "...", aggregate = "count")` on
  relations. It generates a direction-only order field backed by a correlated
  count over the relation's declared source and target keys.
- Added opt-in `graphql_complex_object` composition so handwritten complex
  fields and generated, batched `GraphQLRelations` resolvers share one
  async-graphql complex-object implementation.

No database or stored-data migration is required. Adding an order expression
or relation aggregate changes the entity's GraphQL order-input SDL; adding
complex-object composition preserves the existing handwritten and relation
field SDL.

## 0.27.0 - 2026-08-26

Companion macros crate: `graphql-orm-macros` **0.27.0**.

- Added opt-in, compile-time `source_condition` and `target_condition` predicates
  for relationships over externally managed polymorphic reference columns.
  Generated single, pageable, DataLoader, and nested bulk-preload paths enforce
  the same fixed predicate with bound values.
- Qualified generated relation-loader identities by source entity, preventing
  equal relationship field names on different parent types from sharing an
  incompatible cached result.
- Conditional relationships must set `emit_fk = false`; they describe a
  resolver join, not an unconditional physical foreign key.

Existing relationship declarations and GraphQL SDL are unchanged. No database
or stored-data migration is required. Consumers that add conditional
relationships should regenerate semantic catalogues and dependent capability
fingerprints.

## 0.26.0 - 2026-08-22

Companion macros crate: `graphql-orm-macros` **0.26.0**. Generated ORM and
database behavior are unchanged.

- Aligned the optional `auth-agql` bridge to generic `agql-auth` 0.18.0 at
  merged full revision `527d15d28e3c295a6f6b5e6d74559a7aecdc1322`, retaining
  one public auth type universe for direct consumers.
- The upstream release adds a typed authorization-role claim and issuer hook,
  explicit unknown-role expansion errors, and independent catalogue lifetime
  validation. The ORM bridge remains a projection of an accepted principal;
  database policy and generated resolver behavior are unchanged.

No database, GraphQL SDL, generated-code, or stored-data migration is required.

## 0.25.1 - 2026-08-22

Companion macros crate: `graphql-orm-macros` **0.25.1**. Generated ORM and
database behavior are unchanged.

- Aligned the optional `auth-agql` bridge to `agql-auth` 0.17.1 at merged full
  revision `b792241b7d9bb46bad81dde4534ae5b39857f614`, retaining one public auth
  type universe for direct consumers.
- The upstream patch omits only default-valued session-context fields during
  serialization. Bridge projection, authorization, schema, database policy,
  and generated resolver behavior are unchanged.
- The companion macro version remains aligned as required by the workspace
  release contract; its emitted code and public behavior are unchanged.

## 0.25.0 - 2026-08-22

Companion macros crate: `graphql-orm-macros` **0.25.0**. Generated ORM and
database behavior are unchanged.

- Aligned the optional `auth-agql` bridge to `agql-auth` 0.17.0 at merged full
  revision `82650a447f3b6875160254fa1293b3a0e305c224` so hosts share the neutral
  role-scope catalogue and principal type universe.
- The bridge projection, assurance evaluation, database policy, schema, and
  generated resolver behavior are unchanged.
- The companion macro version remains aligned as required by the workspace
  release contract; its emitted code and public behavior are unchanged.

## 0.24.0 - 2026-08-22

Companion macros crate: `graphql-orm-macros` **0.24.0**. Generated ORM and
macro behavior is unchanged.

- Updated the optional `auth-agql` dependency to `agql-auth` 0.16.0 at merged
  revision `3bc38cd94794f1e868a9cc3a5551047b95a32105`. The bridge remains a
  projection of an already accepted principal and does not select matcher
  policy or gain token-issuer responsibilities.
- Hosts that enable `auth-agql` and also depend directly on `agql-auth` must
  use the same exact revision so public principal and assurance types resolve
  from one source universe.
- Collapsed an equivalent macro-generation branch and removed a redundant
  doctest wrapper to remain warnings-clean under the pinned Clippy toolchain;
  emitted tokens and the compile-fail probe are unchanged.

No database, GraphQL SDL, generated-code, migration-history, backup, token, or
stored-data migration is required.

## 0.23.0 - 2026-08-14

Companion macros crate: `graphql-orm-macros` **0.23.0**. Backend-neutral
semantic owner: `graphql-orm-operation-catalog` **0.3.0**.

- Added a closed `GraphqlSemanticCollectionBound` on Many relationships so a
  pageable paging argument and a resolver-owned server-fixed list ceiling are
  distinct fingerprinted contracts. Generated to-many relationships declare
  `Pageable`; `GraphQLSemanticObject` object lists with `maximum_items`
  declare `ServerFixed`.
- Semantic catalogue wire version is now `2`. Version `1` payloads and
  catalogues that omit a Many collection bound fail closed.

## 0.22.1 - 2026-08-13

Companion macros crate: `graphql-orm-macros` **0.22.1**.

- Fixed generated to-many relationship semantic metadata so its nullable
  `OrderBy` object exactly matches the relationship resolver SDL. Generated
  root list queries retain their existing list-valued `OrderBy` contract.
- Relationship resolver signatures and semantic argument descriptors now use
  one macro-owned contract for public names, nullability, `Where`, `OrderBy`,
  and `Page` types, preventing independent drift across case conventions and
  backends.
- Added a complete PascalCase schema/catalogue/capability regression covering
  single-key, composite-key, one-to-one, nullable and to-many relationships.
- Optional relationship-key extraction now emits the idiomatic `?` form so
  generated consumers remain warnings-clean under current Clippy.

## 0.22.0 - 2026-08-13

Companion macros crate: `graphql-orm-macros` **0.22.0**. Backend-neutral
semantic owner: `graphql-orm-operation-catalog` **0.2.0**.

- Added a versioned, strict `GraphqlSemanticCatalog` containing public entity,
  field, relationship, argument, type, capability, classification, export, and
  generated/custom root-operation semantics. Canonical JSON fingerprints and
  the optional router descriptor extension bind the complete descriptive graph.
- `GraphQLEntity` now emits selectable/filterable/sortable/groupable/aggregate
  facts, typed relationship cardinality and bounds, inherited classifications,
  field-policy presence, and structural non-exportability. Private and
  `read = false` fields remain absent; `sensitive` fields become `Secret` and
  `NeverExport`.
- Generated root and object documentation now use the same bounded semantic
  descriptions. `#[graphql_orm_custom_operations]` publishes equivalent
  metadata beside handwritten resolvers, and `schema_roots!` can compose those
  declarations through `semantic_custom_operations`. Handwritten result
  objects derive `GraphQLSemanticObject` and compose through `semantic_types`.
- `schema_roots!` now offers single-source `described_*_types` lists that
  compose a handwritten root together with its operation and direct result
  semantics. Custom scalar/enum results carry explicit fingerprinted
  classification/export metadata; unclassified leaves default to
  `Secret`/`NeverExport`, and exportable scalar lists require a positive bound.
- Generated aggregate operation descriptors bind their owning public entity.
  AI aggregate disclosure and runtime validation use only the exact selected
  grouping fields and metric field/operator pairs, preventing unrelated or
  drifted fields from affecting or weakening provider egress.
- Subscription semantics truthfully distinguish best-effort delivery from a
  bounded replay-then-live declaration. Generated broadcast subscriptions
  remain best-effort; a replayable custom declaration still requires a
  separately registered authoritative runtime source.
- Added canonical aggregate semantic enums and the opt-in `Aggregate`
  operation category for the typed aggregate implementation to consume.
- Added the canonical `AiMutationExecutionPolicy` semantic classification.
  Generated `ai_mutations(...)` category declarations and handwritten
  `graphql_orm_custom_operations(ai_execution = "...")` declarations select
  `Automatic`, `ApprovalRequired`, or the default `Prohibited` state. The
  classification participates in semantic fingerprints but grants no runtime
  authority.
- Added database-executed typed grouped aggregates with multiple `COUNT`,
  `MIN`, `MAX`, and `SUM` metrics, deterministic nullable-key ordering, bounded
  result groups, generated-filter and field-policy checks, and portable exact
  Decimal values. `aggregate = true` opts an entity into a generated GraphQL
  aggregate root and operation-catalog entry; existing schemas gain no root by
  default.
- Refactored write and transaction capabilities away from SQLx-specific
  traits. SQL Server now supports deliberate `ExternalWritable` entity DML
  through native Tiberius connections while every compatibility constructor
  remains physically read-only. The applicable generated surface includes
  scalar/composite insert, update, delete, upsert, bounded mutation,
  insert-if-absent, versioned compare-and-swap, bulk helpers, hooks, events,
  subscriptions, commit and rollback.
- SQL Server upserts use a locked update/insert transaction rather than
  `MERGE`; transactions discard a Tiberius client after cancellation or an
  indeterminate error. Fixed-decimal and JSON values have native validated
  MSSQL bind/decode paths.
- Closed a generated-write authorization race across SQLite, PostgreSQL, and
  SQL Server. Authoritative row lookup, row policy, input transformation,
  before hooks, and exact-key DML now share one pinned mutation transaction;
  predicate writes cannot capture newly matching rows after authorization.
  Top-level upserts use state-machine isolation, while caller-owned default
  transactions fail closed before an unfenced upsert.
- Logical backups represent Decimal columns as exact validated Decimal values,
  including precision and scale. SQLite scaled integers and PostgreSQL native
  numerics now share one canonical backup form, and restore rejects changed
  definitions or lossy values.
- Added local, individually selectable provider and owned-database acceptance
  runners, complete workspace package/SemVer release-policy coverage, and a
  deterministic release-manifest row for semantic-catalogue wire version 1.
  The release BOM also records the canonical automatic query, mutation, and
  subscription capability contract versions in stable name order.
  The coordinated compile fixture consumes semantic, grouped-aggregate, and
  deliberate MSSQL writable contracts without a downstream application.

The semantic catalogue is discovery and disclosure-shape metadata only. It
does not authorize resolvers, fields, rows, provider egress, or database work.
There is no database, stored-data, or migration-history change. Decimal-aware
logical backup descriptors and schema hashes are new in this release. SDL
descriptions and semantic/catalogue fingerprints change where new generated
documentation participates. MSSQL write adoption is an explicit application
configuration change against an externally managed schema; the ORM still does
not create or migrate SQL Server tables.

## 0.21.1 - 2026-08-13

Companion macros crate: `graphql-orm-macros` **0.21.1** under the aligned
Git-only version policy. The generated macro contract is unchanged.

- Updated the one-way external `agql-auth` integration to 0.15.0 at exact
  revision `e841ffd382082ad7419be259fe957f949b956ff7`.
- The existing ORM principal, scope, assurance, actor, tenant, and database
  authorization projections remain unchanged. Hosts can now share the same
  auth type universe with applications using the reusable session-bound,
  access-token-only delegation API.
- Backend-coexistence coverage proves the SQLite AI runtime, MSSQL application
  schema, optional ORM auth bridge, and direct auth consumer resolve exactly
  one reviewed `agql-auth` source.

No database, GraphQL SDL, generated-code, migration-history, backup, or stored
data migration is required.

## 0.21.0 - 2026-08-11

Companion macros crate: `graphql-orm-macros` **0.21.0**. New independently
consumable packages: `graphql-orm-operation-catalog` **0.1.0** and
`graphql-orm-ai-tool-profiles` **0.1.0**.

- Generated resolver-operation metadata now has a backend-neutral canonical
  owner and remains re-exported unchanged from `graphql-orm`.
- Reviewed AI GraphQL profile compilation and manifest wire contracts now have
  a database-neutral package. MSSQL application subgraphs can publish exact
  manifests in the same Cargo invocation as a SQLite AI runtime without
  selecting an AI persistence backend or duplicating serialized types.
- `#[backend_selected_graphql_entity(...)]` lets a reusable companion crate
  bind derives to its own mutually exclusive host backend feature when Cargo
  has unified other `graphql-orm` backend features elsewhere in the workspace.

Existing ORM imports and operation fingerprints remain source/wire compatible.
There is no database, GraphQL SDL, migration-history, backup, or stored-data
migration.

## 0.20.0 - 2026-08-11

Companion macros crate: `graphql-orm-macros` **0.20.0** under the aligned
Git-only version policy. This additive pre-1.0 minor exposes semantic entity
metadata and aligns the optional router protocol dependency to 0.2.0.

- `#[graphql_entity(description = "...")]` and
  `#[graphql_orm(description = "...")]` now emit bounded public semantic
  descriptions through `Entity::graphql_semantic_metadata`. The projection
  contains public GraphQL field identities and explicit relationship shape,
  but no physical table/column names or authorization policy. It remains
  discovery/documentation metadata and grants no field, resolver, row, or AI
  authority.
- Handwritten `Entity` implementations remain source compatible through the
  default `None` semantic-metadata method.
- The optional `router-protocol` adapter now resolves
  `graphql-orm-router-protocol` 0.2.0. Existing protocol v1 operation exports
  are unchanged.

No database, GraphQL SDL, migration-history, backup, or stored-data migration
is required. Description changes do not alter physical schema hashes.

## 0.19.0 - 2026-08-10

Companion macros crate: `graphql-orm-macros` **0.19.0** under the aligned
Git-only version policy. This release changes public physical-schema model
types and generated migration metadata, so it is a pre-1.0 minor release.

- Added ordered compound physical foreign keys. Relation lowering now retains
  every `from`/`to` pair, translates source Rust fields through `db_column`,
  validates the exact referenced primary or unique key, and renders one
  compound constraint with its configured delete policy.
- SQLite introspection now groups `PRAGMA foreign_key_list` members by
  constraint ID and sequence. PostgreSQL introspection pairs `conkey` and
  `confkey` members by ordinal through `pg_constraint`. Both backends therefore
  round-trip compound foreign keys without flattening them.
- Added stable, optional ordinary-index names and typed per-column `ASC`/`DESC`
  metadata. SQLite `index_xinfo` and PostgreSQL `indoption` introspection retain
  direction, and planner drift recreates an index whose order differs.
- Added exact numeric `min_exclusive` and `max_exclusive` field checks and
  `default = false` for suppressing the conventional implicit timestamp
  default when adopting an existing column without one.
- Added conservative semantic comparison for supported simple check
  expressions. Physical constraint names, whitespace, identifier quoting, and
  redundant outer parentheses do not force DDL, while changed or unrecognized
  expressions remain drift.
- Existing semantically equivalent SQLite layouts can now be recorded by the
  ordinary empty-plan migration-history path. There is no generic adoption
  override: partial, reordered, differently targeted, weakened, or ambiguous
  foreign keys/checks remain migration work or fail closed.
- Physical-schema validation now rejects malformed index-direction arity and
  foreign keys whose members are absent, duplicated, type-incompatible, or do
  not reference an exact managed unique key.

No stored row rewrite is required. Fresh SQLite and PostgreSQL schemas use the
new metadata directly; compatible existing SQLite schemas are adopted only
after live introspection proves semantic equality. Schema hashes change where
check, foreign-key, or index-order metadata participates. See the
[0.19.0 migration guide](MIGRATION.md#0190-compound-foreign-keys-and-directional-indexes)
for the source migration from public struct literals.

## graphql-orm-router 0.1.1 - 2026-08-07

This patch fixes variable-backed argument scope templates on authenticated
HTTP and `graphql-transport-ws` operations. Subscription authorization now
uses the variables from the individual operation and rejects a missing
rendered scope before opening a subgraph connection. Client attempts to supply
router-reserved internal metadata are overwritten. No configuration, protocol,
schema, public API, or stored-data migration is required; subgraph resolver
guards remain authoritative.

## 0.18.0 - 2026-08-07

Companion macros crate: `graphql-orm-macros` **0.18.0** under the aligned
Git-only version policy. The new `graphql-orm-router-protocol` and
`graphql-orm-router` packages begin at **0.1.0**. This release adds public ORM
metadata, macro declaration/generated-code behavior, and optional router
integration, so it is a pre-1.0 minor release. No database or stored-data
migration is required.

- Updated the one-way external `agql-auth` integration to 0.14.0 at exact
  revision `413fda3435f060604cd653c11e2cc18a668aace1`. Its validator now
  normalizes standard OAuth `scope` and bounded legacy `scopes` claims into the
  existing principal scope vector; the ORM bridge API and stored data are
  unchanged. Direct JWT decoders must follow the upstream rolling migration.
- Hardened `graphql-orm-router` for standalone operation with strict JSON and
  environment-only secrets, pre-bind checks, public/downstream deadlines,
  connection and graceful-drain budgets, structured telemetry, authenticated
  core metrics, optional Prometheus execution/subscription metrics, signal
  handling, operator/schema/reconnect/threat-model guidance, migration notes,
  release-policy lanes, and an executable HTTP/WebSocket/shutdown smoke test.
- Added identity-bound dynamic registration and a separately bound,
  scope-protected administrative surface with deny-by-default SSRF policy,
  safe status/metrics, explicit refresh/removal, request limits, and documented
  process-local restart semantics.
- Added conditional schema and authorization-metadata polling to
  `graphql-orm-router`, with canonical no-op fingerprints, bounded retry,
  serialized complete-candidate admission, exact executable last-known-good
  retention, process-local refresh/removal/status APIs, and atomic
  graph-plus-policy replacement across HTTP and WebSocket work.

- Added authenticated `graphql-transport-ws` serving to
  `graphql-orm-router` on the public GraphQL path. Connection-init credentials
  are verified before acknowledgement, operations reuse graph-bound scope
  policy, the approved token is propagated to an upstream WebSocket subgraph,
  and connections close at expiry without in-place refresh. Connections,
  operations, client messages, upstream buffers, and ephemeral fan-out are
  bounded; no events are persisted or replayed.

- Added fail-closed HTTP authentication and advisory authorization to
  `graphql-orm-router`: bounded rotating RS256 JWKS validation, explicit
  standard/legacy scope migration, graph-bound operation metadata, fixed and
  scalar-templated scope preflight, approved bearer propagation, and an
  optional one-way exact-pinned `agql-auth` validator/matcher adapter. The
  router remains resource-server-only and subgraph guards remain authoritative.
- Added a public `graphql-orm-router` static HTTP graph with validated
  fail-closed configuration, bounded credentialed SDL retrieval, atomic graph
  preparation and identity, configurable GraphQL path, downstream-header
  allowlisting, and liveness/readiness endpoints. Anonymous access requires an
  explicit development-only opt-in.
- Added repeatable authorization declarations for every generated operation
  category, including fixed `all_scopes` and any-of/all-of requirements plus
  argument-dependent `all_scope_templates` and `any_scope_templates`.
  Declarations drive server-side resolver enforcement, protocol metadata, and
  native Federation scope metadata where representable. Invalid categories,
  missing generated operations, malformed placeholders, unknown arguments,
  and unsupported complex substitutions fail at compile time.
- Versioned generated-operation authorization fingerprints as v2 so templated
  policies bind the GraphQL type and requiredness of referenced arguments
  without changing the established discovery fingerprint.
- Generated operations with entity-level `auth = "required"` now emit the
  standard namespaced Federation `@federation__authenticated` directive. The
  router structurally restores the composed `authenticated` and
  `requiresScopes` SECURITY links required by Hive enforcement.
- Added deterministic authorization and router-export fingerprints without
  changing the existing generated-operation discovery fingerprint.
- Added optional `router-protocol` export support. Ordinary `graphql-orm`
  consumers do not resolve the protocol package unless the feature is enabled.
- Applied the existing authentication, assurance, and entity read-policy
  guards consistently to append-only generated subscriptions.
- Added integration evidence that generated subscription events are released
  only after commit and are discarded when commit rolls back.

## 0.17.0

Companion macros crate: `graphql-orm-macros` **0.17.0** under the aligned
Git-only version policy. This additive public runtime and generated-code
surface is a pre-1.0 minor release.

- Added provider-neutral operation assurance registries for generated and
  custom root fields, configurable interactive-mutation defaults, explicit
  interactive/machine/service/safety-teardown actor classes, requirements,
  and documented exemptions.
- Added strict completeness audits that fail when an exposed mutation has
  neither a requirement nor exemption. Compatibility mode remains the default;
  queries and subscriptions receive no assurance default.
- Added provider-neutral schema directive definitions/metadata and a
  deterministic advisory client manifest containing exact field identity,
  policy ID, actor class, custom/generated origin, and exemption reason.
- Generated resolvers now call a generic assurance enforcement hook before
  database work. The hook is a compatibility no-op until a schema installs
  `AssuranceEnforcement`; `DeclaredAssuranceGuard` gives custom fields the same
  integration.
- Extended the optional one-way `auth-agql` bridge with
  `AgqlAssuranceEvaluator`, pinned to upstream revision
  `d6b9cef663d52125c52f3fb90d4155ee25d34775`. It evaluates the upstream
  `AssuranceRequirement` with the current user and injected clock, then emits
  lowercase GraphQL extension key `code` with `STEP_UP_REQUIRED`,
  `UNAUTHENTICATED`, or `FORBIDDEN`.
- Added generated/custom mutation, machine principal, safety exemption,
  strict audit, manifest determinism, and stable error-code coverage.

Existing resolver names, SDL, database schema/data, authorization/RLS, and
runtime behavior remain unchanged until assurance enforcement is explicitly
installed. The manifest is advisory; server enforcement is authoritative.

## 0.16.0

Companion macros crate: `graphql-orm-macros` **0.16.0** under the aligned
Git-only version policy. This additive public runtime and generated-code
surface is a pre-1.0 minor release.

- Added `GraphqlOperationMetadata` and immutable generated descriptors for
  every query, mutation, and subscription resolver actually emitted by
  `GraphQLOperations`, including exact case-adjusted root field/argument names,
  stable categories, Rust/GraphQL input/result signatures, backend/entity
  identity, and derive-owned schema declarations.
- Added `graphql_orm_operation_catalog()` generation to `schema_roots!`.
  Catalog descriptors distinguish generated mutations omitted by
  `generated_mutations` none/allowlist/denylist policy from operations actually
  merged into the public root. Root-level read-only policy also resolves
  generated subscriptions as unexposed.
- Added domain-separated, length-framed SHA-256 generated/resolved/catalog
  fingerprints with deterministic ordering and documented compatibility
  limits. Fingerprints detect generated surface and exposure drift; they do
  not authorize execution or bind custom roots, complete host SDL, documents,
  result projections, disclosure policy, runtime pagination, or current
  resolver/RLS decisions.
- Added SQLite/PostgreSQL/MSSQL and default/PascalCase coverage for renamed
  plurals, composite keys, list/single/search/keyset operations, every mutation
  category, subscriptions, read-only and append-only profiles, mutation
  allowlists, hidden JSON fields, private read projections, and fingerprint
  drift.

Existing resolver names, GraphQL SDL, database schema, stored data, repository
APIs, authorization, and RLS behavior are unchanged. No database or data
migration is required.

## 0.15.0

Companion macros crate: `graphql-orm-macros` **0.15.0** under the aligned
Git-only version policy. This is a pre-1.0 minor because generated bounded
mutation behavior changes.

- Fixed generated single-key and repository-only composite-key bounded update
  and delete so their exact `MutationLimit + 1` look-ahead is no longer
  clamped by the public 100-row pagination maximum.
- Applied the same exact internal sentinel selection to host-only retention
  purge. The path orders by the complete primary key and is reachable only
  through generated bounded mutation/retention execution; ordinary GraphQL,
  connection, repository, and runtime reads retain their existing caps.
- Bounded mutations now reject residual/in-memory predicates before selection,
  hooks, events, notifications, or writes instead of risking an unbounded
  candidate scan. Database-renderable filters retain exact all-or-nothing
  outcomes at ceilings above 100.
- Added checked sentinel/count arithmetic and fail-closed selected-versus-
  affected cardinality checks. Any intervening cardinality change rolls back
  the complete transaction and releases no queued events.
- Preserved the optional one-way agql-auth 0.12.0 bridge at exact revision
  `3f3b0c5365adfbe436514a681d977b600991b797` and its single-type-universe
  requirement.

No database schema or stored-data migration is required.

## 0.14.0

Companion macros crate: `graphql-orm-macros` **0.14.0** under the aligned
Git-only version policy; macro syntax and generated output are unchanged.

- Aligned the optional one-way `auth-agql` bridge with released `agql-auth`
  0.12.0 at exact revision
  `3f3b0c5365adfbe436514a681d977b600991b797`. A matching direct host
  dependency resolves one package and public type universe.
- Preserved identity, role, scope, tenant, organization, actor, correlation,
  token/session reference, policy-version, and host-accepted assurance
  mappings. Standard scalar `acr` and separate assurance `context` remain
  byte-for-byte distinct and absent values are not synthesized.
- Hardened assurance projection to omit malformed values and values
  inconsistent with the session MFA state or access-token `auth_time`, AMR,
  and scalar ACR. `MfaAcceptance::Unsatisfied` remains an exact negative MFA
  decision rather than becoming authority.
- Restricted custom claim projection to the documented string
  `policy_version`; arbitrary `AccessTokenMetadata.additional` content no
  longer enters `AuthSubject.claims`, `DbAuthContext`, PostgreSQL settings, or
  their debug/serialized forms.
- Kept OIDC request/outcome handling, provider evidence, rate-limit
  persistence, token minting, MFA inference, and product policy outside the
  ORM. In particular, `EssentialAcrs`/`matched_acrs` alone creates no ORM
  assurance, and graphql-orm does not implement agql-auth 0.11's atomic
  `AuthRateLimitStore`.

This observable bridge hardening is a pre-1.0 minor release rather than a
dependency-only patch. No database schema, data, generated-code, or backend
migration is required.

## 0.13.0

This combined release contains two coordinated prompts. Companion macros
crate: `graphql-orm-macros` **0.13.0** under the aligned Git-only version
policy; derive syntax and generated code are unchanged.

- Added fingerprint-bound opaque parent anchors and batched runtime to-one/
  to-many relation reads with typed composite keys, nullable-key short circuit,
  bounded per-parent forward/backward `gormrr1` keysets, optional exact counts,
  stable errors, hidden grouping/cursor fields, and SQLite/PostgreSQL parity.
- Added `RuntimeRelationLimits`, `RuntimeRelationSelection`, anchored read and
  batch request/result types, plus `Database::execute_runtime_anchored_read`
  and `Database::execute_runtime_relation_batch`. MSSQL remains explicitly
  unsupported for runtime execution; static relation behavior is unchanged.
- Added `runtime_relation_batch_request_with_relation_keys` and
  `RuntimeRelationBatch::relation_parents` so an executed child layer retains
  only opaque, redacted keys for the next explicitly requested relation. A
  multi-level request remains one bounded compatible statement per layer.
- Fixed PostgreSQL introspection to group UNIQUE constraints by ordered catalog
  identity and exclude `pg_constraint.conindid`/primary backing indexes from
  ordinary indexes while preserving explicit unique and partial indexes.
- Managed CREATE TABLE now renders declared composite UNIQUE constraints, so
  the structured target and live PostgreSQL/SQLite schema agree after first
  apply. Unchanged replans and additive complete-target upgrades no longer try
  to drop constraint-owned indexes.
- Existing runtime/static cursors, public backend traits, GraphQL/generated
  APIs, serialized runtime schemas, and stored data are compatible.

## 0.12.0

Companion macros crate: `graphql-orm-macros` **0.12.0**. The Git-only aligned
release policy advances both crates for this public runtime API release; derive
syntax and generated code are unchanged.

- Added schema-fingerprint-bound `RuntimePredicate`, `RuntimeOrder`,
  `RuntimeReadRequest`, limits, page/cursor, connection, page-info, and safe
  error APIs for runtime-schema reads.
- Added validated recursive scalar filters, structural policy-filter `AND`,
  explicit portable null ordering, primary-key tie-breakers, bounded
  bidirectional keysets, hidden cursor columns, and opt-in exact count.
- Added `Database::execute_runtime_read` with typed bindings, exact existing
  runtime row decoding, and optional `DbAuthContext` on SQLite/PostgreSQL.
  MSSQL remains explicitly unsupported for runtime decoding/execution while
  static reads are unchanged.
- Existing static queries, generated CRUD/GraphQL, backend traits, `SqlValue`,
  legacy cursor formats, schemas, and migrations are source-compatible. No
  schema or data migration is required.

## 0.11.0

Companion macros crate: `graphql-orm-macros` **0.11.0**. Both Git-only crates
advance together because this release adds a public derive, generated code,
repository authorization callbacks, and runtime query types.

- Added opt-in `RepositoryEntity` / `#[repository_entity(...)]` generation for
  one canonical managed entity with typed repository CRUD, filters, ordering,
  projections, transactions, CAS/composite operations, hooks, events, search,
  backup, and authorization, but no async-graphql types or roots.
- Added bounded Database-bound `RepositoryQuery` reads and separate fail-closed
  repository field-policy callbacks. Search-enabled entities use a bounded,
  policy-aware `RepositorySearchQuery`. Private/sensitive fields remain
  available to trusted Rust write inputs without widening GraphQL inputs.
- Sensitive generated input/projection debug output, mutation-hook state, and
  change events are redacted; repository entity/row/field policies continue to
  apply without treating an absent GraphQL context as authority.
- Equivalent repository-only and GraphQL-enabled declarations retain identical
  managed schema models and stable hashes. No DDL or data migration is needed.
- SQLite/PostgreSQL provide the applicable full contract; MSSQL repository-only
  entities are read-only and reject write configurations at compile time.

## 0.10.0

Companion macros crate: `graphql-orm-macros` **0.10.0**. Repository release
policy keeps the Git-only companion versions aligned when public runtime APIs
change; derive syntax and generated code are unchanged.

- Added owned `RuntimeValue`, `RuntimeRecord`, finite-float, and canonical
  datetime types covering every existing `RuntimeValueKind`.
- Added fingerprint-bound collection, field, relation, and projection handles
  resolved only by `ValidatedRuntimeSchema`; unknown, cross-collection,
  duplicate, empty, and stale inputs fail before query execution.
- Added the source-compatible `RuntimeRowDecoder` capability and exact,
  projection-only SQLite/PostgreSQL decoding with stable safe errors and
  retained backend sources. MSSQL/no-default configurations remain explicit
  unsupported capabilities while existing static reads continue unchanged.
- Added real SQLite and owned disposable-PostgreSQL parity, hostile-row,
  nullability, type-mismatch, serialization, and feature-boundary coverage.
- Runtime query rendering/execution, dynamic GraphQL, filters, ordering,
  pagination, relation batching, and writes remain deliberately deferred.

## 0.9.0

Companion macros crate: `graphql-orm-macros` **0.9.0**. Both crates require a
pre-1.0 minor version because public schema descriptors and generated code
change.

- Added opt-in `retention_purge = "policy.key"` metadata for append-only managed
  SQLite/PostgreSQL entities.
- Added host-only `Database::retention_transaction[_with_auth]`, narrow
  `RetentionContext`, generated bounded typed purge, exact outcomes, and
  redacted post-commit notifications.
- Added transaction-local SQLite/PostgreSQL append-only enforcement exceptions,
  structural introspection, stable schema/module/backup fingerprints, explicit
  migration work, policy/RLS integration, and fail-closed tamper detection.
- Existing append-only entities remain non-purgeable and retain their previous
  stable fingerprints. Ordinary repository, transaction, and GraphQL mutation
  surfaces are unchanged.
- Manual public metadata/model struct literals and exhaustive enum matches
  require the 0.9.0 source updates listed in MIGRATION.md. Low-level backend
  traits retain fail-closed default methods, and older serialized
  descriptors/catalogs default retention to disabled.

## 0.8.0

Companion macros crate: `graphql-orm-macros` **0.8.0**.

- Added an owned, backend-neutral runtime schema IR (`runtime_schema` module):
  stable ID newtypes, owned collection/field/relation/index metadata with
  ordered relation key pairs and composite primary keys, fail-closed structured
  validation diagnostics, deterministic canonical serialization, and separate
  full and ID-free structural fingerprints.
- Added `RuntimeSchema::from_static_entities` so derive-generated
  `EntityMetadata` graphs convert into the owned IR; equivalent static and
  runtime definitions agree on the ID-free structural fingerprint.
- `ColumnDef` and `FieldMetadata` gained `api_name`, `is_sortable`, and
  `is_date_time` fields (with const builders), emitted by the derives so
  public GraphQL names, sortability, and date-time semantics are recorded in
  metadata. Existing backup hashing, schema planning, and generated GraphQL
  behavior are unchanged.
- Hand-written `ColumnDef`/`FieldMetadata` struct literals must add the new
  fields or use the const builders; see MIGRATION.md.
- Fixed `Option<Vec<u8>>` logical type inference: nullable byte columns now
  carry `BackupValueKind::Bytes` instead of falling through to `Json`. Storage
  DDL was already BYTEA/BLOB; logical backup descriptors and stable schema
  hashes change for affected entities (see MIGRATION.md).
- The IR fails closed: Serde deserialization enforces stable-ID validity and
  rejects unknown properties; validation proves foreign-key target uniqueness,
  default/value-kind compatibility, global stable-ID uniqueness, and duplicate
  key members; canonical rendering escapes literal defaults. The ID-free
  fingerprint is named `structural_fingerprint` and conversion reports policy,
  backup, redaction, ownership, and propagation semantics as unsupported
  rather than dropping them.
## 0.7.1

Companion macros crate: `graphql-orm-macros` remains **0.7.0**.

- Fixed backend dependency isolation so a SQLite-only build activates
  `sqlx-sqlite` but not `sqlx-postgres`, a PostgreSQL-only build activates
  `sqlx-postgres` but not `sqlx-sqlite`, and an MSSQL-only build activates
  neither SQLx database driver.
- SQLite now uses SQLx's Tokio runtime without an unused SQLx TLS stack;
  PostgreSQL retains Tokio plus Rustls. Combined SQLite/PostgreSQL builds still
  activate both drivers.
- No public API, generated code, schema, migration, authorization, repository,
  transaction, backup, GraphQL, or naming behavior changed. No data migration
  is required.

## 0.7.0

Companion macros crate: `graphql-orm-macros` **0.7.0**.

- Added dependency-owned `OrmSchemaModule` composition with stable module ID,
  semantic version, reserved table namespace, schema fingerprint, migration
  target, backup descriptors, and declared restore phases.
- Added module-aware schema/backup snapshots and fail-closed validation for
  duplicate ownership, overlapping namespaces, invalid or duplicate restore
  hooks, and source-controlled fingerprint drift.
- Added backend-neutral fenced lease state, proof bindings, monotonic fencing,
  CAS row versions, heartbeats, fenced child writes, release, and reclaim
  contracts. Failed transitions leave the in-memory state unchanged.
- Added validated `first`/`after` and `last`/`before` keyset windows, portable
  before-cursor SQL predicates, and generated SQLite/PostgreSQL repository and
  transaction helpers that restore backward reads to canonical order.
- Aligned the optional `auth-agql` bridge with `agql-auth` 0.10.0 at exact
  revision `c92dcb441237bbe308499b26525945f60ffa394a` while preserving the existing
  principal/session-assurance mapping.
- Existing GraphQL fields, CRUD behavior, offset pagination, authorization,
  and database schemas are unchanged. The new APIs are opt-in and create no
  automatic data migration.

## 0.6.3

Companion macros crate: `graphql-orm-macros` **0.6.1**.

- `schema_roots!` retains the public Rust root names while exporting their GraphQL object names as
  the conventional `Query`, `Mutation`, and `Subscription`, making async-graphql federation SDL
  unambiguously composable without downstream rewriting.
- Schemas with no subscription contributors now use `EmptySubscription`; they do not emit a fake
  empty object or a dangling operation root.
- Added parsed federation-SDL coverage for complete, zero-subscription, read-only MSSQL, and
  multi-chunk query schemas, including PascalCase resolver naming.
- No repository, authorization, transaction, backup, migration, or database behavior changed.

## 0.6.2

Companion macros crate: `graphql-orm-macros` remains **0.6.0**.

- Aligned the optional Git-only `auth-agql` bridge with `agql-auth` 0.8.1 at exact revision
  `f1fb5fe8c42d29806821d5f1a9032b007dee63e4`, so hosts using the bridge and a direct dependency
  resolve one `agql-auth` type universe.
- No bridge API, authorization behavior, persistence behavior, or generated code changed.

## 0.6.1

Companion macros crate: `graphql-orm-macros` remains **0.6.0**.

- Fixed PostgreSQL logical-backup restores so null values bind with the column's declared type,
  including JSONB, UUID, byte, numeric, and boolean columns, instead of falling back to text.
- Added dependency-aware ordering for rows with self-referential foreign keys so parent rows are
  inserted before their children during empty-database restores.
- Self-reference cycles and references to rows missing from the backup now fail with explicit
  protocol errors before the table transaction commits.
- Added PostgreSQL nullable-JSON round-trip coverage and focused child-before-parent restore-order
  coverage.

## 0.6.0

Companion macros crate: `graphql-orm-macros` **0.6.0**.

- Updated the optional Git-only `auth-agql` bridge to `agql-auth` 0.8.0 at exact revision
  `be4e0a213ce9c9b9fbe9fe985602743a584e019b` and preserved authoritative session assurance,
  organization, correlation, actor, active-scope, and policy metadata.
- Added opt-in repository-only composite-key mutations with generated ordered key/create/update
  types, complete-key CRUD, insert-if-absent, private upsert, and transaction-bound equivalents.
- Added atomic complete-key plus typed-predicate updates with distinct not-found, predicate-conflict,
  and updated outcomes.
- Added explicit `MutationLimit` and no-partial-write bounded update/delete outcomes for single and
  composite key entities.
- New composite mutation SQL dialect-quotes identifiers, binds values, validates exact affected-row
  counts, and preserves policies, transforms, hooks, search, events, rollback, and PostgreSQL RLS.
- Opted-in composite writes require an explicit `EntityPolicy` provider even in legacy mode; the
  new mutation surface is never default-allow.

## 0.5.0

Companion macros crate: `graphql-orm-macros` **0.5.0**.

- Added private entity-level identifier-based `projection(...)` declarations that generate exact typed DTOs and
  select only their declared columns on SQLite and PostgreSQL.
- Added bounded typed repository queries, primary/unique lookup helpers, auth-aware reads, and
  transaction-bound `MutationContext::project` queries with own-write visibility.
- Projection reads preserve entity authorization and PostgreSQL RLS. Application row policies and
  residual in-memory filters fail closed because evaluating them would require a full entity.
- Added `sensitive` field metadata and redacting projection `Debug` implementations. Projections are
  never exposed through GraphQL.

## 0.4.3

Companion macros crate: `graphql-orm-macros` **0.4.3**.

- Conditional-index introspection now accepts only the complete portable closed-set grammar;
  leading/trailing boolean expressions, comments, functions, casts outside PostgreSQL's generated
  text literals, and other tokens are drift.
- SQLite append-only introspection validates both complete generated trigger definitions rather
  than trusting managed names.
- PostgreSQL append-only introspection validates the exact trigger event/timing/enablement,
  unconditional function body, ownership, language, security-definer, search-path, and privilege
  posture.

## 0.4.2

Companion macros crate: `graphql-orm-macros` **0.4.2**.

- Migration-history preparation now transactionally adopts the recognized legacy
  `(version, applied_at)` table on SQLite and PostgreSQL.
- Legacy rows retain their version and timestamp and receive the deterministic description
  `Legacy migration <version>`; current optional metadata remains unknown (`NULL`).
- Existing tables with ambiguous columns, types, nullability, or primary-key identity fail closed.

## 0.4.1

Companion macros crate: `graphql-orm-macros` **0.4.1**.

- Added raw `Vec<u8>` primary-key support across repository/transaction CRUD, CAS, exact filters,
  hooks, row policies, and keyset cursors on SQLite `BLOB` and PostgreSQL `BYTEA`.
- Repository and `MutationContext` upserts may now target host-supplied private keys. When the
  conflict target is absent from the public create input, the GraphQL upsert field is omitted.
- Added structural `conditional_index(...)` metadata for portable closed-set partial indexes,
  including stable hashes, quoted DDL, SQLite/PostgreSQL introspection, and drift recreation.
- Added `gt_field`, `lte_field`, and `lt_field` portable comparisons alongside `gte_field`.
- PostgreSQL managed-schema comparison now canonicalizes harmless SQL type-name case differences.

## 0.4.0

Companion macros crate: `graphql-orm-macros` **0.4.0**.

### Added

- SQLx-free `Database::transaction` / `transaction_with_auth`, transaction-bound reads and writes,
  state-machine isolation, safe retry classification, nested-call rejection, and cancellation-safe
  rollback.
- Opt-in `#[graphql_orm(version)]` atomic compare-and-swap with typed expected filters,
  database-side monotonic increments, and explicit not-found/conflict/updated outcomes.
- Opt-in `append_only = true` generated surfaces and managed SQLite/PostgreSQL trigger enforcement
  with stable metadata, introspection, and drift planning.
- Portable numeric, length, closed-set, and cross-field constraints generated as named managed
  checks and mapped to safe constraint errors.
- Opt-in composite keyset pagination for repository, transaction, and GraphQL paths with bounded
  look-ahead queries and strict versioned opaque cursors.

### Compatibility

- Both crates are GitHub-only and set `publish = false`. Consumers must pin the reviewed full
  `graphql-orm` commit SHA; the optional bridge retains its exact full-SHA `agql-auth` dependency.
- Existing offset connections and mutable entity APIs remain unchanged unless the new attributes
  are selected. Append-only entities intentionally omit mutation APIs.
- `WriteBackend` was not extended; the public transaction runner uses the additive
  `TransactionBackend` capability.
- Stored numeric offset cursors are not accepted by keyset fields. Clients must begin keyset
  traversal without a cursor after switching fields.

See [MIGRATION.md](MIGRATION.md) and
[portable persistence primitives](docs/architecture/portable-persistence.md).

## 0.3.0

Companion macros crate: `graphql-orm-macros` **0.3.23** (epoch-default
generation and runtime expression alignment; patch release for compatibility).

### Security

- Added `AuthorizationMode` with fail-closed `DeclaredPoliciesRequired` and
  `ExplicitPolicyForAllExposedOperations` modes. Default remains
  `LegacyPermissive` for one migration release; production should opt into
  `DeclaredPoliciesRequired`.
- Public GraphQL errors now use stable codes via `OrmPublicError` /
  `OrmErrorCode`. SQL and configuration strings are not exposed by default
  (**breaking** for callers that parsed raw infrastructure messages).
- `AuthSubject` and `DbAuthContext` redact sensitive claim bodies in `Debug`.
- DataLoader / auth cache keys fingerprint claims instead of embedding raw JSON.
- Event sender locks recover from poisoning instead of panicking.
- Pagination defaults reduced from 1000/1000 to 50/100 (**breaking**). Use
  `PaginationConfig::legacy()` during migration.
- Added structural tenant/owner authorization helpers for backend-independent
  predicates.
- Added optional `auth-agql` bridge mapping `agql_auth::AuthPrincipal` →
  `AuthSubject` / `DbAuthContext`, pinned to upstream
  `agql-auth` 0.7.0 (`rev = 5e7f230b96350f55496477c11f8a0505e6438779`) with no
  path/`[patch]` overrides.

### Fixed

- **SQLite migration idempotency:** column defaults such as `unixepoch()` and
  `(unixepoch())` are now treated as equivalent during planning, hashing, and
  live-schema introspection. Reopening a file-backed SQLite database and
  replanning the same managed schema no longer emits a false `AlterColumn`
  step that breaks `ApplyOptions::additive_only` restarts. Canonicalization is
  general for balanced outer parentheses and SQL keyword/boolean defaults; it
  does not weaken additive-only validation for real changes.
- **Empty migration re-apply:** `SchemaManager::apply_migration` (and
  `apply_schema_target`) treat an already-recorded version as a no-op **only
  when the plan has no remaining steps or statements**. Restart paths that
  replan an empty list for the same version no longer insert a second history
  row. If the version is already recorded but the plan still has work, apply
  fails closed (schema drift / unsafe version reuse) instead of silently
  reporting success.
- **Schema-target remaining work:** `apply_schema_target` evaluates remaining
  work from the full plan (nested migration steps/statements, RLS statements,
  and combined executable statements). An empty nested `plan.migration` with
  remaining RLS/combined statements is no longer treated as already applied.
- **SQLite UNIQUE introspection:** inline `UNIQUE` column constraints (and
  multi-column `UNIQUE (...)` constraints) are recovered from
  `sqlite_autoindex_*` entries with origin `u`. Generated `#[unique]` fields
  no longer cause false `AlterColumn` plans after reopening a file-backed
  database.

### Added

- `AccessContext` / `SystemAccess` for deliberate repository system authority.
- `FilterExpression::TrustedFragment` and `trusted_fragment` constructor.
- `canonicalize_column_default_expression` for shared default comparison.
- Documentation: strict authorization, error codes, agql-auth bridge,
  cross-backend tenant isolation, pagination migration.

### Migration

See [MIGRATION.md](MIGRATION.md).

## 0.2.21

- Added `AuthSubject`, upgraded `AuthExt`, exact-scope `ScopeEntityPolicy`, and `DbAuthContext`
  constructors.
- Added generated resolver auth modes on entities and schema roots.
- Kept `auth_user()` as a deprecated alias for source compatibility.
- Added a reserved optional `auth-agql` feature; concrete agql-auth converters are deferred until
  the upstream agql-auth 0.7 API is tagged.

See the 0.2.21 section in the [historical release-notes ledger](docs/archive/2026/graphql-orm-release-notes.md#0221) and the migration
guide in [MIGRATION.md](MIGRATION.md).
