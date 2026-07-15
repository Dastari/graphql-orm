# Migration Guide

`graphql-orm` is distributed from GitHub only. Use a reviewed full 40-character commit in `rev`;
neither the runtime nor macros crate is published to crates.io.

## 0.10.0 Runtime Record Read Foundation

Update both Git-only crates to 0.10.0 at the reviewed full Git revision.
Repository release policy keeps companion versions aligned for a public
runtime API release; declaration syntax and generated code are unchanged.

This is an additive pre-1.0 runtime API release. Existing derived entities,
repositories, GraphQL SDL, database schemas, migration history, backups,
authorization, and static backend behavior require no source or data
migration. Existing `OrmBackend` implementations remain source-compatible;
the new `RuntimeRowDecoder` capability is separate and defaults to a
fail-closed unsupported result.

Runtime-schema hosts may replace host-owned value/row-decoding models with
`RuntimeValue`, `RuntimeRecord`, and handles resolved from their existing
`ValidatedRuntimeSchema`. Resolve fresh handles after catalog/schema
activation: owned handles and records are bound to the creating schema
fingerprint, and mixing generations returns `schema_mismatch`. SQLite UUID,
JSON, and datetime fields must use the documented text representation;
PostgreSQL uses native UUID/JSON(B)/TIMESTAMPTZ types. Datetimes canonicalize to
UTC at PostgreSQL-compatible rounded microsecond precision.

No database migration is required. MSSQL runtime-row decoding is deliberately
unsupported in this slice; its static generated reads are unchanged. See
[Runtime values, records, handles, and row decoding](docs/runtime-records.md).

## 0.9.0 Bounded Append-Only Retention Purge

This release adds public runtime and generated surfaces and extends public
schema/backup descriptors, so it is a pre-1.0 minor release rather than a
patch. Update both crates to 0.9.0 at the final reviewed release revision.

Derive-generated entities need no source change unless they opt in. Code that
constructs public descriptors manually must add disabled retention state:

- `RuntimeCollection { retention_purge: false, .. }`;
- `TableModel { retention_purge: false, .. }`;
- `EntityBackupDescriptor { retention_purge: false, .. }` (older serialized
  descriptors default this field to `false`);
- `EntityMetadata` literals add `retention_policy: None`. Existing
  `EntityMetadata::from_schema` calls remain source-compatible; only code that
  deliberately supplies a retention key uses `from_schema_with_retention`; and
- exhaustive matches on `EntityAccessSurface`, `RuntimeSchemaDiagnosticCode`,
  or `MigrationStep::SetAppendOnly` must handle the new retention variant/field.

Low-level backend capability traits gained safe default methods that reject
retention maintenance, so out-of-tree implementations remain source-compatible
unless they deliberately opt into this contract. SQLite and PostgreSQL provide
the supported implementations. Existing format-v1 owned runtime-schema JSON
without `retention_purge` continues to deserialize with the capability
disabled.

Existing append-only entities and fingerprints remain unchanged. To opt in,
add a dedicated policy key such as
`retention_purge = "audit.retention.purge"`, register an `EntityPolicy` that
allows only `EntityAccessSurface::RetentionMaintenance`, and replace raw purge
SQL with `Database::retention_transaction[_with_auth]` plus
`RetentionContext::purge`. Keep ordinary write policy keys separate.

Enabling or disabling retention changes managed enforcement and requires a new
host/module migration version. SQLite adds the reserved, structurally validated
`__graphql_orm_retention_context` table and replaces the DELETE trigger.
PostgreSQL replaces the append-only function contract and, when managed RLS is
enabled, adds the transaction-local retention DELETE policy. There is no row
data rewrite. Validate foreign-key behavior and the intended cutoff before
enabling physical deletion. See
[Bounded append-only retention maintenance](docs/retention-maintenance.md).

## 0.8.0 Owned Runtime Schema IR

