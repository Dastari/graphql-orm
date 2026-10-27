---
title: Runtime GraphQL reads and checked composition
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-25
review_by: 2027-02-01
supersedes: []
---

# Runtime GraphQL reads and checked composition

The additive `runtime-graphql` feature registers read-only dynamic GraphQL over
an `Arc<ValidatedRuntimeSchema>`. It is off by default. Static derives, SDL,
framework-neutral cursors and repository consumers are unchanged. No data
migration is required. This is C, based on the reviewed B revision; typed mutation registration is explicitly opt-in.

`RuntimeGraphqlModule::compile` checks public names and deterministic ordering.
`RuntimeGraphqlComposer<B>` checks every root and type name before publication.
Submit host dynamic objects, interfaces, unions, enums, inputs and scalars through
`register`, and root fields through `RuntimeHostField::new`. The composer creates
fresh roots; it cannot safely inventory or extend an arbitrary existing dynamic
SchemaBuilder. Static schemas can coexist in the host; this API does not merge
opaque static Schema objects. Host data, extensions, introspection suppression,
depth/complexity limits and host-owned subscription roots are supported. There
are no ORM subscriptions or transport services. Host-owned resolvers remain
responsible for their own I/O and authorization. Generated runtime object payloads
are private to the authorized executor: arbitrary host/preloaded Values cannot be
used to bypass runtime authorization. Use separately registered host types for
host-owned payloads.

Names use ASCII camelCase fields/roots and PascalCase types, without underscores.
The default list root lowercases only the first ASCII letter of the collection's
plural API name. Options permit a type prefix and root overrides keyed by stable
collection IDs. Names are never silently rewritten otherwise. Filterable fields
named `and`, `or` or `not` collide with recursive filter operators and are rejected.
Helper types are shared only within this composer; same-name host types collide.

The locked async-graphql 7.2.1 dynamic builder has no custom-directive factory
registration API. The approved C contract therefore explicitly defers custom
directives: `composer.directive(factory)` returns structured
`unsupported_capability` during composition, before schema publication. It never
ignores registration or invokes the factory. No dependency fork or emulation is
used. Built-in `@skip` and `@include` remain supported, including variable inputs;
`limit_directives` bounds directives per field and cannot relax the installed
selection budget. Host extensions and checked subscription roots remain supported,
with collision checks and mandatory authorization/resource guards intact.

## Host integration

The compiled, executable [host example](../../../crates/graphql-orm/examples/runtime_graphql_reads.rs)
uses typed repository setup, checked host composition, an explicit public read
authority and a real host-owned ChaCha20-Poly1305 provider. Its key is ephemeral;
restarting that example intentionally invalidates its cursors. Production hosts
own durable key management, rotation, nonce uniqueness, expiry and revocation.

```rust,ignore
let module = RuntimeGraphqlModule::compile(schema.clone(), Default::default())?;
let api = RuntimeGraphqlComposer::new(database, "Query", limits)?
    .cursor_protection(host_aead, trusted_audience, protection_limits)?
    .query_field(RuntimeHostField::new("health", TypeRef::named_nn("Boolean"),
        |_| FieldFuture::new(async { Ok(Some(FieldValue::value(true))) })))?
    .install(module)?
    .finish()?;
let request = Request::new("{ notes(first:20) { edges { node { label } } totalCount } }")
    .data(RuntimeGraphqlRequest::<SqliteBackend>::new(
        schema.fingerprint(), authority, db_auth, request_budget,
    ).with_cursor_scope(trusted_partition));
let response = api.execute(request).await;
```

Install protection before a protected module. The default profile requires it.
Options are non-exhaustive so future D configuration can remain source-compatible;
construct them with `Default` and explicit setters/field updates.
`RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected)` is an explicit alternative for hosts accepting
readable existing cursors; it must not be used for confidential hidden keys.

## Authorization, pagination and batching

Each request must provide `RuntimeReadAuthority`. Its borrowed check contains the
schema, collection, selected public fields, typed filter, complete effective order
(including default key tie-breakers), relation traversal and count intent. The host
must grant all those capabilities, even with PostgreSQL RLS. `RuntimeReadGrant::new`
returns a validated structural row predicate, or an explicit unrestricted grant.
No provider, denial, invalid grant or stale schema fingerprint fails closed.

