---
title: Bounded runtime platform capability contract
kind: plan
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-09-30
review_by: 2026-12-31
supersedes: []
---

# Bounded runtime platform capability contract

## Outcome and review checkpoint

Provide reusable runtime migration, transactional single-record mutation, and
opt-in dynamic GraphQL mechanisms for applications with catalog-loaded schemas.
This is a **proposed contract**, not an implemented API. All Rust signatures,
examples, SDL, feature additions, and future versions below are proposals.
Review this checkpoint before implementing unsettled public interfaces.

The initiating [consumer contract](https://github.com/Dastari/digibase/blob/agent/vertical-slice-plan/docs/coordination/prompts/0013-graphql-orm-runnable-product-prerequisites.md)
requires separate reviewable PRs. The host owns catalog persistence, activation,
authorization policy language, administration, HTTP transport, services,
clients, and durable event delivery. No sibling repository changes are needed.

## Baseline evidence

The consumer revision is `a6f079ff29e7f16910eee2f98d9e0eedf329e109`, with
`graphql-orm` and `graphql-orm-macros` 0.33.1. This proposal uses remote main
`131693bdb4a74cf43732cb279419701fe273c1d0`, also ORM/macros 0.33.1. A Git diff
between those revisions shows **no changes in either ORM package**. Intervening
changes concern AI provider startup and its documentation; none closes A–D or
the repository aggregate regression. The original local checkout remains at
`e5023b5c29b097f1c07ebc66057ebb50731a6330` (ORM/macros 0.33.0).

Existing public contracts to reuse:

- `RuntimeSchema::validate`, `ValidatedRuntimeSchema`, stable IDs,
  fingerprint-bound collection/field/relation handles and projections.
- `RuntimeValue`, `RuntimeRecord`, exact SQLite/PostgreSQL decoding, typed
  predicates/orders, bounded keyset reads/counts, and opaque relation anchors.
- `Database::schema`, `SchemaModel`, `SchemaValidationReport`, `PlanOptions`,
  `PlannedMigration`, risk classification, introspection, and explicit
  `apply_migration` with `ApplyOptions`.
- `Database::transaction_with_auth`, `TransactionMode::StateMachine`, and
  `MutationContext::insert`, `update_by_key`, `delete_by_key`, and `update_if`
  for static repository entities. These already permit atomic host journal
  writes, but have no runtime-record mutation methods.

The aggregate regression is still visible in
`crates/graphql-orm-macros/src/entity.rs`: `aggregate_field_definition`
unconditionally emits an async-graphql Enum derive and GraphQL attributes.

Read existing mechanics in [runtime schemas](../../../reference/graphql-orm/runtime-schema-ir.md),
[records](../../../reference/graphql-orm/runtime-records.md),
[queries](../../../reference/graphql-orm/runtime-queries.md),
[relations](../../../reference/graphql-orm/runtime-relations.md),
[schema management](../../../reference/graphql-orm/schema-management.md),
[writes](../../../reference/graphql-orm/runtime-and-writes.md),
[repository entities](../../../reference/graphql-orm/repository-only-entities.md),
[authorization](../../../reference/graphql-orm/strict-authorization.md), and
[GraphQL workflow](../../../development/graphql-workflow.md).

## Non-goals and compatibility invariants

No second canonical schema IR, SQL engine, policy evaluator, automatic
migration application, catalog, publication/lease service, audit/event tables,
subscriptions, durable replay, bulk/nested mutations, or new upsert semantics.
No license changes, agql-auth modifications, or storage/backup/AI dependencies
added to core ORM. Feature-minimal executor restructuring from consumer prompt
0007 is a separate investigation; dynamic GraphQL opt-in does not make the
existing async-graphql dependency optional.

Preserve static APIs, SDL, resolver naming profiles, authorization modes,
and all existing cursor encodings. Runtime schema format remains v1.
Application policy remains outside structural schema fingerprints. Runtime
capability traits are additive; do not add required methods to `OrmBackend`,
`WriteBackend`, or third-party backend implementations.

New executable capabilities support SQLite and PostgreSQL. MSSQL and unknown
backends return `unsupported_backend` before opening connections or beginning
transactions. Static MSSQL DML and schema validation retain their current
contracts. Composition must not imply that static async-graphql roots can be
merged directly into a dynamic schema; hosts provide dynamic fields/types.

## Dependencies and PR order

| PR | Depends on | Change | Suggested aligned ORM/macros version |
| --- | --- | --- | --- |
| Contract | Current main | This review checkpoint only | 0.33.1, unchanged |
| Fix | Reviewed scope, independently of A–D | Plain repository aggregate enums | 0.33.2 |
| A | Contract review | Owned physical target conversion and scoped planning | 0.34.0 |
| B | Contract review; A recommended for installation examples | Transactional runtime mutation engine | 0.35.0 |
| C | Contract review; A for end-to-end fixtures | Read-only dynamic GraphQL | 0.36.0 |
| D | Committed, reviewed B and C | Dynamic mutation registration | 0.37.0 |

These are proposed release increments, not reserved published versions.
Rebase and choose the next appropriate version at each PR. ORM/macros remain
aligned per `scripts/check-package-release-policy.sh`. Fix branches separately
from functional PRs; A, B, and C have independent implementation contracts,
while D consumes B/C. Recommended execution is Fix → A → B → C → D, one PR at a
time. Each dependent branch starts from a committed predecessor, never dirty
work from another task. Do not tag or publish without owner authorization.

## A: runtime migration planning

### Owned canonical physical models

The existing physical `TableModel::indexes` and `MigrationStep::CreateIndex`
contain `IndexMetadata = IndexDef`, whose names, columns, directions, and
predicate members require `&'static` storage. Repeated runtime conversion must
not use `Box::leak`, including through live introspection. Existing generated
index helpers and introspection have leaks that this execution path must avoid.

Proposed compatibility-preserving solution: parameterize the implementation of
the **existing** physical models and migration containers over index storage.
Keep every existing public name as a concrete alias of its static instantiation;
do not make existing struct literals infer an unconstrained generic parameter.
Add owned `IndexModel` with `String`,
`Vec<String>`, owned directions/predicate members, and the same semantics.
For example `SchemaModel = PhysicalSchemaModel<IndexMetadata>`,
`OwnedSchemaModel = PhysicalSchemaModel<IndexModel>`, and corresponding aliases
for TableModel, MigrationStep, and the intervening diff/plan containers. They
are storage instantiations of one canonical physical IR, not parallel models.
`OwnedPlannedMigration` wraps the owned instantiation of that same canonical
plan with private ownership/integrity data and read-only plan/hash accessors.
Static `IndexDef` and its const constructors remain unchanged. Existing
planning entry points retain their current signatures. Generalize existing
`apply_migration` over an additive `MigrationPlanSource` implemented for the
legacy PlannedMigration and the scoped OwnedPlannedMigration, with the same
report type and existing apply machinery. A host applies runtime plans through
the existing method. Compile existing direct struct literals, explicit type
annotations, enum matches, custom backends, and function pointers to prove
compatibility before accepting this approach. Do not change static hashes.

Proposed public surface, re-exported through `graphql_orm::graphql::orm`:

```rust,ignore
impl ValidatedRuntimeSchema {
    pub fn physical_schema<B: RuntimeMigrationBackend>(
        &self, limits: RuntimeMigrationLimits,
    ) -> Result<OwnedSchemaModel, RuntimeMigrationDiagnostics>;
}
impl OwnedSchemaModel {
    pub fn with_static_entities(
        self, entities: &[&EntityMetadata],
    ) -> Result<Self, RuntimeMigrationDiagnostics>;
}
impl ManagedTableSet {
    pub fn new(tables: impl IntoIterator<Item = String>)
        -> Result<Self, RuntimeMigrationDiagnostics>;
}
impl<'db, B: RuntimeMigrationBackend> SchemaManager<'db, B> {
    pub async fn validate_owned_schema(
        &self, target: &OwnedSchemaModel, ownership: &ManagedTableSet,
    ) -> Result<SchemaValidationReport, RuntimeMigrationError>;
    pub async fn plan_owned_migration(
        &self, version: impl Into<String>, description: impl Into<String>,
        target: &OwnedSchemaModel, ownership: &ManagedTableSet, options: PlanOptions,
    ) -> Result<OwnedPlannedMigration, RuntimeMigrationError>;
}
// Existing apply_migration accepts both static and owned scoped plans.
impl<'db, B: MigrationBackend> SchemaManager<'db, B> {
    pub async fn apply_migration<P: MigrationPlanSource>(
        &self, plan: &P, options: ApplyOptions,
    ) -> crate::Result<AppliedMigrationReport>;
}
```

`RuntimeMigrationBackend` is an additive capability implemented for SQLite and
PostgreSQL, using their existing dialect/introspection/migration mechanisms.
Owned introspection returns owned indexes directly. Conversion is pure and
collects all diagnostics with stable collection/field/index/relation IDs.
Limits bound collections, fields, indexes, relations, keys, default bytes,
total target bytes, and rendered plan steps/statements/bytes.

`ManagedTableSet` validates trusted physical identifiers, is owned/private,
and is explicitly supplied by the host. The target tables must be a subset of
this set. Owned current/target comparison is scoped to the set; tables removed
from the target but retained in ownership produce guarded destructive drops.
Outside tables are never dropped, altered, or assigned RLS policy. Reject
cross-boundary constraints whose effects cannot be represented safely.
`PlanOptions::managed_tables_only()` alone cannot express this contract because
it forgets ownership of a removed target table. `strict()` is strict **within
the explicit set** on the new API; legacy semantics remain unchanged.

Reuse existing source/target/plan hashing and baseline validation. The source
hash remains the complete introspected baseline as in existing planning, so
unrelated live drift can conservatively invalidate apply without granting
ownership of that table. Carry immutable ownership metadata with the owned
plan and verify its scope at apply; bind it into reviewed plan integrity without
changing the existing static plan hash format. The host persists its trusted
ownership set separately; ORM does not discover ownership from IDs or history.

### Mapping and safety

Map boolean, i64, finite float, string, UUID, JSON, bytes, and UTC datetime to
the existing exact runtime storage rules. Preserve nullability, ordered keys,
single/composite uniqueness, named ascending runtime indexes, ordered FK pairs,
delete actions, append-only/retention physical enforcement, and default values.
Reuse the static generated-index rules so equivalent FK/filter indexes do not
cause churn. RuntimeIndex has no direction property: do not invent descending
runtime indexes or modify format v1 in this PR.

`CurrentTimestamp` on Integer means epoch seconds, as specified by current IR
validation; reuse `Dialect::current_epoch_expr`. DateTime defaults must decode
as canonical UTC runtime datetimes on both backends; native SQLite
`CURRENT_TIMESTAMP` text needs an exact normalization/rendering solution, not a
decoder guess. Literal strings are quoted by the existing dialect; typed values
are parsed before rendering. Generated UUID fields are application generated,
as in existing derived entities, rather than inferred database identity columns.
Default-backed generated fields retain their declared defaults. Other generated
shapes with no representable generator yield `unsupported_generation`.

Do not translate public names or stable IDs into physical rename steps. Use
physical identifiers for canonical physical model identity; public renames may
change runtime fingerprints/cursors, but cannot change DDL or delete data.
The existing physical hash includes `entity_name` and `default_sort`; retain
its algorithm and physical default-order terms. Runtime target diagnostic
entity names come from api_type_name, so supported static/runtime pairs with
matching labels must have equal target hashes, plans and risks. A public type
rename can change this existing metadata-sensitive target hash while producing
zero physical steps; do not turn that hash difference into a rename or drop.
Stable runtime IDs never enter physical hashes. Acceptance requires equal
physical results and empty replans for supported equivalent targets, including
switching target origin after apply. Do not assert hash equality across
different diagnostic entity labels or non-equivalent storage representations.
Static narrow integers, arbitrary SQL types/defaults, spatial/search/check
semantics absent from runtime IR are not automatically equivalent.

Diagnostics include `unsupported_backend`, `unsupported_generation`,
`unsupported_default`, `invalid_physical_contract`, `target_collision`,
`ownership_mismatch`, `unmanaged_dependency`, and `limit_exceeded`. Validation
reports and plans retain existing risk classes. No DDL/history writes during
planning. Apply alone honors schema policy, clean baseline, expected source
hash, additive-only, destructive authorization, transactional execution, and
history. Runtime conversion does not create/clear host RLS declarations;
system entities remain on the host's existing SchemaTarget/RLS path where used.

Minimal host flow (proposed API; `Journal` is a host repository entity):

```rust,ignore
let target = schema.physical_schema::<SqliteBackend>(RuntimeMigrationLimits::default())?
    .with_static_entities(&[Journal::metadata()])?;
let ownership = ManagedTableSet::new(vec!["customers".into(), "host_journal".into()])?;
let report = database.schema().validate_owned_schema(&target, &ownership).await?;
let plan = database.schema().plan_owned_migration(
    "host-schema-2", "customer schema", &target, &ownership, PlanOptions::strict(),
).await?;
// Host reviews diagnostics/risks and retains the exact reviewed plan.
let applied = database.schema().apply_migration(&plan, ApplyOptions {
    expected_current_schema_hash: plan.source_schema_hash().cloned(),
    ..ApplyOptions::default()
}).await?;
```

## B: transactional runtime mutations

### Inputs, keys, and executor

Inputs are owned schema-bound requests, built only by `ValidatedRuntimeSchema`.
Missing field entries mean omission. `RuntimeValue::Null` means explicit SQL
NULL. Concrete values retain exact current value semantics. A create omission
uses a declared default/generator, otherwise nullable NULL, otherwise fails
`missing_required`. Update omission leaves the field unchanged. Explicit NULL
never requests a default. No new SetDefault/reset behavior in this slice.

```rust,ignore
impl ValidatedRuntimeSchema {
    pub fn runtime_key(&self, collection: &RuntimeCollectionHandle,
        fields: &[(RuntimeFieldHandle, RuntimeValue)])
        -> Result<RuntimeKey, RuntimeMutationError>;
    pub fn runtime_create_request(&self, collection: &RuntimeCollectionHandle,
        fields: &[(RuntimeFieldHandle, RuntimeValue)], returning: Option<RuntimeProjection>,
        limits: RuntimeMutationLimits) -> Result<RuntimeMutationRequest, RuntimeMutationError>;
    pub fn runtime_update_request(&self, key: RuntimeKey,
        fields: &[(RuntimeFieldHandle, RuntimeValue)], expected: Option<RuntimePredicate>,
        returning: Option<RuntimeProjection>, limits: RuntimeMutationLimits)
        -> Result<RuntimeMutationRequest, RuntimeMutationError>;
    pub fn runtime_delete_request(&self, key: RuntimeKey,
        expected: Option<RuntimePredicate>, returning: Option<RuntimeProjection>,
        limits: RuntimeMutationLimits) -> Result<RuntimeMutationRequest, RuntimeMutationError>;
}
impl<'tx, B: RuntimeMutationBackend> MutationContext<'tx, B> {
    pub async fn mutate_runtime(&mut self, schema: &ValidatedRuntimeSchema,
        request: &RuntimeMutationRequest, authority: &dyn RuntimeWriteAuthority<B>)
        -> Result<RuntimeMutationEffect, RuntimeMutationError>;
}
```

`RuntimeKey` requires exactly every primary-key field, no extras, duplicates,
NULLs, wrong kinds, or stale handles; preserve declared composite order.
Generated fields cannot be supplied. Primary keys are immutable after create;
other immutable-field rules are host field authorization. Reject any denied
input, rather than dropping it. Reject empty patches, append-only update/delete,
and incompatible expected predicates before DML. Bounds cover input fields,
bytes/JSON depth, key arity, proposed/preimage bytes, predicates, and binds.
MSSQL/third-party capability rejection happens before transaction entry in
adapters; low-level calls reject before any statements.

Expected state reuses `RuntimePredicate`, including explicit null tests and
typed conjunctions. Existing filterability/operator restrictions apply;
non-filterable expected fields and JSON equality are rejected. No built-in
version column is required. Authorization of CAS fields is separate from
authorization of changed fields because conflicts can disclose information.

### Authority and atomicity

`RuntimeWriteAuthority<B>: Send + Sync` is object-safe with lifetime-bound
`BoxFuture<'a, Result<_, RuntimeMutationError>>` methods (futures are Send):

```rust,ignore
fn authorize_intent<'a>(&'a self, check: RuntimeWriteIntent<'a>,
    tx: &'a mut MutationContext<'_, B>)
    -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>>;
fn authorize_preimage<'a>(&'a self, check: RuntimePreimageCheck<'a>,
    tx: &'a mut MutationContext<'_, B>)
    -> BoxFuture<'a, Result<(), RuntimeMutationError>>;
fn authorize_result<'a>(&'a self, check: RuntimeResultCheck<'a>,
    tx: &'a mut MutationContext<'_, B>)
    -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>>;
```

Checks borrow schema handles/intent/records. The trusted grant specifies the
minimum internal policy projection and optional structural row predicate.
It separately approves collection operation, all inputs/CAS fields, and
requested return fields. `RuntimeReturnGrant` approves the final row and exact
return projection; no implicit read grant follows from write permission.
No default allow implementation; missing request authority or any authority
error denies access. `DbAuthContext` supplements this contract. A host may
deliberately install an explicit public authority for public operations.

`RuntimeWriteIntent` borrows the action (Create/Update/Delete), collection,
affected field/value entries, optional RuntimeKey/expected predicate and
requested return projection. The intent method must approve the entire input
field set, not return a narrowed set. `RuntimeWriteGrant::new(policy_projection,
row_predicate)` validates same-schema/collection ownership; the engine privately
unions that minimum policy projection with keys, changed/CAS fields and return
fields as needed. `RuntimePreimageCheck` contains intent plus the authoritative
policy record. `RuntimeResultCheck` contains intent, optional preimage, and the
actual staged policy record (preimage for delete). `RuntimeReturnGrant::new`
accepts an optional exact return projection; any deviation from the request
is an error, not silent field removal. Full internal records never escape in
RuntimeMutationEffect. Every check/request/grant is redacted in Debug.

`RuntimeMutationBackend` is an additive capability on transaction/decoder
backends; `RuntimeMigrationBackend` and C's RuntimeReadBackend follow the same
pattern. Provide explicit unsupported implementations for MSSQL/sentinels
where their underlying traits permit it, with capability checks before I/O.
Third-party implementations acquire no new required trait methods; they can
opt into the separate capability or remain unsupported.

The engine requires `TransactionMode::StateMachine`: SQLite acquires its
write lock before the first preimage/policy read; PostgreSQL uses existing
serializable isolation and locks the existing exact-key row. Intent grants,
policy repository queries, authoritative preimage, DML, final authorization,
return decoding, and host journal writes use this same pinned transaction.
No pool-based query participates in these checks. Complete-key DML includes
the structural expected predicate; assert zero/one affected row. A stale
expected value returns `conflict`, never a protected current value. Absent or
invisible rows use a single `not_found` category; distinguish conflict only
after authorizing the visible preimage. Serialization/deadlock/busy failures
are retryable transaction failures, distinct from deterministic CAS conflict.

Server defaults/generated values can be unknown before DML. Authorize intent
and preimage first, stage DML, then authorize the **actual** proposed result
including defaults/trigger results before commit. Denial rolls back. Do not
manufacture a preview with a guessed timestamp. Generated UUIDs are minted
by ORM; ambiguous generators fail before I/O. Cascades/SET NULL may affect
other rows: the initial exact-record engine rejects deletes with incoming
modifying FK actions (`unsupported_cascade`) before DML unless a later reviewed
contract authorizes every affected record. Restricting FKs remain supported;
static cascade behavior is unchanged.

Mutation errors poison the encompassing runtime transaction: catching denial,
CAS failure, hook failure, or decoding/limit errors cannot commit staged work.
Introduce an internal rollback-only flag and preserve existing static behavior.
Returning NULL instead of denying a requested protected field is not allowed.
Delete return projection applies to the authorized preimage; create/update
return projections apply to the staged result. `returning: None` supports writes
with no record output; internal policy/key projections stay private.

`RuntimeMutationEffect` is owned and redacted in Debug; it exposes action and
authorized `Option<RuntimeRecord>` only and explicitly represents **pending**
work. It is not an event or committed success. The outer runner is the commit
boundary. Runtime mutation code emits no event by itself. Existing static
repository events remain deferred until successful commit.

### Host journal example

```rust,ignore
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "host_journal", plural = "HostJournal")]
struct Journal {
    #[primary_key]
    id: String,
    operation_id: String,
    action: String,
}

// Resolve handles from the active Arc<ValidatedRuntimeSchema>.
let key = schema.runtime_key(&customers, &[(id.clone(), RuntimeValue::Integer(7))])?;
let expected = schema.runtime_compare(&customers, &status,
    RuntimeScalarOperator::Eq, RuntimeValue::String("draft".into()), query_limits)?;
let request = schema.runtime_update_request(key,
    &[(status.clone(), RuntimeValue::String("published".into()))], Some(expected),
    Some(public_projection), RuntimeMutationLimits::default())?;

let committed = database.transaction_with_auth(TransactionMode::StateMachine,
    db_auth.as_ref(), |tx| Box::pin(async move {
        let pending = tx.mutate_runtime(&schema, &request, authority.as_ref()).await
            .map_err(OrmPublicError::from)?;
        tx.insert::<Journal>(CreateJournalInput {
            id: journal_id, operation_id, action: "update".into(),
        }).await?;
        // Both the mutation and journal insert are uncommitted here.
        Ok(pending)
    })).await?;
// Only this successful return permits a response or in-process notification.
```

The host registers the journal entity's existing static repository policies
and hooks; runtime authority does not bypass them. Journal insertion errors
propagate from the callback, rolling back both changes. GraphQL D offers a
`RuntimeMutationHook<B>: Send + Sync` whose `before_commit` method receives
`&mut MutationContext<'_, B>` and the pending authorized effect and returns a
Send BoxFuture. It performs this same journal insert, without using `defer`.
The hook is host-provided and product-neutral; hooks must not send external
notifications during the callback.

Cancellation/drop before commit rolls back; no detached mutation task. During
commit, cancellation or a transport error can leave the outcome unknown.
Retain existing `TransactionError` variants and add a conservative
`commit_outcome()` accessor with known-rollback/unknown classifications;
existing `Failed` is conservatively unknown unless rollback is established.
The runtime GraphQL adapter reports `commit_unknown` at an ambiguous commit
boundary and withholds success/events. Retryable means retry the entire
transaction after rollback, not exactly-once delivery. Host operation IDs and
repository reconciliation remain host responsibilities.

Safe runtime codes include `invalid_input`, `missing_required`, `field_denied`,
`denied`, `not_found`, `conflict`, `constraint_violation`, `append_only`,
`unsupported_backend`, `unsupported_generation`, `unsupported_cascade`,
`schema_mismatch`, `limit_exceeded`, `retryable_transaction`, `hook_failed`,
`database_failed`, and `commit_unknown`. Trusted error sources retain driver
details; Display, GraphQL extensions, Debug, and tracing redact values, SQL,
physical identifiers, credentials, and preimages. Error conversion into
`OrmPublicError` must preserve machine codes through the transaction runner.

## C: read-only dynamic GraphQL

Add `runtime-graphql = ["async-graphql/dynamic-schema"]`; default features stay
unchanged. Registration belongs in `graphql_orm::graphql::runtime`, gated by
that feature. Framework-neutral A/B use `graphql::orm` without it.

```rust,ignore
impl RuntimeGraphqlModule {
    pub fn compile(schema: Arc<ValidatedRuntimeSchema>, options: RuntimeGraphqlOptions)
        -> Result<Self, RuntimeGraphqlDiagnostics>;
    pub fn descriptor(&self) -> &RuntimeGraphqlDescriptor;
}
impl<B: RuntimeReadBackend> RuntimeGraphqlComposer<B> {
    pub fn new(database: Database<B>, query_name: impl Into<String>,
        limits: RuntimeGraphqlLimits) -> Result<Self, RuntimeGraphqlError>;
    pub fn query_field(self, spec: RuntimeHostField)
        -> Result<Self, RuntimeGraphqlError>;
    pub fn register(self, ty: async_graphql::dynamic::Type)
        -> Result<Self, RuntimeGraphqlError>;
    pub fn install(self, module: RuntimeGraphqlModule)
        -> Result<Self, RuntimeGraphqlDiagnostics>;
    pub fn finish(self) -> Result<async_graphql::dynamic::Schema, RuntimeGraphqlError>;
}
```

The immutable module owns its Arc schema and compiled name/cost descriptors;
it stores no catalog or authority. Runtime options carry an optional explicit
type prefix/root naming map keyed by stable IDs. Public fields/arguments are
camelCase, named types PascalCase; reject names violating that profile instead
of silently rewriting existing RuntimeSchema API names. Collection plural
root conversion uses one documented deterministic case rule and collision
check. Resolver naming feature flags for static entities do not change it.

Composer builds fresh dynamic roots and registers every host/module type
through one checked name map. async-graphql's raw `SchemaBuilder::register`
overwrites duplicate types and exposes no public inventory; do not promise a
collision-safe installer into an arbitrary existing builder. Host root fields
use `RuntimeHostField::new(name, TypeRef, resolver)` so the composer owns and
checks the actual name before creating the dynamic Field. Host non-root types
are accepted as dynamic Types whose public variant accessors identify names.
Hosts build their own types/fields normally but submit them through this
boundary. No unchecked builder escape hatch before finish. D adds mutation
root field methods. Reject collisions regardless of registration order,
including builtins, helper/filter/input/connection names, roots, and explicit
reserved namespaces. Share ORM helper scalars only by an exact descriptor;
arbitrary same-name host types are not treated as equivalent.

Expose checked forwarding methods for host-owned data, extensions, custom
directives, introspection policy, depth/complexity settings and optional
subscription roots; these do not add ORM subscriptions or transport. Keep host
configuration choices while reserving the internal operation preflight and
request budget enforcement. Do not require ORM to own the host's entire
server configuration or add an opaque builder callback that bypasses collision
checks. Generic register support covers host interfaces/unions/scalars too.

Proposed SDL shape for a Customer collection (abbreviated):

```graphql
type Query {
  customers(where: CustomerWhereInput, orderBy: [CustomerOrderInput!],
    first: Int, after: String, last: Int, before: String): CustomerConnection!
}
input CustomerWhereInput {
  and: [CustomerWhereInput!]
  or: [CustomerWhereInput!]
  not: CustomerWhereInput
  status: RuntimeStringFilter
}
type CustomerConnection {
  edges: [CustomerEdge!]!
  pageInfo: RuntimePageInfo!
  totalCount: RuntimeInt64!
}
type CustomerEdge { cursor: String!, node: Customer! }
type Customer {
  id: RuntimeInt64!
  status: String!
  contacts(first: Int, after: String, last: Int, before: String,
    where: ContactWhereInput, orderBy: [ContactOrderInput!]): ContactConnection!
}
```

To-one relations return nullable target objects (missing/filtered targets are
NULL), not arbitrary first matches. Counts execute only if selected, including
through fragments/aliases; unrequested counts incur no count query. The
non-null selected count requires explicit authority. Order input carries a
closed field enum, direction and null placement. Filters expose only the
underlying supported operators for filterable kinds; no JSON equality,
relation predicates on parents, spatial operators, or advanced aggregates.
Omitted pagination uses the host's bounded default page size. Mixing forward
and backward arguments, zero/negative/oversized sizes, or cursor/shape mismatch
fails validation; do not silently clamp the existing runtime contract.

| Runtime kind | Dynamic scalar and exact coercion |
| --- | --- |
| Boolean | GraphQL Boolean |
| Integer/count | RuntimeInt64: canonical signed decimal string, exact i64; reject float/inexact numeric coercion |
| Float | GraphQL Float, finite `RuntimeFloat` rules |
| String | GraphQL String, no normalization |
| UUID | RuntimeUuid: parse UUID strings, emit canonical UUID |
| JSON | RuntimeJson: bounded JSON values; JSON null is distinct from SQL NULL internally |
| Bytes | RuntimeBytes: canonical base64 string, bounded decoded bytes |
| DateTime | RuntimeDateTime: existing RFC3339 parser, emit canonical UTC microseconds |

These exact custom scalar wire forms are a review decision for the new opt-in
module; they do not rewrite existing static Int, JSON, UUID, bytes or date
SDL. Static/runtime conformance compares normalized SDL and exact logical
values using the documented scalar mapping, and separately snapshots existing
static SDL unchanged. Review must explicitly accept the full-i64 decimal-string
and bytes-base64 forms rather than imply identical static wire types.

SQL NULL uses GraphQL null. JSON null also serializes as GraphQL null on output;
document this representational limitation. To preserve JSON null versus SQL
NULL for D inputs, RuntimeJson input uses an exact JSON-text string (the string
`"null"` is JSON null; GraphQL null is SQL NULL); ordinary JSON output stays
structured. Unloaded fields are never coerced to NULL: selection/project
mismatch is an internal safe error. Existing `gormrq1` and `gormrr1` encodings
pass through unchanged, with full underlying stale/order/parent validation.
Cursor envelopes are opaque but neither secret nor authorization tokens;
internally selected keys stay out of records/GraphQL fields/errors and are
never added as extra output. Hosts needing secret cursors require a separate
reviewed protection contract, not an undocumented encoding change here.
This is a material review boundary: current cursor envelopes contain typed
order values. Keeping those fields Unloaded does not provide cryptographic
confidentiality of cursor contents. If "private keys" means secret even to a
client decoding its cursor, C needs an explicitly reviewed cursor-protection
addition; preserving the current envelope alone cannot meet that stronger
requirement. Do not declare the privacy gate closed without resolving it.

### Request authority, limits, and batches

`RuntimeReadAuthority: Send + Sync` is object-safe; a Send BoxFuture method
accepts a borrowed `RuntimeReadCheck` and returns `RuntimeReadGrant` with an
owned validated row predicate. Check includes collection, selected projection,
filter/order fields (including effective default order), relation traversal,
count intent, and schema identity. Authorization sees stable IDs and typed
operations, not client-supplied physical names. Explicit allow-all for public
hosts still requires a provider; absence/error denies.

```rust,ignore
pub trait RuntimeReadAuthority: Send + Sync {
    fn authorize<'a>(&'a self, check: RuntimeReadCheck<'a>)
        -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>>;
}
// A host compiles its own tenant restriction with existing constructors.
let tenant_only = schema.runtime_compare(&customers, &tenant,
    RuntimeScalarOperator::Eq, RuntimeValue::String(principal_tenant), query_limits)?;
// Inside authorize(), after approving every requested capability:
let grant = RuntimeReadGrant::new(Some(tenant_only));
```

The grant is owned, private and checked against the current check before use;
`None` explicitly grants an unrestricted structural row set, still requiring
all capability checks. Scalar field/operator checks and relation/count grants
belong in authorize, even for a host using PostgreSQL RLS. Request data owns
the principal/policy closure; authority methods borrow it and do not require
another canonical principal model.

The host attaches `RuntimeGraphqlRequest<B>` to each async-graphql Request.
It owns an Arc authority, optional owned DbAuthContext, expected
SchemaFingerprint, request budget, and fresh relation batch state. The
execution-only batch scope is private and newly allocated per GraphQL
execution, even if host request data is reused. Trusted internal cursor/relation
key acquisition is explicitly part of a granted operation; it does not grant
client filtering/ordering/output access to those keys. Use the exact existing
hidden-key/anchor protections; reject operators on denied fields before SQL.

Preflight all selected ORM subtrees, expand fragments/aliases and evaluated
directives, validate/coerce variable inputs, authorize each selected layer, and
reserve a bounded cost before the first ORM statement. Host-owned resolvers
remain responsible for their own I/O. AND policy/application predicates before
constructing row/count requests. Never fetch then filter a page. For each
relation layer, independently combine target policy predicates before
per-parent pagination/counts and propagate the same RLS auth context.

Install a mandatory per-execution async-graphql extension: its prepare/parse
hooks bound request/query/variable sizes and recursion before parser/selection
expansion, and its execution hook completes ORM preflight before invoking
resolvers. Host extensions cannot remove the internal guard. The host still
bounds HTTP body decoding before constructing an already-owned Request; ORM
does not own transport allocation. Check variable trees iteratively with
bounded traversal instead of serializing/cloning an arbitrarily deep input to
measure it.

Execute via `execute_runtime_read`, `execute_runtime_anchored_read`, and
`execute_runtime_relation_batch`; keep schema/auth request scopes isolated.
Batch identity contains the schema, relation, projection, predicate, order,
page shape, backend and private authority/RLS scope. Equal principal IDs do
not merge authority identities. One compatible selected layer uses one bounded
union statement plus one optional grouped count, not one query per parent;
multiple shapes/groups consume explicit budgets. Avoid unbounded prefetching.
Existing per-request limit overflows fail rather than silently splitting into
unlimited queries. Top-level aliases similarly consume the query budget.

Resource limits cover schema/type/name bytes, GraphQL operation/variable bytes,
fragments/aliases, depth, recursive filters, scalar decoding, projection/page
sizes, relation parents/groups, total statements, count work, and maximum
materialized nodes/bytes. Expose `descriptor().cost_metadata()` and a
`RuntimeGraphqlCost` estimate with checked arithmetic for host admission.
Count requests can still scan many rows; bound count operations/time, without
claiming a guaranteed database scan-row bound. Database timeouts are host
configured. Reject limits before I/O or allocation beyond the budget; reserve
incremental budgets before relation allocations. No global loader/cache.

Minimal read host composition (proposed APIs):

```rust,ignore
let module = RuntimeGraphqlModule::compile(schema.clone(), RuntimeGraphqlOptions::default())?;
let dynamic = RuntimeGraphqlComposer::new(database.clone(), "Query", graphql_limits)?
    .query_field(RuntimeHostField::new("health", TypeRef::named_nn("Boolean"),
        |_| FieldFuture::new(async { Ok(Some(FieldValue::value(true))) })))?
    .install(module)?.finish()?;
let request_context = RuntimeGraphqlRequest::<SqliteBackend>::new(
    schema.fingerprint(), read_authority, db_auth, request_budget,
);
let response = dynamic.execute(Request::new(
    "{ customers(first: 20) { edges { node { status } } totalCount } }",
).data(request_context)).await;
```

The host's read authority separately grants status projection, effective key
order and counts and returns, for example, a validated tenant predicate. The
same runtime predicate constructors used in B's example supply that predicate.
The adapter ANDs it with user filters. Host transport and schema publication
remain outside this module.

Safe dynamic codes preserve underlying runtime codes and add
`name_collision`, `unsupported_scalar`, `invalid_graphql_input`,
`authorization_missing`, `cost_exceeded`, and `invalid_composition`.
Publication is only the host calling finish and adopting the resulting schema;
no active-schema pointer or catalog write in ORM.

### Ownership and lifetimes across all PRs

Validated schemas, handles, values, requests, owned physical targets/plans,
module descriptors, limits and safe errors own their data and are Send + Sync.
Use Arc for shared immutable schema/module/policy storage. Constructors borrow
slices/handles and copy into bounded owned requests; no leaked strings or
caller `&'static` requirements. Dynamic resolver closures own Database clones
and Arc metadata. Host policy/hook objects retained by request/schema data are
Send + Sync + 'static; checks and Send futures borrow for the invocation only.
MutationContext is exclusively borrowed through the existing runner's HRTB
BoxFuture lifetime and cannot escape or be cloned/shared for concurrent SQL.
Journal writes borrow it sequentially; no mutex is held across policy awaits.
Cancellation does not spawn hidden work. No driver rows/pools or GraphQL
Context enter framework-neutral policy signatures.

## D: dynamic mutation registration

Extend `RuntimeGraphqlOptions` through a builder method
`with_mutations(RuntimeGraphqlMutationOptions)`; reads remain the default.
Reuse C naming/coercion/composition and B input construction/engine. Add
`RuntimeGraphqlComposer::mutation_root(name)` and checked `mutation_field`.
Attach write authority and optional before-commit hook with request-context
builder methods. No GraphQL Context is required by the framework-neutral B
authority or journal API.

```graphql
input CustomerKeyInput { id: RuntimeInt64! }
input CreateCustomerInput { status: String! }
input UpdateCustomerInput { status: String }
type CustomerMutationPayload { record: Customer }
type Mutation {
  createCustomer(input: CreateCustomerInput!, expectedSchema: String!): CustomerMutationPayload!
  updateCustomer(key: CustomerKeyInput!, input: UpdateCustomerInput!,
    expected: CustomerWhereInput, expectedSchema: String!): CustomerMutationPayload!
  deleteCustomer(key: CustomerKeyInput!, expected: CustomerWhereInput,
    expectedSchema: String!): CustomerMutationPayload!
}
```

Non-null update inputs remain optional in GraphQL but reject supplied null for
non-null storage. Nullable input omission and explicit null are tested
separately. Create fields with defaults are optional without adding GraphQL
schema defaults (which would erase omission). Generated fields are absent
from create/update; keys absent from update; extra fields fail normal input
validation. CAS uses C's typed filters and B's field authorization. Per-request
denied fields need not disappear from global SDL, but cannot be submitted.

Compare expectedSchema and the request expectation against the module's
fingerprint before I/O. The host chooses/pins its active catalog revision and
maps that broader product identity to the fingerprint; the ORM cannot know
that a newer activation occurred, or enforce a product lease. Fingerprints do
not encode policy identity or authenticate clients.

Resolve selected payload record fields into an independently authorized
RuntimeProjection before mutation. If only `__typename` is selected, request
no return record; never expose the internal key as a fallback. Execute one
state-machine transaction per mutation root, call the host before-commit hook
with the pending effect, then commit before resolving a successful payload.
Denials, CAS conflicts, and commit failures are safe GraphQL errors, with no
protected values in conflict payloads. No relation/nested mutation payload
fields in this initial slice; scalar record projection only. Multiple roots
follow GraphQL serial execution and are separate transactions; there is no
operation-wide atomicity promise.

Minimal host delta:

```rust,ignore
let module = RuntimeGraphqlModule::compile(schema.clone(),
    RuntimeGraphqlOptions::default()
        .with_mutations(RuntimeGraphqlMutationOptions::default()))?;
let dynamic = RuntimeGraphqlComposer::new(database, "Query", graphql_limits)?
    .mutation_root("Mutation")?.install(module)?.finish()?;
let request_context = RuntimeGraphqlRequest::<SqliteBackend>::new(
    schema.fingerprint(), read_authority, db_auth, request_budget,
).with_write_authority(write_authority).with_mutation_hook(journal_hook);
// journal_hook.before_commit uses tx.insert::<Journal>(...) as in B.
let response = dynamic.execute(Request::new(
    "mutation($schema: String!) { updateCustomer(key: {id: \"7\"}, \
      input: {status: \"published\"}, expected: {status: {eq: \"draft\"}}, \
      expectedSchema: $schema) { record { status } } }",
).variables(Variables::from_json(serde_json::json!({"schema": schema.fingerprint().to_string()})))
 .data(request_context)).await;
```

## Independent repository aggregate fix

Condition GraphQL Enum derives/attributes/implementations on the entity's
GraphQL surface. Repository mode retains Clone/Copy/Debug/Eq/Hash/PartialEq,
the aggregate-field enum, TypedAggregateField implementation, and plain Rust
aggregate builder. Preserve ordinary GraphQLEntity aggregate SDL exactly.

Add minimal **external** fixture crates with graphql-orm plus required Rust
dependencies and no direct async-graphql dependency for SQLite/PostgreSQL/MSSQL.
Compile a repository entity with public persisted aggregate fields and actually
type-check aggregate builder usage; existing in-package tests have the direct
dependency and cannot prove this regression fixed. Include negative checks for
GraphQL traits on the repository enum, normal aggregate SDL snapshots, and
static repository backend regressions. Check emitted tokens for scalar and
composite keys and sensitive/private/unreadable fields, and run actual
repository aggregate authorization failure tests on owned SQLite/PostgreSQL
databases. This does not remove async-graphql from
the ORM dependency graph.

## Acceptance gates and verification

Every functional PR includes public reference documentation, runnable examples
on SQLite and PostgreSQL, root changelog/migration notes, owning package README
updates and aligned versions; regenerate the workspace inventory after manifest
changes. The contract PR changes documentation only and requires no version
bump. Existing accepted ADRs remain immutable; add an ADR only if review adopts
a durable new boundary requiring one.

| PR | Required focused evidence |
| --- | --- |
| Fix | External dependency-minimal builds on three backends; Rust aggregate helpers; GraphQL SDL unchanged |
| A | Static/runtime physical equivalence; no-op replans both origin directions; defaults/keys/indexes/FKs; public rename no DDL; unsupported diagnostics; read-only plan/history; explicit ownership removal; destructive/additive/source-hash guards; rollback on failed apply; repeated schema conversion memory bounded |
| B | Omission/null/default/generated inputs; denied inputs/CAS/preimage/result/return fields; append-only; exact composite keys; two competing writers; stale CAS; authoritative transactional policy reads; journal/hook/commit rollback; swallowed-error poisoning; cancellation; ambiguity; RLS cleanup; safe errors |
| C | Exact SDL/scalars/null/cursors; composition collision permutations; missing authority; tenant predicate before page/count; denied projection/filter/order/relation/count before SQL; fragments/aliases; stale handles/cursors; hidden keys; auth scope isolation; nested relation query counts; schema and operation budgets |
| D | B/C engine delegation; missing versus null/default; generated/key input exclusions; expected schema; CAS; hook journal rollback; independently denied return fields; committed payloads only; competing GraphQL writers; host mutation collision; ambiguous commit errors |

Use shared owned-database fixture infrastructure for new parity tests, with
test-created labelled containers, loopback ports, unique database/credentials,
identity checks before destructive cleanup, and verified removal. Docker
failure is failure, not a passing skip. Never pass ambient database URLs to
legacy destructive tests. Existing owned runner currently covers aggregates;
extend it to select each new runtime suite rather than calling a skipped legacy
test and claiming parity. The canonical [testing guide](../../../development/testing.md)
and [PostgreSQL runbook](../../../operations/runbooks/postgres-testing.md)
remain authoritative.

Required commands per applicable code PR, recorded with exact outputs/status:

```sh
cargo fmt --all -- --check
python3 scripts/check-documentation.py
python3 scripts/generate-workspace-inventory.py --check
scripts/check-workspace-dependencies.sh
scripts/check-package-release-policy.sh <exact-base-sha>
cargo test -p graphql-orm -p graphql-orm-macros --locked --no-default-features --features sqlite
cargo test -p graphql-orm --locked --no-default-features --features sqlite --doc
scripts/run-owned-database-lanes.sh sqlite
scripts/run-owned-database-lanes.sh postgres
```

For each of `sqlite`, `postgres`, `mssql`, and `sqlite,mssql`, run explicit
`cargo check -p graphql-orm -p graphql-orm-macros --locked --no-default-features
--features <lane>`. Run warnings-denied Clippy on affected backend targets and
Rustdoc/doctests on the supported API lanes, using `RUSTDOCFLAGS="-D warnings"`.
C/D additionally run separate `sqlite,runtime-graphql` and
`postgres,runtime-graphql` test/Clippy/Rustdoc lanes, unsupported MSSQL dynamic
capability compile tests, and a tree proving the feature is absent by default.
Tree checks select the same backend features with `cargo tree -p graphql-orm
--locked --no-default-features --features <lane>` and check workspace duplicates.
No workspace all-features command. Do not mark acceptance met based only on
compilation or unexecuted/skipped PostgreSQL tests.

Each PR handoff records URL/status, exact base/head SHA, versions/features,
new public calls and runnable examples, commands/results, migration and
compatibility notes, limitations, and review/merge state. No integration pin
is advertised until its contract is committed and reviewed.

## Exact consumer integration sequence

1. Consume each reviewed merged monorepo commit through a full Git revision,
   with one graphql-orm/macros dependency universe. Choose `default-features =
   false` and `features = ["sqlite"]` or `["postgres"]`; add `runtime-graphql`
   only for C/D. Host direct dynamic types may use the ORM async_graphql re-export.
2. Load host catalog definitions into the existing RuntimeSchema, validate,
   retain `Arc<ValidatedRuntimeSchema>`, and resolve fresh handles on activation.
3. A: create the owned physical target, compose trusted system entities, supply
   the explicit persisted managed table set including intentionally removed
   tables, validate/plan, review exact risks/hashes, then explicitly apply.
   Product approval, activation and backfills remain host-owned.
4. B: implement RuntimeWriteAuthority and static system repository policies;
   use state-machine transactions and tx.insert for journal work. Publish a
   success only after the runner returns successfully. Reconcile ambiguous
   commits using host operation IDs without blind retries.
5. C: compile/install a module through the checked composer; supply per-request
   read authority, row predicates, schema expectation, RLS context and budgets.
   Host constructs and publishes the active GraphQL schema/transport.
6. D: explicitly enable mutation registration, attach write authority and the
   before-commit journal hook, send expectedSchema and optional structural CAS,
   and authorize the selected return projection independently.
7. Run downstream SQLite/PostgreSQL adapter contracts. Upstream green checks
   establish mechanisms, not product readiness or durable event replay.

## Current checkpoint

Proposal prepared against the exact remote base; no functional APIs implemented.
Contract review must settle the concrete-alias generic owned-index model and static
source compatibility, explicit ownership plan integrity, generated/default
mapping, conservative cascade rejection, authority/poisoning/commit-outcome
surface, checked dynamic composer, exact scalar wire forms, naming profile,
cursor privacy interpretation, and request cost/batching rules. Then implement the independent fix and each
bounded functional PR with its own committed evidence and handoff.
