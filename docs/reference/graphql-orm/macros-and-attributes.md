---
title: GraphQL ORM macro and attribute reference
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-31
review_by: 2027-02-01
supersedes: []
---

# GraphQL ORM macro and attribute reference

This is the canonical syntax reference for the derives exported by
`graphql-orm`. It describes what the macro parser accepts in this revision;
the task-oriented guides explain when to use the options. `GraphQLEntity` and
`GraphQLOperations` produce a GraphQL surface. `RepositoryEntity` deliberately
does not.

## Derives and macros

| Item | Generates | Do not use it when |
| --- | --- | --- |
| `GraphQLEntity` | GraphQL object/input/filter/order types, row decoding, entity metadata, and query helpers | you need a repository-only entity |
| `GraphQLSchemaEntity` | schema/entity metadata for validation and planning | you need GraphQL or repository operations |
| `RepositoryEntity` | typed repository CRUD, filters, ordering, projections, and Rust write inputs | you need generated `async-graphql` types or resolvers |
| `GraphQLRelations` | relation resolver/loading implementations | the struct has no relation fields |
| `graphql_complex_object` | handwritten `ComplexObject` fields composed with generated relation fields | the entity does not opt into complex-object composition |
| `GraphQLOperations` | generated query, mutation, and subscription operation types plus discovery descriptors | the entity uses `#[repository_entity(...)]` |
| `schema_roots!` | `QueryRoot`, `MutationRoot`, `SubscriptionRoot`, `AppSchema`, schema builders, schema metadata helpers, and operation/semantic catalogs | no generated operation types participate in the schema |
| `graphql_orm_custom_operations` | canonical semantic metadata beside one handwritten `async-graphql` root impl | the impl is not actually composed into the finished schema |
| `GraphQLSemanticObject` | canonical fields, descriptions, classifications, export rules, and bounded object/list shape for a handwritten result object | the object is private or never returned by a semantic root |
| `mutation_result!` | a `SimpleObject` mutation result with `success: bool`, optional `message`, and optional typed field | a normal application GraphQL object is more appropriate |

All entity derives require a struct. `GraphQLEntity` requires
`#[graphql_entity(...)]`; `RepositoryEntity` requires
`#[repository_entity(...)]`. They are mutually exclusive. `GraphQLRelations`
and `GraphQLOperations` cannot be combined with `RepositoryEntity`.

## Entity surface: `graphql_entity` and `repository_entity`

Both attributes accept the following options. `table` and `plural` are the
normal minimum for `GraphQLEntity`; repository entities have no GraphQL plural
surface. In a build with more than one backend feature, `backend` is required.

| Option | Value | Default / constraint |
| --- | --- | --- |
| `table` | string | physical table name |
| `plural` | string | GraphQL plural name |
| `description` | nonempty string | at most 1,024 bytes; no control characters |
| `classification` | `"public"`, `"internal"`, `"confidential"`, `"restricted"`, or `"secret"` | `"internal"`; inherited by ordinary public fields |
| `backend` | `"sqlite"`, `"postgres"`, or `"mssql"` | inferred only when exactly one backend feature is enabled |
| `default_sort` | string | no declared default |
| `schema_policy` | `"managed"`, `"external_read_only"`, `"external_writable"`, `"validate_only"`, or `"plan_only"` | no declared policy |
| `auth` | `"required"`, `"optional"`, or `"none"` | schema-root mode, otherwise runtime compatibility default |
| `schema_only`, `append_only`, `repository_mutations`, `aggregate`, `backup` | boolean | `false`, except `backup` has no explicit declaration; `aggregate = true` opts into a bounded generated GraphQL aggregate root |
| `ai_mutations` | `(create = "…", upsert = "…", update = "…", update_many = "…", delete = "…", delete_many = "…")` | each value is `"automatic"`, `"approval_required"`, or `"prohibited"`; omitted categories are prohibited |
| `retention_purge` | nonempty policy-key string | absent |
| `keyset`, `read_policy`, `write_policy`, `notify`, `notify_with` | string | absent |
| `backup_export_order`, `backup_restore_order` | integer | absent |
| `upsert` | comma-separated columns | at least one column; may appear once |
| `unique_composite` | comma-separated columns | at least two columns; repeatable |
| `index`, `unique_index` | `"a,b"` or `(name = "…", columns = ["…"], directions = ["asc" | "desc"])` | at least one column; direction count must match columns |

