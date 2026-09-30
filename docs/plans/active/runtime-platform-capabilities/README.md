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
`a90f229da66416e70f094c27e01f0bf4b4edc6bd`, also ORM/macros 0.33.1.
The original reviewed proposal used `131693bdb4a74cf43732cb279419701fe273c1d0`. A Git diff
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
| B | Contract review and A's verified composed physical target | Transactional runtime mutation engine | 0.35.0 |
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

Keep the existing public SchemaModel, TableModel, MigrationStep, SchemaDiff,
MigrationPlan, PlannedMigrationStep and PlannedMigration definitions and
namespaces intact. In particular MigrationStep remains an enum, not a type
alias: `use MigrationStep::CreateTable` and `use MigrationStep::*` must compile.
The previous alias proposal was incorrect; a Rust type alias cannot serve as
an importable enum module. Keep all existing function signatures intact too.

Add owned index storage (`IndexModel`: String names, Vec<String> columns and
owned directions/predicates) as the owned storage form of the same canonical
physical contract. OwnedSchemaModel/owned step storage expose that contract
without requiring static strings. Private borrowed schema/index/step views
adapt both existing models and owned storage into **one** comparison,
classification, hashing, rendering and apply implementation. Compatibility
adapters contain storage conversion only, no planner decisions. Do not
convert an owned index back to IndexDef by leaking memory or replace existing
enum variants. No second semantic schema IR or parallel SQL/planning engine.
Document the owned storage types and legacy adapters together as one contract.

OwnedPlannedMigration contains private owned canonical steps and
ownership/integrity data, with read-only inspection and hash accessors. Add
`apply_owned_migration` as a compatibility adapter into the existing guarded
apply machinery. The existing `apply_migration(&PlannedMigration, ApplyOptions)`
retains its exact signature, avoiding changes to existing function pointers or
inference. Both methods share policy, risk, baseline, transaction and history
checks; the owned method adds explicit scope verification. Static IndexDef,
const constructors, enum namespace, public struct literals and hashes stay
unchanged.

The [external migration compatibility fixture](../../../../crates/graphql-orm/tests/fixtures/migration-api-compatibility/src/lib.rs) in this contract PR compiles
variant/glob imports, qualified constructors, exhaustive matches, unannotated
values, original struct literals and public function/method pointers against
the actual ORM. Its SQLite, PostgreSQL and MSSQL tests pass without database
I/O; A must retain this executable source-compatibility evidence. A must retain that exact fixture across every backend lane
and test owned/static adapters against the same semantic implementation.

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
    pub async fn apply_owned_migration(
        &self, plan: &OwnedPlannedMigration, options: ApplyOptions,
    ) -> Result<AppliedMigrationReport, RuntimeMigrationError>;
    pub async fn runtime_mutation_environment(
        &self, schema: Arc<ValidatedRuntimeSchema>, target: &OwnedSchemaModel,
        ownership: &ManagedTableSet,
    ) -> Result<RuntimeMutationEnvironment, RuntimeMigrationError>;
}
// Existing apply_migration retains its original signature and enum types.
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

Owned apply must not run a target-RLS reconciliation with an empty RLS target.
Execute only reviewed owned table steps; preserve existing PostgreSQL RLS
enable/force bits, policy definitions, owners, grants and functions on surviving
tables. Reject a table rebuild/alter whose RLS/dependency preservation is not
provable rather than silently clearing it. Tests install host/system RLS and
unrelated tables/policies, compare catalog state before/after owned apply and
prove policies still enforce the same access. Unrelated objects are never
touched. An explicit reviewed deletion of an owned table naturally removes
that table's own dependent objects; this is a destructive gate, not RLS
reconciliation authority.