A mandatory per-execution extension expands aliases/fragments and evaluated
`@skip`/`@include`, resolves variables/defaults and obtains every selected runtime
layer's grant before the first ORM statement. Row predicates are ANDed with client
filters before reads, pagination and counts. Hidden key acquisition is internal to
the granted operation and does not expose those keys as loaded output fields.
Filter/order authorization remains independent of projection authorization.
The check distinguishes client-requested order terms (including explicit empty
ordering) from the effective order's internal keys. It also exposes scalar/list
and logical predicate operators; granting `eq` does not hide an enclosing `not`
from the host's policy check.

Filters are recursive `and`/`or`/`not` inputs with supported per-kind operators.
SQL NULL uses `isNull`; omitted/null optional filter operands add no condition.
JSON exposes only SQL-null testing, not structural JSON equality. Ordering uses
closed field enums, ASC/DESC and FIRST/LAST null placement. Omitted ordering uses
the existing deterministic runtime default. Page sizes and cursor shapes are
validated, not clamped. To-one targets missing or removed by the target predicate
return null. Collection/field/traversal denial during preflight rejects the
operation before parent reads; it is not treated as a missing target.

Counts execute only when selected, including through aliases/fragments. Each
compatible relation layer calls the existing bounded relation batch executor;
two parents do not mean two child queries. Different selection shapes consume
separate bounded layers. There is no process-wide cache, detached task or authority
identity inferred from a client principal string. Reusing request data does not
reuse previous execution state or grants. Optional DbAuthContext propagates to
every layer and its count.

Read/count pairs retain the backend executor's isolation semantics. This does not
provide a frozen snapshot across pages, aliases or nested layers: concurrent
writes can move records between pages or alter counts. Current authorization is
reapplied on every resume. Hosts configure database statement timeouts; a bounded
number of count statements is not a bound on scanned database rows.

Response aliases are distinct from schema field identities, including scalar/relation
alias overlaps and connection helpers. Response-size accounting follows the same
identities as resolution. Field/operator authorization uses validated requested
capabilities captured before constant lowering: empty `in: []` is false and empty
`notIn: []` is true, but neither bypasses filter-field or operator authorization.
Explicit nested `and`, `or` and `not` capabilities remain visible to the host.
Use `RuntimeReadCheck::filter_operations` for requested filter authorization;
the lowered predicate's referenced fields alone cannot represent constant inputs.

### Relation filters