`schema_policy = "external_read_only"` suppresses generated mutations and
subscriptions on every backend. MSSQL requires `external_read_only` or
`external_writable`; the latter generates only capabilities implemented by the
external-schema DML contract. Schema policy controls schema ownership, not
application authorization. See [schema management](schema-management.md) and
[SQL Server](mssql.md).

### Entity-level `graphql_orm` options

The entity-level namespace is separate from `graphql_entity` so it can carry
schema/search metadata.

| Option | Accepted shape | Defaults and limits |
| --- | --- | --- |
| `search` | `(index = bool, language = "…", tokenizer = "…", min_token_len = integer, fallback = "enabled" | "disabled")` | defaults are supplied by the runtime; only these keys are accepted |
| `order_expression` | `(name = "GraphQLField", expression = "trusted SQL expression", parameters = "server_function_path")` | repeatable; `parameters` is required only when the expression contains `:named` binds; the client supplies only `ASC` or `DESC`; raw placeholders, comments, and statement separators are rejected |
| `compose_complex_object` | marker | use with `GraphQLRelations`, `#[graphql(complex)]`, and `#[graphql_complex_object]` on the handwritten inherent impl |
| `conditional_index` | `(name = "…", columns = ["…"], unique = bool, predicate_field = "…", predicate_values = ["…"])` | `columns`, `predicate_field`, and nonempty `predicate_values` are required |
| `projection` | `(name = "TypeName", fields = [field, …], private = true)` | all three facts are required; public projections are rejected |
| `operation_authorization` | described below | consumed by `GraphQLOperations` |

`#[graphql_rls(...)]` is the entity RLS declaration. Its detailed policy
semantics belong to [strict authorization](strict-authorization.md); it is
metadata for database policy generation, not a substitute for resolver and
row checks.

## Fields

The following attributes may be placed on fields. Unless an option says
otherwise, it is additive metadata; visibility and write capabilities are
still subject to entity policy and backend/schema-policy restrictions.

| Attribute | Accepted values and effect |
| --- | --- |
| `#[primary_key]` | marks a key field; generated single-record operations require a complete key |
| `#[filterable]` / `#[filterable(type = "…")]` | enables a filter; bare form selects `"string"` metadata |
| `#[sortable]`, `#[unique]` | enables ordering / declares a unique field |
| `#[db_column = "…"]` or `#[graphql_orm(db_column = "…")]` | chooses the physical column name |
| `#[graphql(name = "…")]` | chooses GraphQL field name; `#[serde(rename = "…")]` records the serialization name |
| `#[graphql(skip)]`, `#[graphql_orm(skip_input)]` | omits the field from generated inputs; `graphql(skip)` also supports relation implementation fields |
| `#[skip_db]` | excludes a field from persisted row decoding |
| `#[input_only]` | keeps a writable field in inputs even where it is skipped from GraphQL reads |
| `#[graphql_orm(private)]` | removes generated read/filter/order/subscription exposure and inputs |
| `#[graphql_orm(sensitive)]` | emits `Secret` plus structural `NeverExport`; it does not by itself remove the field from ordinary GraphQL |
| `#[graphql_orm(classification = "…")]` | overrides the inherited public/internal/confidential/restricted/secret classification |
| `#[graphql_orm(non_exportable)]` | structurally excludes the field from external-provider result projections while retaining ordinary GraphQL visibility |
| `#[graphql_orm(description = "…")]` | nonempty semantic description, maximum 1,024 bytes |
| `#[graphql_orm(read = bool, write = bool, filter = bool, order = bool, subscribe = bool)]` | per-surface capability switches; `write = false` also skips input |
| `#[graphql_orm(read_policy = "…", write_policy = "…")]` | policy identifiers for runtime enforcement |
| `#[graphql_orm(version)]` | optimistic-version field; is not writable and is omitted from inputs |
| `#[graphql_orm(default = "SQL expression" | false)]` | explicit SQL default or disables the implicit default; one declaration only |
| `#[graphql_orm(decimal(precision = P, scale = S))]` | required for `rust_decimal::Decimal`; portable precision is 1 through 18 and scale must not exceed precision |
| `#[graphql_orm(auto_generated = bool)]` | declares whether the value is database-generated |
| `#[date_field]`, `#[boolean_field]`, `#[json_field]`, `#[graphql_orm(json)]` | type metadata; JSON disables filter/order generation |
| `#[transform(write = "…", read = "…")]` | names generated write/read transforms |
| `#[backup(include | exclude | redact)]` | per-field backup treatment |

