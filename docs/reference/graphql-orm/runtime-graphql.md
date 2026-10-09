---
title: Runtime GraphQL reads and checked composition
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-09
review_by: 2027-02-01
supersedes: []
---

# Runtime GraphQL reads and checked composition

The additive `runtime-graphql` feature registers read-only dynamic GraphQL over
an `Arc<ValidatedRuntimeSchema>`. It is off by default. Static derives, SDL,
framework-neutral cursors and repository consumers are unchanged. No data
migration is required. This is C, based on the reviewed B revision; mutation
registration D is not implemented here.

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

The unreconciled B predecessor does not yet contain #112. Successful PostgreSQL
StartsWith/EndsWith execution and the SQLite empty-suffix correction remain gated
on reviewed #112 → A → B → C reconciliation. This does not weaken those release
acceptance requirements. No merge, publication or downstream pin is implied by an
unpublished C branch. D requires separate reviewed C/B predecessors.