`RuntimeGraphqlOptions::with_relation_filters()` adds every relation to its
collection's `…WhereInput`; `with_relation_filters_for([relation ids])` admits only the
listed relations. Compilation rejects unknown IDs. A relation that is not admitted has no
entry and no filter input type. `RuntimeGraphqlModule::relation_filter_quantifiers(id)`
reports each relation's accepted quantifiers, which is empty when it is not admitted. A to-one relation takes `{TargetToOneFilter: is,
isNot}` and a to-many relation takes `{TargetToManyFilter: some, none, every}`.
Each value is the target's own `WhereInput`, so filters nest and compose with
scalar, `and`, `or` and `not` entries. The option is off by default and the SDL
is unchanged without it. Each occurrence lowers to a
[relation predicate](runtime-queries.md#relation-predicates).

Every relation predicate is one correlated subquery and one target-layer authorization.
An operation may contain at most `RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTERS` (16) of
them, nested at most `RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTER_DEPTH` (3) relation hops
deep. Change both with `with_relation_filter_limits(max_filters, max_depth)`. Exceeding
either fails with `cost_exceeded` before any I/O. The general predicate node and depth
budgets still apply.

`RuntimeReadCheck::relation_filters` lists the relation predicates a layer uses
directly. Their correlation keys (the source fields of each key pair) also appear in
`filter_operations` with the operation name `relation`. A host that authorizes field use
through `filter_operations`, and that does not recognise `relation`, therefore denies a
predicate over a key field the caller may not read. Every occurrence is then authorized as its own target layer:
`purpose: RuntimeReadPurpose::RelationFilter(quantifier)`, `relation` set, an
empty projection, and the client's target filter with its `filter_operations`.
That grant's row predicate is applied inside the subquery. A target denied with
`RuntimeGraphqlError::denied()` makes every related row non-matching rather than
failing the request, so a filter is never an error oracle. Other errors fail the
request. All grants are obtained before the first ORM statement, and the
predicate is rebuilt with them. Until a grant is bound, the planned predicate
uses a deny-all placeholder. Mutation `expected` conditions reject relation
entries. Ordinary `RuntimeReadCheck`s carry `purpose: RuntimeReadPurpose::Select`.

## Confidential cursors

Protected output is `gormgqlc1.<public-key-id>.<base64url-ciphertext>`. The entire
internal gormrq1/gormrr1 envelope must be authenticated and encrypted by the host;
signing readable contents is insufficient. The ORM owns framing, validates bounds,
and never falls back to plaintext. Every edge and matching page-info cursor uses
the same protected token. Missing page-info cursors remain null.

Associated data is versioned, length-delimited and constructed privately from
trusted audience, authorization partition, schema fingerprint, collection and
complete logical order. A relation adds its source/target identity and exact typed
parent identity from an opaque authorized anchor. Page direction and size are not
bound, so an edge can resume forwards or backwards. Parent context is obtained
after an authorized parent read; wrong-parent tokens fail before child I/O.

Default limits are 16 KiB plaintext, 64 ASCII key-ID bytes, 16 KiB associated data,
512 bytes provider overhead and 32 KiB public token. Provider outputs and incoming
frames are bounded before decoding/allocation. Request budgets cap crypto calls
and output bytes. Providers must also honor their supplied limits.

New cursors use the host's active key; old cursors resume only while their key is
in the bounded allowed ring and host expiry/revocation permits it. Retired keys
and provider unavailability yield `cursor_unavailable`; framing, tampering and
context/authentication failures yield indistinguishable `invalid_cursor`. Errors
carry no ciphertext, plaintext, crypto source error or caller values. Tokens grant
no access. Hosts may include a policy revision in their trusted partition to revoke
old cursors; otherwise current row policies still apply to resumed pages.

## Descriptions and the semantic catalogue

Every generated type, field, relation, enum value, input member and root carries a
description, so SDL and introspection are complete. Collection, field and relation text is
the IR's [effective description](runtime-schema-ir.md#descriptions-classification-and-export)
(declared, else the humanized name); filter, order and mutation input members repeat their
field's text. Roots use the static derive wording (`Page through Invoice records`,
`Create one Invoice records`) and arguments the static rule (`orderBy` → `Order by`). Host
root objects (`Query`, `Mutation`) are host-named and undescribed.

```rust,ignore
let (schema, catalog) = RuntimeGraphqlComposer::new(database, "Query", limits)?
    .mutation_root("Mutation")?
    .install(module)?
    .query_field(host_root.semantic(host_descriptor)?)?
    .finish_with_semantic_catalog()?;
