---
title: "Changelog"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# Changelog

This file is the authoritative user-facing release chronology. The former
[release-notes ledger](docs/archive/2026/graphql-orm-release-notes.md) is retained
for historical context.

## Unreleased

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