Update both `graphql-orm` and `graphql-orm-macros` to 0.8.0 at the same reviewed
Git revision. This is a **breaking** pre-1.0 release, not an additive one, in
two respects:

**1. Public metadata struct fields.** `ColumnDef` and `FieldMetadata` gained
`api_name: &'static str`, `is_sortable: bool`, and `is_date_time: bool`. Code
built through the derives or the `ColumnDef` const builders needs no changes.
Code constructing either struct as a literal must add the new fields
(`api_name` defaults to the column name through `ColumnDef::new`; both flags
default to `false`), or switch to the builders: `.api_name(...)`,
`.sortable()`, `.date_time()`.

**2. Nullable-byte logical identity.** `Option<Vec<u8>>` fields previously
reported logical type `Json` while storing BYTEA/BLOB; they now correctly
report `Bytes`. Column DDL, stored data, row decoding, and generated GraphQL
are unchanged. For entities **with** nullable byte columns:

- `stable_schema_hash` and schema-module fingerprints that include such an
  entity change. Bump the semantic version of any `OrmSchemaModule` whose
  fingerprint covers one, and regenerate recorded fingerprints.
- Backups taken before 0.8.0 that contain such an entity will fail 0.8.0
  hash-compatibility verification. Take fresh backups after upgrading. Keep
  the old archives: they remain readable by pre-0.8.0 binaries, and their
  bytes are not corrupted — only the recorded schema identity differs. Do not
  overwrite or prune them until a post-upgrade backup has been verified.
- Logical backups written by 0.8.0 record byte columns as `Bytes` (their
  values were already binary).

Entities without nullable byte columns keep their existing hashes,
fingerprints, and backup compatibility; no database or data migration is
required for anyone.

The new `runtime_schema` module is additive API surface. Runtime query
execution, migration planning from the IR, and dynamic GraphQL registration
are not part of this release.

## 0.7.1 Backend Dependency Isolation

Update `graphql-orm` to 0.7.1 at the reviewed full Git revision. The companion
`graphql-orm-macros` crate remains 0.7.0. Existing feature declarations do not
change:

```toml
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.7.1", default-features = false, features = ["sqlite"] }
```