Validation metadata accepted inside `graphql_orm` is `min`, `max`,
`min_exclusive`, `max_exclusive` (numeric), `non_negative`, `min_length`,
`max_length`, `one_of = ["…"]`, and `gte_field`, `gt_field`, `lte_field`,
`lt_field` (field-name strings).

### Generated date-filter contract

`#[filterable(type = "date")]` generates exact `Eq`, `Ne`, `Lt`, `Lte`, `Gt`,
and `Gte` value comparisons plus structured date predicates. Exact values are
bound unchanged. Calendar predicates are sargable half-open ranges and never
wrap the persisted column in a cast or date function:

| Predicate | Calendar range |
| --- | --- |
| `IsToday` | `[today, tomorrow)` |
| `InPast` | before today |
| `InFuture` | at or after tomorrow |
| `RecentDays(N)` | `[today - (N - 1 days), tomorrow)` |
| `WithinDays(N)` | `[today, today + N days)` |
| `GteRelative(days: d)` | at or after the start of today plus `d` days |
| `LteRelative(days: d)` | before the start of today plus `d + 1` days |

`DateRangeInput.start` and `.end` are required and inclusive. Both must be
parseable date/timestamp values and the range cannot be reversed.
`RecentDays` and `WithinDays` accept 1 through 36,600. Relative offsets accept
-36,600 through 36,600. Generated filters validate recursively before database
work, including filters constructed directly in Rust; invalid direct SQL
rendering produces a false predicate and execution returns `INVALID_INPUT`.

PostgreSQL obtains today from session `CURRENT_DATE`, SQL Server from the
server-local date of `GETDATE()`, and SQLite from UTC `date('now')`. SQLite
spatial fallback uses one UTC anchor for its complete in-memory boolean tree
and preserves SQL NULL/unknown behavior. The runtime does not normalize exact
comparison strings or choose an application timezone.

Entity-level expression ordering is intended for computed fields whose SQL is
owned by the server declaration rather than accepted from a GraphQL request:

```rust,ignore
#[graphql_orm(order_expression(
    name = "Duration",
    expression = "COALESCE(finished_at, :as_of) - started_at",
    parameters = "duration_order_parameters"
))]
```

This adds `Duration: OrderDirection` to the generated order input and lowers it
to a parameterized `(COALESCE(finished_at, ?) - started_at) ASC|DESC`. The
named bind is resolved by an entity-owned synchronous function:

```rust,ignore
fn duration_order_parameters(
    ctx: &async_graphql::Context<'_>,
) -> async_graphql::Result<OrderExpressionParameters> {
    let as_of = ctx.data::<RequestClock>()?.unix_seconds;
    Ok(OrderExpressionParameters::new().bind("as_of", SqlValue::Int(as_of)))
}
```

Expressions are backend-specific and trusted like `default_sort`; the public
input never accepts an expression, identifier, fragment, bind value, or other
client-provided SQL. The macro rewrites only declared `:name` tokens outside
SQL literals and identifiers, rejects raw backend placeholders, and fails the
request if the provider omits a declared value. Generated GraphQL resolvers
resolve providers from their server context. Programmatic callers with such an
order use `EntityQuery::order_by_with_context`; parameter-free orders retain
`order_by`. Entity and relation pagination append any missing primary-key
columns as ascending tie-breakers so equal computed values have stable
limit/offset windows.

### Relations and foreign keys

Use a relation field plus `GraphQLRelations`:

```rust,ignore
#[graphql(skip)]
#[relation(
    target = "Account",
    from = "account_id",
    to = "id",
    on_delete = "cascade",
    emit_fk = true
)]
pub account: Option<Account>;
```

`target`, `from`, and `to` are relation metadata. `from` and `to` take either
one string or an array of string literals for a composite key. `multiple`
changes the relation cardinality. `emit_fk` is a boolean; `on_delete` and
`propagate_change` are strings validated for the selected backend/policy.
Relations are not ordinary persisted fields.

An unconditional readable relation can add a correlated count to its parent
entity's generated order input:

```rust,ignore
#[relation(
    target = "StaffAssignment",
    from = "id",
    to = "policy_id",
    multiple,
    order_aggregate(name = "AssignedStaffCount", aggregate = "count")
)]
pub staff_assignments: Vec<StaffAssignment>;
```

`name` is the GraphQL order-input field and `aggregate` currently accepts only
`"count"`. The request supplies only `OrderDirection`; the macro obtains the
target table through `DatabaseEntity` and constructs the correlated aggregate
from `from`/`to`. The relation must remain public and readable. Combining
`order_aggregate` with `source_condition` or `target_condition` is rejected so
conditional predicates continue to use their bound-value resolver path.

When an entity already has handwritten complex fields, opt into one composed
`ComplexObject` implementation instead of applying async-graphql's attribute
directly:

```rust,ignore
#[derive(GraphQLEntity, GraphQLRelations, SimpleObject, Clone)]
#[graphql(complex)]
#[graphql_orm(compose_complex_object)]
struct Job {
    // persisted and relation fields
}

#[graphql_complex_object]
impl Job {
    async fn duration(&self) -> i64 {
        self.finished_at - self.started_at
    }
}
```

`graphql_complex_object` delegates schema and resolution for generated
relations through an internal flattened object, so handwritten fields and
batched relation resolvers share the one `async_graphql::ComplexObject` trait
implementation required by `SimpleObject`.

Polymorphic references can add one fixed discriminator condition:

```rust,ignore
#[graphql(skip)]
#[relation(
    target = "Document",
    from = "reference_id",
    to = "id",
    source_condition(field = "reference_kind", equals = 1),
    emit_fk = false
)]
pub document: Option<Document>;
```

The reverse collection uses
`target_condition(column = "reference_kind", equals = 1)`. Condition values
accept string, integer, float, and boolean literals. `source_condition` names a
persisted scalar Rust field and must match its type; `target_condition` names a
physical target column. Either condition requires `emit_fk = false`. Fixed
conditions are always bound parameters and apply to every resolver and
batch-preload path.

A generated to-many relationship exposes nullable `Where`, `OrderBy`, and
`Page` objects. In particular, its ordering contract is one nullable
`ChildOrderByInput`, because one relation-loader query accepts one composed
ordering object. A generated root list query remains different: it accepts a
nullable list of non-null order objects, `[ChildOrderByInput!]`. Resolver SDL
and `graphql_orm_semantic_catalog()` derive these relationship arguments from
one macro-owned contract; consumers must not normalize or rewrite either
shape.

### Search, spatial, and projections

Entity search is enabled by `#[graphql_orm(search(...))]`; fields can then use
these options:

| Field option | Values / defaults |
| --- | --- |
| `searchable` | `(weight = "A" | "B" | "C" | "D", alias = "…", policy = "…")`; weight defaults to `D` |
| `search_json` | `(path = "$.field", weight = "A" | "B" | "C" | "D", policy = "…")`; path is required and supports field segments and `[*]` |
| `search_relation` | `(fields = ["…"], weight = "A" | "B" | "C" | "D", max_items = integer, policy = "…", propagate_change = "up")`; fields are required and `max_items` defaults to 100 |
| `spatial` | `(kind = "geometry", geometry_type = "Geometry" | "Point" | "LineString" | "Polygon" | "MultiPoint" | "MultiLineString" | "MultiPolygon" | "GeometryCollection", srid = integer, index = bool, index_method = "gist")`; defaults: geometry, `Geometry`, 4326, false, GiST |