// The catalogue compiles as AI capabilities with no separate input.
let queries = AiGraphqlQueryCapabilityCatalog::compile(id, target, &schema.sdl(), &catalog, limits)?;
```

`finish_with_semantic_catalog` returns a `GraphqlSemanticCatalog` containing, per installed
collection, one entity named by its GraphQL object (`RuntimeGraphqlModule::semantic_entities`)
and its generated roots (`semantic_operations`):

- scalar fields with their effective description, classification and export, the exact
  runtime filter operators (`notIn` has no catalogue operator and is not advertised),
  `sortable`, and no grouping or aggregates (runtime GraphQL has none);
- to-one relations as relationships; to-many relations are connection-typed and
  projector-served fields return host objects, so neither is advertised;
- one `KeysetList` query per collection (`first`/`after`/`last`/`before` arguments are kept)
  and the enabled create/update/delete mutations, which are
  `AiMutationExecutionPolicy::Prohibited`.

Host roots must be described: `RuntimeHostField::semantic(descriptor)` (field and argument
names must match; its description and argument descriptions become the SDL text),
`subscription_semantic` for a host subscription root, and `semantic_entity` for host result
objects. Otherwise `finish_with_semantic_catalog` fails with `semantic_descriptor_missing`;
`finish()` still builds the schema alone. The catalogue binds no authority: runtime read and
write authorities remain authoritative.

## Exact scalar wire forms

| Kind | GraphQL wire representation |
| --- | --- |
| Integer and count | `RuntimeInt64`: canonical signed decimal string; full i64 range, no plus/leading zeros/negative zero/numeric coercion |
| JSON | `RuntimeJson`: bounded valid JSON text on input and output; SQL NULL is GraphQL null; JSON null is non-null string `"null"` |
| UUID | `RuntimeUuid`: valid UUID string input, lowercase hyphenated output |
| Bytes | `RuntimeBytes`: canonical padded RFC4648 base64, bounded decoded bytes |
| DateTime | `RuntimeDateTime`: RFC3339 input, UTC `YYYY-MM-DDTHH:MM:SS.ffffffZ` output; existing microsecond rounding |
| Float | finite GraphQL Float, existing RuntimeFloat normalization |
| Boolean/String | ordinary GraphQL Boolean/String, no string normalization |
| Decimal | `Decimal` (the static decimal scalar name): strict decimal string or integer input for the field's definition, float numbers rejected; output the canonical string with exactly `scale` fractional digits. Registered with `RuntimeDecimalFilter` only when a composition has a decimal field |

JSON whitespace/key spelling need not round-trip; the supported serde_json::Value
must. Integer JSON tokens must fit i64 (negative) or u64 (nonnegative), and
fractional/exponent tokens must fit finite f64. Integer overflow is rejected,
rather than silently falling back to a rounded floating-point value. JSON strings include their JSON quotes in the scalar text. Non-null JSON
fields can contain JSON null. Unloaded fields are errors, never SQL NULL.

TypeScript scalar mappings are strings for Int64/JSON/UUID/Bytes/DateTime. Use
BigInt for integers, exact JSON parsing where number fidelity matters, Uint8Array
for decoded bytes and a precision-preserving datetime parser rather than Date.
Rust wire DTOs use string newtypes with fallible conversions. SDK generation is
host-owned; static GraphQL scalars are unchanged.

## Bounds and verification

`RuntimeGraphqlLimits` bounds types/names/schema bytes, operation/variable input,
recursive selections, predicates, pages/projections, relation parents, statement
work, materialized nodes, output bytes and crypto calls. Request budgets may only
lower installed limits. Variable trees are walked iteratively; query nesting is
bounded before parsing. Output admission conservatively accounts for JSON escaping
and repeated aliased selections. Hosts still bound HTTP allocation before building
an already-owned GraphQL Request.

`descriptor().cost_metadata()` describes one default root page including count.
Hosts can use `RuntimeGraphqlCost::for_root_page`, `for_relation_layer` and
`checked_add` for overflow-checked admission estimates. Execution computes its own
cost from the selected operation; caller estimates never bypass those checks.

Run backend lanes sequentially with `CARGO_BUILD_JOBS=2`:

```sh
cargo test -p graphql-orm --no-default-features --features sqlite,runtime-graphql --lib --test runtime_graphql -- --test-threads=1
cargo test -p graphql-orm --no-default-features --features postgres,runtime-graphql --lib --test runtime_graphql -- --test-threads=1
cargo run -p graphql-orm --no-default-features --features sqlite,runtime-graphql --example runtime_graphql_reads
cargo run -p graphql-orm --no-default-features --features postgres,runtime-graphql --example runtime_graphql_reads
```

SQLite tests own in-memory databases; PostgreSQL tests/examples create labelled
Docker containers and verify exact ownership before cleanup. MSSQL remains an
unsupported runtime decoder and is rejected before ORM I/O; compilation is not
live MSSQL execution. GitHub Actions remain paused.

The combined candidate contains reviewed #112 → A → B → C → D. Its owned
backend runner executes PostgreSQL prefix/suffix CAS success and exact bind-budget
boundaries, SQLite empty-suffix behavior, and repeated GraphQL string-CAS groups
with atomic journals. Individual A–D heads remain unchanged. Final combined
source review and owner-controlled merge/release are still adoption gates; local
execution does not supply an immutable release identity or authorize a downstream pin.


## Typed mutations and committed output authorization

Enable `RuntimeGraphqlOptions::with_mutations(RuntimeGraphqlMutationOptions::default())`
and compose `mutation_root("Mutation")`. The existing executable host example now
runs typed create, CAS update and exact-key delete with atomic typed journal
inserts, and explicit post-commit public-record release. Enable only the required
actions through mutation options. GraphQL cannot represent empty input objects,
so when a collection has no client-writable field `create<Object>` takes no `input`
argument (it inserts defaults, generated values and automatic timestamps), and when it
has no client-writable non-key field `update<Object>` and `Update<Object>Input` are
omitted. Before 0.75.0 such a module failed to compile with `unsupported_capability`.
Link roots and deletes are unaffected. Generated fields and
[automatic timestamps](runtime-schema-ir.md#automatic-timestamps) are excluded from both
input types, keys are required on create and absent from update. Defaults stay omitted in GraphQL
SDL so omission and explicit null remain distinct. Payloads may select the record's
relations, including nested selections under them: a to-one relation as an object, and
a to-many or many-to-many (join) relation as its `…Connection` page with `first`/`last`,
`after`/`before`, `where`, `orderBy`, `pageInfo` and `totalCount`, as in queries
(0.78.0; before, to-many selections failed with `unsupported_capability`). For a
create or update with attached link changes, or a link mutation, the page shows the
committed links. The payload layer then also loads the record's primary key and the
relation's source key fields (the join's source fields for a join relation), and the
host's read and output checks see them. Each relation is planned
and authorized like a query layer before the write. After commit and
`authorize_mutation_result`, it is read through the relation batch executor. A
related row hidden by its grant is `null`, or absent from the page and its count. A
nested layer denied outright fails the mutation before the write. Payload relation reads share the request's crypto and
projection call budgets with every other mutation field. A failure while reading the
payload after commit (`mutation_payload_failed`, with `extensions.outcome = "committed"`)
means the write committed; clients must not retry it blindly. A page bound exceeded only
when the relation batch is built after commit (for example a per-request
`RuntimeRelationLimits::max_page_size` below the requested `first`) is such a failure.
Nested and bulk writes are unsupported.

Generated deletes refuse `unsupported_cascade` when the collection has incoming
`Cascade` or `SetNull` foreign keys, unless
`RuntimeGraphqlMutationOptions::dependent_changes` is `Some(RuntimeDependentLimits)`.
Then each delete uses B's
[dependent-change contract](runtime-mutations.md#cascade-and-set-null-dependents),
and the mutation hook receives the dependent effects.

Each many-to-many relation adds
`update<Object><Relation>Links(expectedSchema, key, set | add, remove)` when updates are
enabled. It returns the source's `…MutationPayload` and requires the host write authority's
`authorize_links` and `link_target_visibility`. `RuntimeGraphqlMutationOptions::max_link_keys`
bounds the supplied keys and the visible links read; `max_delete_links` bounds the links a
generated delete may remove. A denied link write returns the code `denied`, as does naming a hidden or missing target. Join relations read as `…Connection` fields like other
to-many relations. They do not accept relation filters; mutation payloads may select them.

Attach a certified environment, pinned write authority and optional journal hook
using `with_mutation_environment`, `with_write_authority` and
`with_mutation_hook`. Each validated response-key group uses B's guarded
state-machine transaction once. Repeated inline fields and fragment selections
with the same response key merge their requested payload projections. They share
one transaction, journal hook, post-commit authority decision and immutable
payload/error, even when another alias occurs between the repeats. Distinct
aliases execute serially in first-occurrence document order in separate
transactions. They do not share outcomes, grants or caches across requests.

Preflight conservatively reserves four ORM statements and one materialized node
per distinct runtime mutation group; it measures the merged response shape once.
Host journal/policy I/O needs separate host bounds. Every selection occurrence
still contributes to document/fragment limits. The locked executor may invoke
repeated resolver occurrences, but the request-scoped terminal outcome prevents
another mutation attempt. Its response-object merger combines their selected
fields; cached errors may be reported at each occurrence's location.

Hook failures and cancellation before commit roll back. No automatic retries occur. The host
retains complete policy-revision validation and schema/DDL fencing obligations.

Selected projection and CAS field/operator capabilities are independently checked
before mutation; B's authoritative result check still runs in the transaction.
After confirmed commit, `RuntimeReadAuthority::authorize_mutation_result` receives
schema/collection identity, action, selected projection and the immutable captured
record. `RuntimeMutationRecordKind::DeletedPreimage` explicitly identifies delete
results; no post-commit reread is attempted. Internal keys and policy fields stay
unloaded and are never serialized unless independently selected and authorized.
The object-safe callback defaults to denial, preserving existing read-only host
implementations. Hosts must deny when required policy information is absent.
Only selecting `__typename` requests no record and does not call this callback.

The callback timeout defaults to five seconds and is host configurable through
`output_authorization_timeout`. Denial yields `mutation_payload_denied`, other
errors yield `mutation_payload_failed`, and timeout yields
`mutation_payload_timeout`. All have `extensions.outcome = "committed"` and
`extensions.retryable = false`, without record values or host error details.
These errors cannot undo the mutation or journal. Encoding/response-budget failures
after commit use the same committed failure classification. Whole-request
cancellation or transport loss after commit may deliver no response at all;
clients must reconcile state instead of assuming rollback or retrying blindly.
An uncertain transaction commit remains distinct from a confirmed committed
payload error. No durable replay or idempotency service is provided.

If a host extension cancels an individual mutation resolver but continues the
same execution, that response-key group remains terminal. Repeats do not restart
its transaction, journal hook or output authorization. Cancellation after
confirmed commit produces `mutation_payload_canceled` with `outcome: "committed"`
and `retryable: false` on a subsequent occurrence. Cancellation before commit
confirmation (including while COMMIT may be in flight) produces
`mutation_outcome_unknown` with `outcome: "unknown"` and `retryable: false`.
This conservative classification does not imply rollback; reconcile state.
Whole-request cancellation may prevent any response. Before-commit guarded hook
cancellation still rolls back under B's guarantees. Static transaction
inner-timeout semantics are unchanged.

## Formatted fields and host projections

A field carrying a [semantic format](runtime-schema-ir.md#semantic-field-formats) is served only
after an explicit compile-time decision; a formatted field without one fails
`RuntimeGraphqlModule::compile` with `missing_projection`, before anything is served.

- `RuntimeGraphqlOptions::scalar_format(namespace, versions)` serves every field of that
  namespace and version range as its plain stored scalar. SDL, inputs, filters, ordering and
  values are identical to an unformatted field; the descriptor stays visible to hosts through
  `RuntimeFieldHandle::format()`.
- `RuntimeGraphqlOptions::project_field(field_id, projector)` binds a reviewed
  `RuntimeFieldProjector` to one field by stable field ID. The projector's
  `RuntimeProjectionContract` is read once at compilation: accepted namespace, version range and
  value kinds, a flat `RuntimeProjectedObject` of at most 32 scalar fields, and whether a stored
  value may resolve to no object (`may_be_absent`).

Compilation rejects a binding for an unknown or unformatted field (`invalid_composition`), a
namespace/version/kind outside the contract (`incompatible_projection`), a primary-key or
relation-key member (`unsupported_capability`; keys stay scalar so cursors, key inputs and
relation batching are unchanged), and an object name that collides with a generated or host type
or reuses a name with a different definition (`name_collision`). One projector and object may
serve several fields and modules.

The ORM registers the projected object type itself; its fields read only the authorized payload,
so there is no raw builder hook. A projected field outputs `Field: Object`, nullable when the
stored field is nullable or the contract allows absence. Create/update inputs, `WhereInput`
filters and order enums keep the stored scalar type, and their existing collision, nullability,
filter and cost checks remain authoritative. Selections accept aliases, fragments on the object
type, `__typename`, `@skip`/`@include`; unknown object fields or arguments are rejected.

Execution order is fixed:

1. Every layer is authorized before any ORM I/O, as before. `RuntimeReadCheck::projections`
   lists each projected field with its selected object fields; the stored field also appears in
   `projection`. A denial fails the request and the projector is never called.
2. After a layer's rows are read and decoded, the ORM makes **one** `resolve` call per projected
   field per layer with the distinct non-null stored values of authorized rows only. Null stored
   values output null without a call. Calls run sequentially, each under
   `RuntimeProjectionLimits::call_timeout`; the planner admits `max_calls` per operation and
   `max_values_per_call` before I/O (`cost_exceeded`).
3. The ORM validates the result: exactly one outcome for every requested value and none for
   other values, only declared object fields with matching kinds, nulls only where declared
   nullable, and `Absent` only when allowed. It emits only the selected object fields, encodes
   them with the scalar limits and charges the response budget. The stored scalar itself is
   never emitted for a projected field.
4. Any failure fails the whole operation with a value-free code: `projection_denied`,
   `projection_failed`, `projection_timeout` or `cost_exceeded`. Projector error text and values
   never reach the response.

Mutation payloads use the same contract after the transaction commits and
`authorize_mutation_result` releases the record (`RuntimeMutationOutputCheck::projections`
lists them). The batch purpose is `RuntimeProjectionPurpose::MutationResult { action, kind }`;
a delete resolves the authorized preimage. A projection failure then reports the committed
outcome: `mutation_payload_failed` with `outcome: committed` and `retryable: false`.

`RuntimeFieldProjector` is `Send + Sync + 'static` and held in an `Arc` by the compiled module.
`RuntimeProjectionBatch` borrows the schema, field handle, descriptor, values, selection,
`DbAuthContext` and the optional trusted `RuntimeGraphqlRequest::with_projection_context` value
for the duration of one call only. Projectors must be read-only and cancel-safe: the ORM drops
the future on request cancellation or timeout. Persistent side effects belong in the mutation
transaction (see [hooks](runtime-mutations.md#cancellation-rollback-and-commit)), never in a
projector.

The executable [projection example](../../../crates/graphql-orm/examples/runtime_field_projection.rs)
serves a projected file reference and a plain-scalar email format side by side.