Cargo now resolves only the selected backend driver. There is no Rust API,
generated-code, GraphQL SDL, schema, configuration, or database migration.
Remove any downstream workaround that patched SQLx features, refresh the lock
file, and confirm the selected graph with the commands in
[Development](docs/development.md#backend-dependency-isolation).

## 0.7.0 Schema Modules, Fenced Leases, and Bidirectional Keysets

Update both `graphql-orm` and `graphql-orm-macros` to 0.7.0 at the same reviewed
Git revision. If `auth-agql` is enabled or the host directly uses `agql-auth`,
align it to version 0.10.0 at revision
`c92dcb441237bbe308499b26525945f60ffa394a` so Cargo resolves one public type
universe.

Existing entities, generated GraphQL SDL, mutations, offset connections, and
stored cursors remain valid. No automatic database or data migration is
required.

The ORM bridge API and mapped principal/session-assurance data are unchanged.
Hosts that directly use `agql-auth` OIDC state and opt into 0.10 bound
reauthentication must follow its 0.10 migration guide: persisted
`OAuthLoginState` records gain an optional authorization-policy value, and a
decomposed relational store needs a nullable column before enabling the new
writer. Hosts that only consume the ORM principal bridge need no data
migration.

Dependency crates that own private tables may implement `OrmSchemaModule` and
compose a `SchemaModuleCatalog`. Applying the resulting schema target remains a
host-controlled migration operation and must use a fresh host migration/module
version whenever an owned entity, index, constraint, or persistent semantic
changes. Backup and restore code should persist `SchemaModulesSnapshot` and run
the declared restore phases through the owning dependency.

`FencedLeaseState` is a backend-neutral transition contract, not a replacement
for an atomic database predicate. Claims, heartbeats, child writes, and release
must compare resource, owner, attempt, fencing token, unexpired deadline, and
CAS row version in the same persistence operation.

Entities with configured keyset ordering gain the additive repository method
`keyset_connection_page`. Use `first` with optional `after` for forward reads,
or `last` with optional `before` for backward/tail reads. Limits remain bounded
by the database pagination configuration. Existing generated GraphQL keyset and
offset fields are unchanged.

## 0.6.3 Federation Operation Roots

Update the runtime to 0.6.3 and the companion macros crate to 0.6.1 at the same reviewed Git
revision. No Rust call-site or database migration is required: `QueryRoot`, `MutationRoot`, and
`SubscriptionRoot` remain the generated Rust names.

The generated GraphQL object names change to `Query`, `Mutation`, and `Subscription`. This makes
federation SDL valid when the exporter relies on GraphQL's conventional implicit operation roots.
If schema tooling explicitly matched the old GraphQL type names, update those matches. Empty
mutation and subscription roots remain absent rather than becoming fieldless objects.

Regenerate provider SDL, parse or validate it, and run the federation composition check described
in [Federation operation roots](docs/federation.md) before promotion.

## 0.6.2 agql-auth Bridge Alignment

Update `graphql-orm` to the reviewed `v0.6.2` commit and align any direct `agql-auth` dependency to
version 0.8.1 at exact revision `f1fb5fe8c42d29806821d5f1a9032b007dee63e4`. This ensures Cargo
resolves one `agql-auth` package and one set of public types. No bridge API, authorization behavior,
database migration, or generated-code change is required. The companion macros crate remains
0.6.0.

## 0.6.0 Auth Assurance and Typed Composite Mutations

Pin both crates to the reviewed `v0.6.0` commit. This release keeps existing single-key and 0.5
projection APIs, but adding assurance/organization/correlation fields to the public `AuthSubject`
and `DbAuthContext` structs is a source change for direct struct literals. Prefer
`AuthSubject::builder` and `DbAuthContext { ..Default::default() }`.

The optional `auth-agql` bridge now requires Git-only `agql-auth` 0.8.0 revision
`be4e0a213ce9c9b9fbe9fe985602743a584e019b`. It retains session assurance and safe policy/audit
metadata. Remove any direct 0.7 pin so Cargo resolves exactly one `agql-auth` version.

Natural composite-key writes are opt-in:

```rust
#[graphql_entity(
    repository_mutations = true,
    upsert = "tenant_id,natural_id",
    unique_composite = "tenant_id,natural_id"
)]
```

Mark every key field `#[primary_key]` and host-assigned with
`#[graphql_orm(auto_generated = false)]`. Use the generated `EntityKey`, `CreateEntityInput`, and
`UpdateEntityInput` with `find_by_key`, `insert`, `insert_if_absent`, `upsert`, `update_by_key`,
`delete_by_key`, `update_if`, and bounded typed filter mutations. These APIs add no GraphQL mutation
fields. See [Typed Composite-Key and Bounded Mutations](docs/composite-mutations.md).

`MutationLimit::new` is required by bounded operations; an overflow returns `LimitExceeded` without
changing rows. Legacy `update_where`/`delete_where` remain available for source compatibility and
should be migrated when the caller needs a reviewable hard ceiling.

## 0.5.0 Typed Read Projections

This additive release is source-compatible with 0.4.3. Pin both crates to the reviewed `v0.5.0`
commit. Projection declarations change generated Rust APIs only and require no database migration.

Add `#[graphql_orm(projection(name = "...", fields = [id, field_name], private = true))]` to a
`GraphQLEntity`, then replace least-privilege full-entity reads with the generated projection's
repository or transaction methods. Mark secrets `#[graphql_orm(private, sensitive)]`; existing
`#[backup(redact)]` also drives projection `Debug` redaction.

If the database registers an application `RowPolicy`, projection reads now return a fail-closed
error rather than fetching a full entity to evaluate it. Move projection-compatible tenant or
soft-delete enforcement to generated typed filters or PostgreSQL RLS before migrating that caller.
No GraphQL query or DTO is added. See [Typed Read Projections](docs/read-projections.md).

## 0.4.3 Structural Introspection Hardening

This compatible patch needs no application API change. Pin both crates to the reviewed `v0.4.3`
commit and run managed validation with a new migration version.

Conditional indexes created by graphql-orm remain restart-idempotent. A same-name live index is now
accepted only when the entire stored predicate parses as the supported `field IN (closed set)` form
or PostgreSQL's equivalent `field = ANY (ARRAY[...])` representation. Extra boolean expressions,
comments that SQLite persists, functions, and unsupported casts are drift. PostgreSQL discards SQL
comments when storing index expressions, so comment-only spelling has no persistent structural
meaning on that backend.

Append-only validation now checks complete SQLite trigger definitions and PostgreSQL trigger and
function catalog contracts, including unconditional enforcement and privilege/search-path posture.
If an older deployment contains a recognizable managed name with hand-edited SQL, planning will
produce repair work. Reusing an already recorded migration version then fails closed; review and
apply that work under a fresh version with the schema-owner migration role.

## 0.4.2 Legacy Migration-History Adoption

This compatible patch needs no application API change. Move both crates to the reviewed `v0.4.2`
commit before adopting a database created by an older migration helper.

At managed-schema preparation, a history table containing exactly `version` as a non-null textual
sole primary key and a non-null textual/timestamp `applied_at` is upgraded in one transaction.
Every existing version and timestamp is preserved. Missing descriptions are set to
`Legacy migration <version>`, and missing current metadata columns are added as nullable text. No
historical migration is re-executed. Repeated preparation is idempotent.

SQLite rebuilds the recognized legacy table to install the complete current schema while preserving
rows verbatim. PostgreSQL requires `applied_at TIMESTAMPTZ NOT NULL`; arbitrary legacy text is not
converted because doing so could change timestamp meaning. PostgreSQL restores the
`CURRENT_TIMESTAMP` default for future rows without changing existing values. Unknown columns,
incorrect types or nullability, an empty version, or any other primary-key identity are rejected.
Recorded-version reuse and remaining-plan drift checks still run after adoption and still fail
closed.

Back up the database before first preparation. If a legacy table is rejected, inspect and migrate it
explicitly rather than renaming columns until it happens to pass validation.

## 0.4.1 Binary Keys and Conditional Indexes

This is a compatible Git-pin update. Move both crates to the reviewed `v0.4.1` commit.

- Binary `Vec<u8>` keys require no host encoding. Mark host-assigned keys
  `#[graphql_orm(auto_generated = false)]`; use `private`, `skip_input`, or `#[graphql(skip)]` when
  they must not appear in public GraphQL inputs. Add `min_length` and `max_length` before migration
  when fixed digest width is an invariant.
- Existing repository upsert entities with hidden conflict targets now compile. Their repository and
  transaction helpers remain available, while the unsafe GraphQL upsert field is absent.
- Before adding a unique conditional index, validate that rows inside the selected predicate set
  contain no duplicate indexed keys. Apply the generated create-index plan under a new migration
  version.
- Adding `gt_field`, `gte_field`, `lte_field`, or `lt_field` creates managed checks. SQL check
  predicates evaluate to UNKNOWN when either nullable operand is NULL, so NULL rows pass unless a
  separate non-null constraint applies.

## 0.4.0 Portable Persistence

This is an additive migration for existing entities. Upgrade both crates together to `0.4.0`.

1. Replace host-owned pool transactions with `Database::transaction`; use `StateMachine` for
   security-sensitive read/decide/write flows and retry the whole callback when classified
   retryable.
2. Add `#[graphql_orm(version, default = "0")]` to an `i64` field, apply the planned column
   migration, then move guarded updates to `compare_and_swap`.
3. Add `append_only = true` only after removing update/delete/upsert callers. Review and apply the
   trigger plan with the schema-owner migration role; remove UPDATE/DELETE grants from ordinary
   PostgreSQL roles as defense in depth.
4. Add portable constraint attributes, validate existing data, and apply the planned SQLite table
   rebuild or PostgreSQL named checks. New checks may reject historical invalid rows during rebuild.
5. Add a deterministic `keyset = "..., id asc"` order and migrate clients to the generated keyset
   field. Discard legacy numeric cursors; they intentionally fail strict keyset decoding.

Managed startup should validate after every step. Missing append-only triggers or named checks are
schema drift and must not be ignored or repaired with a reused migration version.

## 0.3.0 Security Hardening

### SemVer Recommendation

Release as **0.3.0** (minor with documented breaking security defaults for pagination
and public error messages). A future major can flip `AuthorizationMode` default to
`DeclaredPoliciesRequired`.

### Authorization Mode

```rust
// Before (implicit fail-open when no policy provider)
let database = Database::new(pool);

// After (recommended production setting)
let database = Database::new(pool)
    .with_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
database.set_entity_policy(MyEntityPolicy);
```

| Mode | Current default | Secure recommended | Future default |
| --- | --- | --- | --- |
| `LegacyPermissive` | yes | no | removed as default |
| `DeclaredPoliciesRequired` | no | yes | planned default |
| `ExplicitPolicyForAllExposedOperations` | no | for high-assurance APIs | optional |

### AuthSubject Expansion

```rust
// Before
AuthSubject::from_parts(id, roles, scopes, tenant_id)

// After (compatible) — same helper still works
// Prefer builder for new fields:
AuthSubject::builder(id)
    .user_id(user_id)
    .roles(roles)
    .scopes(scopes)
    .tenant_id(tenant)
    .token_id(jti)
    .session_id(session)
    .actor_id(actor)
    .build()
```

`Debug` no longer prints claim JSON bodies.

### Public Errors

```rust
// Before
Err(async_graphql::Error::new(error.to_string()))

// After
Err(OrmPublicError::from_sqlx(&error).into_graphql_error())
```

Missing auth messages changed from `"missing auth"` to `"unauthenticated"` with
`extensions.code = "UNAUTHENTICATED"`.

### Pagination Defaults

```rust
// Restore 0.2.x limits
Database::new(pool).with_pagination_config(PaginationConfig::legacy())
```

Default limit: `1000` → `50`. Max limit: `1000` → `100`.

### agql-auth Bridge

```toml
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.6.0", features = ["sqlite", "auth-agql"] }
```

The optional feature depends on upstream `agql-auth` 0.8.0 via git revision
`be4e0a213ce9c9b9fbe9fe985602743a584e019b` (tag `v0.8.0`). It does not use a
local path, sibling checkout, or Cargo `[patch]`.

```rust
use graphql_orm::graphql::auth_agql::auth_bundle_from_principal;
let (subject, db_auth) = auth_bundle_from_principal(&principal);
```

### Structural Tenant Helpers

```rust
let resolution = resolve_structural_auth(
    StructuralAuthMetadata::new(Some("tenant_id"), None, StructuralAuthorization::Required),
    &StructuralAuthValues::from_subject(&subject),
);
```

Macro-generated wiring of structural predicates on every operation path remains a
follow-up; helpers are available for host and incremental macro integration.

### Trusted SQL Fragments

Prefer `FilterExpression::trusted_fragment(clause, values)` for host-authored
predicates. `FilterExpression::Raw` remains for generated compatibility and is
documented as a trusted surface.

### SQLite Default Expression Idempotency

No application migration is required. After upgrading, reopening a file-backed
SQLite database and replanning the same managed schema should produce an empty
plan even when live `PRAGMA table_info` returns `unixepoch()` while generated
metadata previously declared `(unixepoch())`.

`ApplyOptions::additive_only` behavior is unchanged for real non-additive steps
such as altering a default from `unixepoch()` to `date('now')`.

### Empty Migration History Idempotency

`SchemaManager::apply_migration` is idempotent **only** when:

1. the version is already present in `__graphql_orm_migrations`; and
2. the plan has no remaining steps or statements.

Restart code that re-plans and re-applies the same version when the schema is
already current receives
`AppliedMigrationReport { already_applied: true, statements_applied: 0, .. }`.

If the version is recorded but the plan still has work, apply fails with an
explicit protocol error. That is intentional: it surfaces schema drift or
unsafe reuse of a migration version rather than silently treating the plan as
done.

For `apply_schema_target`, “remaining work” includes nested migration
steps/statements, RLS statements, and the combined executable `plan.statements`.
An empty nested table migration with remaining RLS work is **not** a no-op.

Callers that pattern-match `AppliedMigrationReport` must accept the new
`already_applied` field.

## 0.2.21 Auth Bridge

### Structural Changes

`AuthExt::auth_user()` is deprecated but still available. Migrate call sites based on what they need:

```rust
// Before
let user_id = ctx.auth_user()?;

// After: id only
let user_id = ctx.auth_user_id()?;

// After: id, roles, scopes, tenant id
let subject = ctx.auth_subject()?;
```

Applications can keep injecting the legacy `String` user id while migrating. `graphql-orm` upgrades it
to `AuthSubject { id, roles: [], scopes: [], tenant_id: None, ... }`. New code should inject
`AuthSubject` directly.

If a downstream crate implemented `AuthExt` itself, add implementations for `auth_user_id`,
`auth_subject`, and `auth_subject_opt`. Most applications only use the built-in implementation for
`async_graphql::Context<'_>` and do not need to change anything beyond call-site names.

```rust
let request = request.data(AuthSubject {
    id: user.id.to_string(),
    user_id: None,
    roles: user.roles.clone(),
    scopes: user.scopes.clone(),
    tenant_id: user.tenant_id.clone(),
    claims: None,
    token_id: None,
    session_id: None,
    actor_id: None,
});
```

`DbAuthContext::from_subject(&subject)` can mirror the same principal into PostgreSQL RLS settings:

```rust
let request = request
    .data(subject.clone())
    .data(DbAuthContext::from_subject(&subject));
```

### Generated Resolver Auth Modes

Generated resolvers keep the previous fail-closed default: if no `auth` setting is present, they
require an auth subject before database access. This preserves the old generated `ctx.auth_user()?`
gate.

Use `auth = "none"` for public generated resolvers:

```rust
#[graphql_entity(table = "pages", plural = "Pages", auth = "none")]
pub struct Page {
    // fields...
}
```

Use `auth = "optional"` when a schema should read a subject if present but leave allow/deny decisions
to `EntityPolicy`, `RowPolicy`, or `FieldPolicy`:

```rust
schema_roots! {
    auth: "optional",
    query_custom_ops: [],
    entities: [Record],
}
```

Use `auth = "required"` explicitly for new private schemas or entities:

```rust
schema_roots! {
    auth: "required",
    query_custom_ops: [],
    entities: [Ticket, Session],
}
```

Entity-level `auth` overrides the schema-root mode.

### ScopeEntityPolicy

`ScopeEntityPolicy` is exact-match only:

```rust
let mut database = Database::new(pool);
database.set_entity_policy(ScopeEntityPolicy::new(
    &["tickets.read"],
    &["tickets.write"],
));
```

`require_auth: true` returns an unauthenticated GraphQL error when no subject exists. A subject that
lacks the required exact scope returns `Ok(false)` from the policy and is denied by the generated
access path.

### Behavioral Notes

- No JWT, OIDC, cookie, wildcard, or product-specific scope logic was added to `graphql-orm`.
- PostgreSQL RLS helper functions still use exact scope matching.
- The `auth-agql` feature is available against `agql-auth` 0.10.