Private fields cannot be searchable. A protected searchable field needs an
explicit `policy`. Spatial fields are JSON-shaped and cannot be ordered. The
[backend guide](backends.md#spatial-support) explains the PostgreSQL/SQLite
execution difference and the MSSQL compile-time limitation.

## Generated-operation authorization

Attach this to an entity that derives `GraphQLOperations`:

```rust,ignore
#[graphql_orm(operation_authorization(
    categories = ["single_read", "update"],
    all_scope_templates = ["records.{id}.read"]
))]
```

`categories` is required and contains generated categories: `list`,
`single_read`, `search`, `keyset_list`, `create`, `upsert`, `update`,
`update_many`, `delete`, `delete_many`, or `subscription`. Declare exactly one
of `all_scopes`, `any_scopes`, `all_scope_templates`, or
`any_scope_templates`. `all_*` is a nonempty string array; `any_*` is a
nonempty array of nonempty string arrays. Fixed scopes cannot contain
whitespace, control characters, or braces. Templates may use only balanced
`{argument}` placeholders naming supported non-null scalar root arguments.

This creates a generated resolver guard and operation-catalog metadata. It
does not authorize custom resolvers, replace field/row/database policy, or
make an arbitrary scope template safe.

## AI mutation execution classification

Mutation execution is prohibited by default. An entity may classify only the
generated categories it deliberately exposes:

```rust,ignore
#[graphql_entity(
    table = "reviewed_tasks",
    plural = "ReviewedTasks",
    ai_mutations(
        create = "automatic",
        update = "approval_required",
        delete = "prohibited"
    )
)]
struct ReviewedTask { /* public fields */ }
```

`automatic` is reserved for bounded low-consequence work under the current
user's ordinary resolver authority. `approval_required` prepares one exact
server-authored request for expiring one-shot human approval. `prohibited` is
absent from executable AI capabilities. The declaration changes only canonical
semantic metadata; it does not enable an AI runtime, satisfy tool/target policy,
or weaken resolver, field, row, tenant, assurance, or database authorization.

Handwritten mutations use the existing resolver macro rather than a separate
AI catalogue:

```rust,ignore
#[graphql_orm_custom_operations(
    kind = "mutation",
    authorization = true,
    ai_execution = "approval_required"
)]
#[async_graphql::Object]
impl ApplicationMutations {
    /// Applies one reviewed bounded change.
    async fn apply_change(&self, input: ApplyChangeInput) -> ApplyChangeResult {
        // Ordinary application authorization remains authoritative.
    }
}
```

`ai_execution` is accepted only for mutation roots and defaults to
`prohibited` when omitted.

## `schema_roots!`

```rust,ignore
schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    auth: "required",
    generated_mutations: "allowlist",
    generated_mutation_allowlist: [Account],
    query_custom_ops: [],
    described_query_types: [ApplicationQueries],
    described_mutation_types: [ApplicationMutations],
    described_subscription_types: [ApplicationEvents],
    entities: [Account],
}
```

`entities` is required. The optional `backend`, `schema_policy`, and `auth`
take the same values as the entity options. `query_custom_ops`,
`described_query_types`, `described_mutation_types`, and
`described_subscription_types` are the single-source form for handwritten
roots annotated by `#[graphql_orm_custom_operations]`: each entry composes the
root and automatically includes its operation metadata and direct handwritten
result objects deriving `GraphQLSemanticObject`. The legacy
`extra_query_types`, `extra_mutation_types`, `extra_subscription_types`,
`semantic_custom_operations`, and `semantic_types` lists remain available for
incremental compatibility. A root in a `described_*` list cannot be repeated
in a legacy composition/semantic list.
`generated_mutations` defaults to `"all"`; it accepts
`"all"`, `"none"`, `"allowlist"`, or `"denylist"`. An allowlist/denylist
requires its matching nonempty list, and every listed entity must be in
`entities`. External-read-only roots use empty mutation and subscription types
regardless of requested exposure. External-writable MSSQL roots may compose
generated DML and in-process subscriptions, but never schema-management roots.

`schema_roots!` also emits `graphql_orm_semantic_catalog()`. The strict,
versioned value contains public API names, safe descriptions, typed field and
relationship shape, generated/custom root coordinates, and canonical
fingerprints. It contains no table/column names, Rust paths, policy keys, or
credentials and grants no authority. The optional `router-protocol` feature
can wrap it in a generic descriptor extension.

## Handwritten root semantic metadata

Place `#[graphql_orm_custom_operations]` before the corresponding
`async-graphql` `Object`, `Mutation`, or `Subscription` attribute:

```rust,ignore
#[graphql_orm_custom_operations(kind = "query", authorization = true)]
#[async_graphql::Object]
impl ApplicationQueries {
    /// Returns one bounded public status value.
    #[graphql(name = "ApplicationStatus")]
    async fn status(&self, #[graphql(desc = "Maximum records")] limit: i32) -> String {
        todo!()
    }

    /// Returns a bounded exportable set of public status codes.
    #[graphql_orm(
        result_classification = "public",
        result_export = "exportable",
        result_maximum_items = 10
    )]
    async fn status_codes(&self) -> Vec<i32> {
        todo!()
    }
}
```

`kind` is required. `authorization` defaults to `true` and records only that an
authoritative resolver policy exists; it does not describe or execute that
policy. Resolver and argument names honor explicit `#[graphql(name = "…")]`
attributes, then `async-graphql`'s default camel-case convention. Descriptions
use the same explicit/doc/fallback rules and are emitted into SDL. Unknown attributes,
unbounded descriptions, malformed GraphQL types, duplicate root coordinates,
and stale fingerprints fail closed.

Custom scalar and enum result leaves carry an explicit project-neutral
`result_disclosure`. Without a declaration they default to
`Secret`/`NeverExport` and are unavailable to generated AI capabilities while
remaining ordinary GraphQL roots. `result_classification` and `result_export`
must be declared together; `secret` cannot be exportable. Exportable scalar or
enum lists additionally require positive `result_maximum_items`. Use
`result_type_kind = "enum"` (or `"scalar"`) for a custom wrapper the macro
cannot classify from its Rust type. These facts are fingerprinted descriptive
metadata and grant no resolver or provider authority. Object results continue
to use selected `GraphQLSemanticObject` fields; any operation-level declaration
may only make the root stricter.

Handwritten result objects use the sibling derive. Public list fields require
an explicit positive maximum; sensitive fields are always secret and
non-exportable:

```rust,ignore
/// Current application status.
#[derive(async_graphql::SimpleObject, GraphQLSemanticObject)]
#[graphql_orm(classification = "internal")]
struct ApplicationStatus {
    /// Bounded status entries.
    #[graphql_orm(maximum_items = 10)]
    entries: Vec<String>,
    #[graphql_orm(sensitive)]
    provider_credential: String,
}
```

Object and field `description`/`classification` use the same bounded values as
entities. `non_exportable`, `sensitive`, `maximum_items`, and an exceptional
`type_kind = "scalar" | "enum" | "object"` override are accepted on fields.
Explicit `#[graphql(name = "…")]` and `rename_fields` match the SDL; otherwise
the ordinary `async-graphql` camel-case default is used.

For `GraphQLSemanticObject`, put descriptions in Rust documentation (preferred)
or `#[graphql(desc = "…")]`; those declarations are read by both
`SimpleObject` and the semantic derive. A separate `graphql_orm(description)`
is rejected here so SDL and semantic metadata cannot drift. A field hidden by
a handwritten `SimpleObject` must use `#[graphql(skip)]`; semantic-private
fields must likewise be absent from that finished GraphQL object.

Custom subscription roots may additionally declare truthful bounded
observation metadata:

```rust,ignore
#[graphql_orm_custom_operations(
    kind = "subscription",
    authorization = true,
    observation = "replay_then_live",
    maximum_duration_seconds = 300,
    maximum_events = 100
)]
#[async_graphql::Subscription]
impl ApplicationEvents {
    /// Observes bounded application events.
    async fn application_events(&self) -> impl futures::Stream<Item = ApplicationEvent> {
        // ...
    }
}
```

This metadata does not make delivery durable. A later waiter/runtime must bind
an authoritative source that implements the declared opaque cursor, watermark,
replay, and reset contract. Generated broadcast subscriptions are explicitly
`BestEffort` and have no bounded-wait registration limits.

## Naming features

Feature groups alter generated names independently of the selected database.
Enable at most one in each group:

| Group | Features |
| --- | --- |
| resolver names | `resolver-case-pascal`, `resolver-case-snake`, `resolver-case-screaming-snake`, `resolver-case-lower`, `resolver-case-upper` |
| argument names | `argument-case-pascal`, `argument-case-snake`, `argument-case-screaming-snake`, `argument-case-lower`, `argument-case-upper` |
| field names | `field-case-pascal`, `field-case-snake`, `field-case-screaming-snake`, `field-case-lower`, `field-case-upper` |

## Further reading

- [Core ORM reference index](README.md)
- [Entities and relations](entities-and-relations.md)
- [Runtime writes and repository operations](runtime-and-writes.md)
- [Schema management](schema-management.md)
- [Strict authorization](strict-authorization.md)
- [Macro crate README](../../../crates/graphql-orm-macros/README.md)