At composition, build incoming FK dependencies from the **complete composed
physical target**, including static/system tables, using ordered physical
key pairs and declared delete actions, not runtime relation names alone.
Owned live introspection also checks the full physical schema for external
incoming dependencies. A rejects unsafe unmanaged dependencies. B consumes
an immutable verified RuntimeMutationEnvironment derived from this composed
target plus its managed ownership and live-validation identity. It is bound
to the runtime schema fingerprint, but is not another schema IR: it is an
execution-capability/dependency certificate. The host installs it only after
A validation/apply and replaces it with its pinned schema generation.
Runtime-only schema metadata cannot certify absent system-table FKs. Missing,
stale or incomplete dependency verification fails closed before DML. If live
physical schema can change outside the host's migration fence, revalidate the
certificate or refuse writes; there is no implicit assumption of ownership.

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
let applied = database.schema().apply_owned_migration(&plan, ApplyOptions {
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
    pub async fn mutate_runtime(&mut self, environment: &RuntimeMutationEnvironment,
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

An operation guard starts on the first poll of mutate_runtime, **before any
await or DML**, recording an unfinished operation in transaction-owned state.
It does not hold another mutable borrow of MutationContext. Only successful
completion of all operation validation/authorization/DML/decoding explicitly
disarms it. Error or guard Drop marks rollback-only; forgetting the guard
leaves an outstanding-operation count, which also prevents commit. State is
monotonic: later successes cannot clear rollback-only. An unpolled future
cannot have staged work. This preserves static-only transaction behavior.

The outer runner and commit_and_emit check both rollback-only and unfinished
operations immediately before commit, refuse commit even if the callback
returns Ok, roll back record/journal work, and discard queued side effects.
This explicitly covers cancellation of only the operation future while the
outer transaction stays alive. Catching a timeout, denial, CAS failure, hook
failure, or decode/limit error cannot commit staged runtime work.

Before-commit host hooks run through a guarded
`MutationContext::run_runtime_before_commit(hook, pending_effect)` method.
The wrapper holds a separate unfinished-operation guard across the hook's
future, including repository journal writes and awaits. Dropping/timing out
that future poisons the transaction even after mutate_runtime succeeded.
D must use this wrapper; a direct unguarded callback invocation is not the
supported hook path. A guard's Drop never performs async work: the runner
performs rollback. Cancellation of the whole runner retains transaction-drop
rollback; cancellation while committing can still be ambiguous.

Required owned SQLite/PostgreSQL tests stage DML, suspend authorize_result,
time out/drop mutate_runtime, catch the timeout **inside** the still-running
callback, then return Ok. Assert the runner rejects/rolls back, both record
and host journal remain unchanged, events are absent, and the connection is
reusable. Repeat with a before-commit hook that inserts a journal row then
suspends and is canceled. Also test caught denial, forgotten/incomplete
operations and cancellation before DML. No blind retry after unknown commit.

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
        let pending = tx.mutate_runtime(&environment, &request, authority.as_ref()).await
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

For a Customer with a required non-generated integer id, create must supply
that id. Every non-generated composite-key member is likewise included;
generated/server-managed keys alone are excluded. Keys remain absent from
update patches. Representative repository operations, to become compiled and
executed examples in B on both backends:

```rust,ignore
let create = schema.runtime_create_request(&customers, &[
    (id.clone(), RuntimeValue::Integer(7)),
    (status.clone(), RuntimeValue::String("draft".into())),
], Some(public_projection.clone()), mutation_limits)?;
let update = schema.runtime_update_request(
    schema.runtime_key(&customers, &[(id.clone(), RuntimeValue::Integer(7))])?,
    &[(status.clone(), RuntimeValue::String("published".into()))],
    Some(expected_draft), Some(public_projection.clone()), mutation_limits)?;
let delete = schema.runtime_delete_request(
    schema.runtime_key(&customers, &[(id.clone(), RuntimeValue::Integer(7))])?,
    Some(expected_published), Some(public_projection), mutation_limits)?;
// Run each request through its own transaction_with_auth callback, calling
// mutate_runtime(&environment, &request, authority), then tx.insert::<Journal>.
// For registered hooks use tx.run_runtime_before_commit(hook, &pending).
```

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
    pub fn cursor_protection(self, protector: Arc<dyn RuntimeCursorProtector>,
        audience: RuntimeCursorAudience, limits: RuntimeCursorProtectionLimits)
        -> Result<Self, RuntimeGraphqlError>;
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
| JSON | RuntimeJson: bounded valid JSON-text string on input **and output**; GraphQL null is SQL NULL, string `"null"` is JSON null |
| Bytes | RuntimeBytes: canonical base64 string, bounded decoded bytes |
| DateTime | RuntimeDateTime: existing RFC3339 parser, emit canonical UTC microseconds |

These exact custom scalar wire forms are a review decision for the new opt-in
module; they do not rewrite existing static Int, JSON, UUID, bytes or date
SDL. Static/runtime conformance compares normalized SDL and exact logical
values using the documented scalar mapping, and separately snapshots existing
static SDL unchanged. Review must explicitly accept the full-i64 decimal-string
and bytes-base64 forms rather than imply identical static wire types.

RuntimeJson is symmetric and lossless over RuntimeValue::Json values. Inputs
must be strings containing valid bounded JSON, not GraphQL object/list values;
outputs are compact JSON-text strings. `RuntimeValue::Null` alone emits
GraphQL null. `RuntimeValue::Json(serde_json::Value::Null)` emits the non-null
GraphQL string `"null"`, including for non-null JSON fields. Input omission is
a third state; explicit GraphQL null never means JSON null. JSON strings are
JSON-encoded within the scalar text (for JSON string `hello`, the scalar's
text is `"hello"`, including those JSON quotes). Whitespace/key-order spelling
need not round-trip, but parsing the output must reproduce the exact supported
serde_json::Value; no lossy conversion through JavaScript numbers. Invalid JSON,
out-of-range numbers, depth/node/text-byte bounds fail before DML/I/O. Existing
runtime JSON filter restrictions remain unchanged. Unloaded fields are never
coerced to NULL: selection/project mismatch is a safe internal error.

RuntimeInt64 accepts/emits only canonical signed decimal strings, grammar
`0|-?[1-9][0-9]*`, length at most 20, checked against i64 bounds. Reject plus,
leading zeros, negative zero, decimals/exponents, and numeric GraphQL inputs.
Counts use the same scalar. UUID accepts valid UUID strings and emits lowercase
hyphenated canonical form. Float accepts finite GraphQL numeric values and
uses RuntimeFloat normalization, including negative zero; reject NaN/infinity
and overflow. Bytes use RFC4648 standard alphabet and canonical padding;
check encoded/decoded byte bounds before allocation and reject noncanonical
encoding. DateTime input is an RFC3339 string, processed by RuntimeDateTime's
existing UTC normalization and microsecond rounding (including carry into the
next second); output is `YYYY-MM-DDTHH:MM:SS.ffffffZ`. Reject invalid or oversized
datetime text. Tests include i64 extrema and ±(2^53+1), JSON null versus SQL
NULL and non-null JSON, Unicode/structured JSON, exact byte boundaries, offsets,
six-digit precision and existing sub-microsecond rounding/carry semantics.

SDK scalar mapping is explicit: TypeScript maps RuntimeInt64/RuntimeJson/
RuntimeUuid/RuntimeBytes/RuntimeDateTime to string (nullable only for SQL NULL).
Use BigInt for integer arithmetic, exact JSON parsing when numeric fidelity is
required, Uint8Array after base64 decoding, and a string/precision-preserving
datetime parser rather than JavaScript Date for microseconds. Rust wire DTOs
use String newtypes and fallible conversions to i64, serde_json::Value, UUID,
Vec<u8>, and RuntimeDateTime. Neither SDK generation nor product client changes
belong here; document these mappings and compile host examples. Static
GraphQL scalars/SDL remain unchanged.

### Confidential cursor adapter

Preserve framework-neutral `gormrq1`/`gormrr1` encodings and static cursor APIs.
At the **dynamic GraphQL boundary**, introduce generic host-managed cursor
protection with explicit RuntimeCursorProfile::AuthenticatedEncryption and
an object-safe `RuntimeCursorProtector: Send + Sync + 'static`. The confidential
profile is required for Digibase and the default for this new module; module
installation fails without a provider. An explicitly chosen Unprotected
profile may preserve original cursor behavior for other hosts; profiles never
auto-detect or downgrade and no individual protected operation accepts raw
internal cursors. Signing or encoding readable values does not satisfy the
AuthenticatedEncryption provider contract.

```rust,ignore
pub trait RuntimeCursorProtector: Send + Sync {
    fn seal<'a>(&'a self, context: &'a RuntimeCursorContext,
        plaintext: &'a str, limits: RuntimeCursorProtectionLimits)
        -> BoxFuture<'a, Result<RuntimeSealedCursor, RuntimeCursorProtectionError>>;
    fn open<'a>(&'a self, context: &'a RuntimeCursorContext,
        key_id: &'a str, ciphertext: &'a [u8], limits: RuntimeCursorProtectionLimits)
        -> BoxFuture<'a, Result<String, RuntimeCursorProtectionError>>;
}
```

The trusted host implements authenticated encryption of the **entire** internal
cursor envelope with unique nonces and authenticates the supplied canonical
associated-data context. It owns the algorithm, keys, nonce management,
encryption-key selection, bounded decryption key ring, and expiry/revocation
policy. ORM never stores keys or assumes a signed token is confidential.
Provider results are redacted, validated/bounded by ORM, and never logged.
RuntimeSealedCursor carries a validated public key ID and opaque ciphertext
including provider nonce/tag/framing. ORM emits
`gormgqlc1.<key-id>.<base64url-no-padding-ciphertext>`; no order/parent/schema/
tenant values appear in public framing. Key IDs are bounded public labels,
not URLs or instructions to fetch untrusted keys.

Protect **every** edge cursor, startCursor and endCursor at every top-level
and relation layer; page-info absent cursors remain NULL. Reuse the same sealed
token for an edge and matching page-info cursor. Incoming after/before must
have the protected framing; check size/version/structure before decoding,
authenticate/decrypt before invoking the unchanged runtime cursor decoder.
Raw gormrq1/gormrr1/static cursors fail `invalid_cursor` in this profile with
no fallback. Decrypted envelopes still undergo all runtime schema/order/key
checks. Ciphertext cannot be used as a relation cache/authority identity.

RuntimeCursorContext is constructed privately from trusted module/request and
validated execution state, never from client claims about scope. Its bounded
canonical associated data includes protection version, host cursor audience
(application/project/endpoint namespace), schema fingerprint, collection,
cursor kind, and complete effective logical order signature. Relation scope
additionally includes relation/source/target identity and the exact typed
parent identity from its opaque ORM anchor. The host provides a stable trusted
authorization partition (principal/tenant/security realm) that is bound as
associated data; equal client-provided strings are not authority. Scope values
are redacted and not in token framing. Bind logical order, not page direction
or page size: the same edge can resume forward or backward. A wrong audience,
auth partition, relation, parent, order or schema cannot open the token.

For nested paging, validate encrypted input shape before the parent query;
derive parent-bound context only after an authorized parent anchor exists,
and open before the child-layer query. No client-supplied raw parent key is
trusted. A cursor scoped to one parent cannot resume another parent; host
queries/aliases must scope nested resume to that selected parent. Exact
parent-context verification cannot precede acquiring that trusted anchor, so
the guarantee is no child I/O on mismatch, not no preceding parent read.

Default protection bounds: internal plaintext at most 16 KiB and additionally
the underlying query/relation cursor bound, key ID at most 64 ASCII identifier
bytes, associated data at most 16 KiB, provider framing/tag overhead at most
512 bytes, and public token at most 32 KiB. Check lengths with checked
arithmetic before base64 allocation, and cap decoded ciphertext accordingly.
Host providers honor these supplied bounds. Lower host limits are supported;
overheads exceeding them fail closed. Aggregate response/request budgets
include all protected cursor bytes and crypto calls; no unbounded per-edge
task spawning. Callback errors/oversized outputs return safe stable codes.

Rotation seals new tokens with the active key ID and resumes old tokens only
while their decryption key remains in the host's bounded allowed ring and
host expiry/revocation rules allow it. Associated-data encoding is versioned
and stable across compatible key rotations. A retired/unknown key or expired
token returns safe `cursor_unavailable`; invalid framing, authentication,
context mismatch and tampering return indistinguishable `invalid_cursor`.
Do not include crypto errors or plaintext in public errors/tracing. An active
key change alone need not prevent resume; schema/order/audience changes do.
Request cancellation or provider outage does not downgrade protection.

Reapply current collection/field/relation/count authority and current row
predicates for **every** resume before executing its layer. A cursor grants
no access, and no old grants/predicates are restored from the token. Policy
changes can reduce rows even when crypto/schema scope still accepts the token;
hosts may deliberately change the trusted scope to revoke all old cursors.
Tests prove ciphertext hides distinctive hidden order and parent values,
bit flips and substituted scopes fail, raw inputs fail, all edge/page-info
positions are protected, and forward/backward/nested resume, key rotation,
retirement, bounded provider failures and policy changes obey these rules.
Use a real AEAD test provider and a compiled host-provider example; encode-only
or signing-only mocks cannot establish confidentiality.

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
    .cursor_protection(host_aead_provider, trusted_audience, cursor_limits)?
    .query_field(RuntimeHostField::new("health", TypeRef::named_nn("Boolean"),
        |_| FieldFuture::new(async { Ok(Some(FieldValue::value(true))) })))?
    .install(module)?.finish()?;
let request_context = RuntimeGraphqlRequest::<SqliteBackend>::new(
    schema.fingerprint(), read_authority, db_auth, request_budget,
).with_cursor_scope(trusted_auth_partition);
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
input CreateCustomerInput { id: RuntimeInt64!, status: String! }
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

The example integer id is explicitly non-generated: CreateCustomerInput must
include it. Required non-generated composite-key members are also required
create fields. A generated UUID variant would omit that generated key instead;
do not infer generation merely from primary-key status or an integer type.
Compile and execute create(input: {id: "7", status: "draft"}), the CAS update,
and exact-key delete examples on both backends, plus composite-key variants.

Compare expectedSchema and the request expectation against the module's
fingerprint before I/O. The host chooses/pins its active catalog revision and
maps that broader product identity to the fingerprint; the ORM cannot know
that a newer activation occurred, or enforce a product lease. Fingerprints do
not encode policy identity or authenticate clients. For Digibase's profile,
the host also validates the client's expected **complete public revision**,
including policy-only changes, before granting a mutation intent. Capture that
host revision expectation in request-scoped authority, compare it with current
host state inside the pinned transaction where the state is repository-backed,
and pin/fence the corresponding policy snapshot through commit. A policy-only
revision change can reject the mutation even when expectedSchema still matches.
ORM does not invent the host revision format, persistence or activation fence.
Document this host obligation and test a host authority rejecting an old
policy-only revision before DML, without conflating revision with fingerprint.

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
    .cursor_protection(host_aead_provider, trusted_audience, cursor_limits)?
    .mutation_root("Mutation")?.install(module)?.finish()?;
let request_context = RuntimeGraphqlRequest::<SqliteBackend>::new(
    schema.fingerprint(), read_authority, db_auth, request_budget,
).with_cursor_scope(trusted_auth_partition)
 .with_mutation_environment(environment)
 .with_write_authority(write_authority).with_mutation_hook(journal_hook);
// The adapter invokes the journal hook through run_runtime_before_commit;
// the hook uses tx.insert::<Journal>(...) as in B.
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
changes. The contract PR changes documentation and external compatibility fixtures only
and requires no version bump. Existing accepted ADRs remain immutable; add an ADR only if review adopts
a durable new boundary requiring one.

| PR | Required focused evidence |
| --- | --- |
| Fix | External dependency-minimal builds on three backends; Rust aggregate helpers; GraphQL SDL unchanged |
| A | External legacy imports/constructors/matches/literals/inference/function pointers; static/runtime physical equivalence; no-op replans both origin directions; defaults/keys/indexes/FKs; public rename no DDL; unsupported diagnostics; read-only plan/history; explicit ownership removal; destructive/additive/source-hash guards; rollback on failed apply; repeated schema conversion memory bounded; PostgreSQL RLS preservation; unrelated objects untouched; composed system-table incoming dependencies |
| B | Omission/null/default/generated inputs; required scalar/composite create keys; denied inputs/CAS/preimage/result/return fields; append-only; exact composite keys; two competing writers; stale CAS; authoritative transactional policy reads; journal/hook/commit rollback; canceled operation/hook caught inside a live callback then Ok must roll back on SQLite/PostgreSQL; swallowed-error poisoning; ambiguity; RLS cleanup; verified incoming dependencies; RESTRICT usable; safe errors |
| C | Exact SDL and symmetric scalar wire forms, i64 extrema/JS-unsafe values, both JSON null states/non-null JSON, byte bounds and datetime precision; AEAD hides every cursor and parent value; tampering/wrong scopes/raw inputs rejected; forward/backward/nested resume; rotation/retirement/policy changes; composition collision permutations; missing authority; tenant predicate before page/count; denied projection/filter/order/relation/count before layer SQL; fragments/aliases; stale handles/cursors; auth scope isolation; nested relation query counts; schema and operation budgets |
| D | B/C engine delegation; compiled/executed create/update/delete examples; required non-generated/composite create keys and keys absent from update; missing versus null/default; expected schema and host policy-only revision; CAS; guarded hook cancellation/journal rollback; independently denied return fields; committed payloads only; competing GraphQL writers; host mutation collision; ambiguous commit errors |

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
4. B: establish RuntimeMutationEnvironment from the verified composed physical
   target (including system tables), implement RuntimeWriteAuthority and static system repository policies;
   use state-machine transactions and tx.insert for journal work. Publish a
   success only after the runner returns successfully. Reconcile ambiguous
   commits using host operation IDs without blind retries.
5. C: compile/install a module through the checked composer, install a
   host-managed authenticated-encryption cursor provider and trusted audience/
   auth partition, and supply per-request
   read authority, row predicates, schema expectation, RLS context and budgets.
   Host constructs and publishes the active GraphQL schema/transport.
6. D: explicitly enable mutation registration, attach write authority and the
   before-commit journal hook, send expectedSchema and optional structural CAS,
   and authorize the selected return projection independently. Validate the
   complete expected host public revision (including policy-only changes) in
   host intent authority before DML; schema fingerprint checking is additional.
7. Run downstream SQLite/PostgreSQL adapter contracts. Upstream green checks
   establish mechanisms, not product readiness or durable event replay.

## Current checkpoint

Proposal prepared against the exact remote base; no functional APIs implemented.
The independent repository aggregate fix is approved for implementation in
its own PR. A–D remain pending review of this revised contract: preserved enum
namespaces with shared internal storage adapters, ownership/RLS/dependency
verification, guarded operation/hook cancellation, host-managed AEAD cursor
protection, symmetric lossless JSON-text scalars, required create keys, and
host policy-only revision validation. Other accepted boundaries, including
initial unsupported_cascade with RESTRICT supported, are retained. No A–D
interfaces are implemented by this proposal. Each approved functional PR must
provide its own compiled examples, isolated evidence and exact handoff.
