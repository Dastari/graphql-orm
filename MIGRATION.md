---
title: "Migration Guide"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-10-22
review_by: 2027-02-01
supersedes: []
---

# Migration Guide

`graphql-orm` is distributed from GitHub only. Select one published immutable
workspace release tag for every chosen workspace package and record its full
commit from the release manifest. Neither the runtime nor macros crate is
published to crates.io.

## 0.59.0 to 0.60.0: generated query-shape selection

Upgrade runtime/macros together. `#[graphql_entity(query_shapes = "list,keyset,scan")]` exposes only those generated query shapes. Supported selectors are `single`, `list`, `keyset`, `scan`, `search` and `aggregate`. The list must be nonempty and contain no duplicates; selected optional capabilities must already be configured. An omitted selector retains all existing generated queries. Disabled shapes are absent from both SDL and operation/semantic metadata; repository methods and entity relations remain available. Mutation and subscription exposure stay independent. This permits a recorded legacy root to coexist while its generated same-name query is deliberately omitted; it grants no authorization and does not make the legacy root an accepted permanent exception. No data or schema migration is required.

## 0.58.0 to 0.59.0: composite-primary-key keysets

Explicit `read = true` or `order = true` never overrides a private configured
GraphQL cursor field. Keyset and scan queries reject it before the first read;
trusted repository pagination retains its independent field policy.

Configured cursors carry raw ordering values, so their fields now require current
read authority even when clients select only `cursor`. GraphQL rejects private,
skipped and transformed cursor fields. Repository policies remain independent.
Manual `AuthorizedKeysetEntity` implementations must supply `KEYSET_CURSOR_FIELDS`;
missing field metadata fails closed. Generated derives populate it automatically.
Ordering, fingerprints and cursor bytes remain unchanged for authorized reads.

Upgrade runtime/macros together. Composite-key entities may now declare `keyset = "rank desc, partition_key asc, item_no asc"` when `(partition_key, item_no)` is the primary key in declaration order. Every primary-key member must appear as the final unique tiebreaker tuple; order fields may not repeat. The generated keyset and scan read surfaces use the existing bounded, authorized cursor implementation. Existing valid single-key orders/cursors are unchanged. No generated GraphQL writes or subscriptions are enabled for composite-key entities by this change. No data or schema migration is required, and an externally managed source stays externally managed.

## 0.57.0 to 0.58.0: public host-owned timestamps

Upgrade runtime/macros together. Public GraphQLEntity and RepositoryEntity timestamp fields may explicitly use `#[graphql_orm(timestamp = "host")]` on ordinary `i64`, `String`, or optional forms whose Rust or physical name is `created_at` or `updated_at`. For an external source timestamp mapped to physical `updated_at`, opt in on the mapped field to prevent automatic epoch assignment. Values are bound unchanged; String content is caller-owned and is not parsed or normalized as a date. Existing automatic behavior remains unchanged without the annotation. The annotation does not change stored type/default metadata; conventional Rust names still need `default = false` to suppress their implicit default when appropriate. Schema-only, generated/version/key/relation fields and date/JSON/boolean adapters remain rejected. No data or schema migration is required merely to opt in; preserve field/row authorization and distinguish source observation time from collection and mutation clocks.

## Generated result disclosure patch: operation catalog 0.4.2 / profiles 0.16.2

Select one qualified published workspace tag containing both patches. Keep
secret classifications, field export bans, generated operator roots and the
full canonical semantic catalogue. Do not filter generated metadata locally.
Generated root results now inherit their entity's classification and structural
export boundary; secret and entirely nonexportable results do not become AI
capabilities. Public results omit provider relationship selections that cannot
reach exportable fields within the configured depth and cycle limits.

Regenerate derived capability manifests and semantic/router descriptors from
the actual finished SDL and semantic catalogue because fingerprints change.
Existing resolver, field, row and provider-egress authorization remains
independent. GraphQL SDL, stored data and schema ownership are unchanged; no
DDL or data migration is needed. Runtime/macros package versions stay 0.60.0.

## 0.56.0 to 0.57.0: exact SQL Server MONEY reads

Upgrade `graphql-orm` and `graphql-orm-macros` together to 0.57.0 using one
published workspace release tag and refresh the lockfile. Router advances to
0.6.4 for the TLS patch lock refresh below. Other companion package versions
and durable wire/schema contracts are unchanged. For an existing SQL Server `MONEY` column mapped to `rust_decimal::Decimal`, use
`#[graphql_orm(mssql_money)]` instead of a portable `decimal(...)` declaration.
The generated SELECT projects `CAST([column] AS DECIMAL(19,4)) AS [column]` so
the driver receives an exact Numeric value. The annotation rejects non-SQL
Server backends and values outside the native MONEY range; it cannot be mixed
with portable decimal metadata. Existing portable Decimal fields, GraphQL SDL,
source tables and stored data are unchanged. No data or schema migration is
needed; in particular this feature performs no DDL against external tables.

Runtime decimals (0.56.0) stay portable at precision 1..=18. A static MONEY
column is not converted by `RuntimeSchema::from_static_entities`; it is
reported as `UnsupportedCapability`. Runtime schemas and JSON runtime values
cannot declare the MONEY definition.

The coordinated workspace also selects `rustls-webpki` 0.103.16 and
`webpki-roots` / `webpki-root-certs` 1.0.10 for the published certificate-distrust
fixes. Refresh downstream locks and rebuild consuming binaries. No stored-data
migration is required. See the [router migration](crates/graphql-orm-router/MIGRATION.md#063-to-064-tls-certificate-distrust-patches)
for the affected trust paths and source-versus-binary release boundary.

## 0.55.1 to 0.56.0: runtime descriptions and exact decimals

Upgrade ORM/macros together with `graphql-orm-operation-catalog` 0.4.1 and refresh the
lockfile. Static derives, generated code, static SDL, stored data and existing serialized
runtime schemas are unchanged; no automatic database migration is needed.

Source changes for direct IR/runtime users:

- `RuntimeCollection` gains `description` and `classification`; `RuntimeField` and
  `RuntimeRelation` gain `description`, `classification` and `export`, all `Option`. Struct
  literals add `None` for each (or use `..existing`). Absent members are omitted from
  serialization, so stored documents and every identity of an unchanged schema stay
  byte-identical. Readers older than 0.56.0 reject documents that use the new members or the
  decimal kind.
- `RuntimeValueKind` gains `Decimal(DecimalDef)` and `RuntimeValue` gains
  `Decimal(DecimalValue)`; exhaustive matches need new arms. `RuntimeValueKind` stays `Copy`
  and `Ord`; `DecimalDef` now derives `PartialOrd`/`Ord`. Canonical identity spells a decimal
  kind with `RuntimeValueKind::canonical_name()` (`decimal(12,2)`).
- `RuntimeSchemaDiagnosticCode` gains `InvalidDescription` and `InvalidClassification`.
  Decimal primary keys, relation keys and generated decimals are `UnsupportedCapability`.
- `RuntimeHostField` arguments are now attached when the field is added to a root; the
  public builder API is unchanged.

Behaviour changes:

- Runtime GraphQL SDL and introspection now carry descriptions for every generated type,
  field, relation, enum value, input member and root. Only descriptions are added: names,
  types, arguments and defaults are identical (the 0.50.2 SDL goldens still match once
  descriptions are removed). Refresh any committed SDL snapshots.
- `RuntimeSchema::from_static_entities` now converts a static decimal column to
  `RuntimeValueKind::Decimal(def)` instead of `String`, and its default to the canonical
  decimal literal. This changes `structural_fingerprint()` only for converted static schemas
  containing decimal columns; compare against catalog schemas that declare the same decimal
  kind.
- Owned/runtime planning refuses an in-place change of a decimal field's precision or scale
  and any change to or from a decimal column type (`InvalidPhysicalContract`): a SQLite rebuild
  would copy scaled mantissas unchanged and PostgreSQL rounds when the scale narrows. Add a new
  field and copy values explicitly. On PostgreSQL, `numeric(p,s)` constant defaults compare by
  value, which only removes spurious default-only differences.
- Adding, editing or removing a description, classification or export changes
  `fingerprint()` and requires a new runtime mutation environment and fresh client
  `expectedSchema`, but no DDL.

Adopting the semantic catalogue:

- Call `RuntimeGraphqlComposer::finish_with_semantic_catalog()` instead of `finish()` to obtain
  the `GraphqlSemanticCatalog`. Every host query/mutation root then needs
  `RuntimeHostField::semantic(descriptor)`, a host subscription root needs
  `subscription_semantic`, and host result objects can be added with `semantic_entity`;
  otherwise it returns `semantic_descriptor_missing`. Plain `finish()` is unchanged.
- To-many relations (connections) and projector-served fields are not advertised in the
  catalogue. Generated runtime mutations are `AiMutationExecutionPolicy::Prohibited`.

See the [runtime schema IR](docs/reference/graphql-orm/runtime-schema-ir.md#descriptions-classification-and-export),
[runtime decimals](docs/reference/graphql-orm/runtime-schema-ir.md#exact-decimals) and
[runtime GraphQL](docs/reference/graphql-orm/runtime-graphql.md#descriptions-and-the-semantic-catalogue)
references.

## 0.55.0 to 0.55.1: fixture lock refresh

Upgrade ORM/macros together and refresh the lockfile. Only a test-fixture lock
changed (`graphql-orm-ai` 0.109.0); there is no source, API, SDL, data or schema
change.

## 0.54.0 to 0.55.0: typed literal defaults and semantic field formats

Upgrade ORM/macros together. Generated code, SDL of existing schemas, stored data and
existing serialized runtime schemas are unchanged; no automatic database migration is needed.

Source changes for direct IR/runtime users:

- `RuntimeField` has a new public field `format: Option<RuntimeFieldFormat>`. Struct literals
  must add `format: None` (or use `..existing`). Serialized schemas without `format` load
  unchanged, and `format` is omitted when absent, so stored documents and every identity stay
  byte-identical.
- `RuntimeSchemaDiagnostic` has a new public field `detail: Option<RuntimeSchemaDiagnosticDetail>`
  and `RuntimeSchemaDiagnosticCode` a new variant `InvalidFormat`. Exhaustive matches need a new
  arm; code that constructs diagnostics needs `detail: None`. Invalid-default messages no longer
  include the submitted literal; match on `code`/`detail`, not message text.
- Runtime GraphQL: `RuntimeReadCheck` and `RuntimeMutationOutputCheck` gain `projections`,
  `RuntimeGraphqlLimits` gains `projection: RuntimeProjectionLimits` and `RuntimeGraphqlCost`
  gains `projection_calls`. Use `..Default::default()` in limit/cost literals. A schema that
  carries a format now needs `scalar_format` or `project_field` at compilation
  (`missing_projection` otherwise); schemas without formats compile as before.

Behaviour changes:

- JSON, bytes and date-time literal defaults are now accepted in their canonical forms; build
  them with `RuntimeDefault::{json,bytes,datetime}`. Every previously accepted literal keeps
  its exact text, rendering and hashes. Legacy non-canonical UUID literals still render as
  stored; new hosts should use `RuntimeDefault::uuid`.
- Owned/runtime planning on PostgreSQL compares recognized constant defaults by value. Schemas
  whose UUID, negative or large integer or negative float defaults previously produced a
  perpetual default-only `AlterColumn` and `BaselineMismatch` now replan as no-ops and certify.
  Plans for any other input are byte-identical; recorded history is untouched. Static planning
  and the 8e1 replay recipes are unchanged.
- Adding, editing or removing a format descriptor changes `fingerprint()` and requires a new
  runtime mutation environment and fresh client `expectedSchema`, but no DDL.

See the [runtime schema IR](docs/reference/graphql-orm/runtime-schema-ir.md#typed-literal-defaults),
[runtime migrations](docs/reference/graphql-orm/runtime-migrations.md#typed-literal-defaults-0550)
and [runtime GraphQL](docs/reference/graphql-orm/runtime-graphql.md#formatted-fields-and-host-projections)
references.

### Upgrading directly from 0.50.2 to 0.55.0

Consumers of `graphql-orm` 0.50.2 (`8e1e5698437d9ea5c0d6ff2c0f29cf7af58d45a0`) apply the
0.55.0 notes above plus the following accumulated 0.51–0.54 changes. None requires a database or
stored-data migration, and macro output is unchanged (`graphql-orm-macros/src` is identical).

- Pin one workspace release tag for every workspace package and refresh the lockfile.
  ORM/macros 0.54.1 only selected Auth v0.22.1; it adds no ORM API change.
  `graphql-orm` and `graphql-orm-macros` move together (0.55.0 pins macros 0.55.0 exactly).
  `graphql-orm-operation-catalog` remains 0.4.0 with no source change. `graphql-orm-storage`
  moves 0.7.4 → 0.7.5 with no source, provider, `[features]` or dependency change; `s3`,
  `local` and the other provider features behave as before.
- `auth-agql` users: the public API now uses Auth v0.22.1 types. Select tag `v0.22.1`
  (`86fabec09eea1eec0f675bc150e90e3809734567`) for every direct `agql-auth` dependency so only
  one Auth source is resolved. Since v0.21.3, Auth adds
  `ResolvedPrincipal::from_current_session` (v0.22.0) and the finite signed role-catalogue
  correction (v0.22.1); see [the signed role-catalogue note](#signed-role-catalogue-transport-bound).
- `auth-agql` behaviour (0.53.0): sensitive checks re-read original token status and current
  authority after binding and successful business-policy awaits, so resolver/status checkers
  are called up to two or three times per invocation and an earlier expiry still bounds it.
  Adjust mocks that count calls or checkers with call quotas.
- New opt-in APIs that need no action: the default-off `owned-planning-replay` feature and its
  `SchemaManager::plan_owned_migration_with_replay_witness*` methods (0.51/0.52), the
  always-compiled `graphql_orm::cache_sync` module (0.54), and `AgqlOperationTarget`,
  `AgqlOwnedOperation` and `AgqlOwnedOperationAuthorizer` (0.53, also exported through the
  `auth-agql` prelude; glob imports with the same names become ambiguous).
- Every build now runs the crate build script (check-cfg registration; `rustc -vV` only with
  `owned-planning-replay`). Ordinary plan hashes are unchanged.

## Signed role-catalogue transport bound

ORM/macros 0.54.1, AI 0.107.4 and router 0.6.3 select published Auth `v0.22.1`
at `86fabec09eea1eec0f675bc150e90e3809734567`. Use this exact tag for direct
Auth dependencies and refresh all host/fixture locks to one source. A different
selector for the same commit still creates a second Rust Auth type universe.

Router 0.6.3 accepts complete signed role catalogues up to the finite default
2 MiB HTTP limit. Explicit lower bounds remain in force. Adopt the coordinated
Auth signature-envelope correction too; transport capacity does not bypass
structural, cryptographic, issuer/audience or exact-content checks. No generated
schema, data migration or host policy change is added.


## 0.53.0 to 0.54.0: service-owned cache synchronization

Upgrade ORM/macros together. Existing generated schemas and stored data are
unchanged; no automatic data migration is needed. New adopters register a
service-owned status entity and implement `CacheSyncStore` with transaction-bound
generated repositories. Use `CacheSync::{claim,promote,fail}` for fenced refresh
and atomic generation promotion; pass a live clock such as
`cache_sync::unix_time_seconds` so lease checks remain current across awaits.
Use the status entity's generated `…Changed`
subscription for authorized post-commit status invalidation. For an explicit
initial registration barrier and lag recovery, expose `CacheSync::changes` in a
native subscription with a current-authority callback, or use
`changes_with_agql` with the existing owned Auth bridge. `CacheSyncChange` uses
lossless decimal generation strings and explicit `RefetchRequired` signals.
Existing generated streams remain
best-effort: clients must reconcile the initial subscription/query race and
refetch unconditionally after reconnect or gaps. See the
[cache coordination reference](docs/reference/graphql-orm/cache-sync.md).


## 0.52.0 to 0.53.0: post-await and owned release checks

Upgrade ORM/macros together. Successful sensitive checks now re-read original
status/current authority after binding and business-policy awaits. Callbacks run
once per invocation and must perform only admission work: protected effects
belong after final success. Business denial stays denied. An earlier, shorter
current expiry still bounds the invocation even if a later read extends it.
Parallel fields keep immutable accepted identity; narrowing denies rather than
rewriting their shared context. These are read-observation checks, not atomic
fences against remote revocation after the final read.

Custom streams returning owned futures can capture `AgqlOwnedOperation<T>` from
a real context and check every release with mandatory owned binding/business
policy, including after queue/owner awaits. There is no fabricated Context or
reusable allow permit. Preserve host ownership/filter/row/field/deleted-preimage
and commit/revision fences. See [the bridge guide](docs/reference/graphql-orm/agql-auth-bridge.md#owned-custom-release-checks).
No SDL, wire, database or stored-record migration is required. Select published
Auth `v0.22.0` for every direct Auth dependency and refresh the lockfile to
`997707e060bcf5fabf879a5f98146dc2732b1c99`, retaining one tag source across
ORM, AI, router and direct consumers. For an authoritative ordinary-session
projection with distinct access-token jti/correlation/expiry metadata, use
`ResolvedPrincipal::from_current_session(original_reference, current_session, now)`.
It validates identity, kind, session/family/tenant/bindings, security version and
both deadlines. Preserve the original accepted principal and independently
check its original token status; never copy original metadata onto the current
session or replace the accepted carrier. Current grants only narrow the original
ceilings, and a longer current lifetime cannot extend the accepted credential.
The legacy `ResolvedPrincipal::new` sensitive path remains strict. Host binding,
owner/current-row checks and transaction/revision fences remain mandatory.


## 0.51.0 to 0.52.0: explicit native Rust 1.98.1 replay recipe

Source releases may explicitly select the reviewed scoped positive release
profile; inspect the attached verification evidence and runtime-unverified gaps.
The full release profile remains the default. No package version or database
migration is added by the release-policy change. See the
[release runbook](docs/operations/release/process.md#replay-scoped-positive-v1).

Upgrade ORM/macros together. Existing historical capture/decode entry points keep
all signatures/defaults and remain on exact Rust 1.97.1. They do not automatically
select a new recipe when the compiler changes. Existing witness bytes, producer
identities, format 1 mapping and historical provenance are unchanged.

For new evidence on exact qualified Rust 1.98.1, explicitly select
`OwnedPlanningReplayRecipe::Rust1981V1` in
`plan_owned_migration_with_replay_witness_for_recipe` and
`OwnedPlanningReplayWitness::from_json_for_recipe`. The chosen recipe must equal
the encoded recipe; each pair is library-admitted only on its own exact stock
compiler commit, GNU x86_64 host/target and absence of compiler wrappers.
`to_json`/`verify` use the privately retained recipe; no relabel or fallback exists.

Rust 1.98.1 can produce different native Debug-based plan hashes for valid Unicode
metadata. This release labels those native facts with a separate recipe; it does
not change ordinary planning or apply hashes. Bind complete canonical witnesses,
format/recipe and authenticated producer artifact identity with external
cryptographic custody; matching `planHash` alone does not establish equivalence.
No baseline can be reconstructed, old receipt adopted/upgraded or approval
reissued by selecting the new recipe. Separate historical verifier integrations,
release publication and consumer/pin adoption require review.

The feature remains default-off and explicit per call. No database migration,
dependency change or change to existing callers is required. See the
[recipe/compatibility reference](docs/reference/graphql-orm/owned-planning-replay.md)
and [per-recipe accounting](docs/reference/graphql-orm/owned-planning-replay-accounting.md).

## 0.50.2 to 0.51.0: optional owned planning replay

Upgrade ORM/macros together. Existing callers need no code, configuration,
serialized-format or database migration. Enable `owned-planning-replay` explicitly
alongside `sqlite` or `postgres`, then call
`plan_owned_migration_with_replay_witness` with a checked limits profile and
`OwnedPlanningReplayCaptureConsent::accept_sensitive_schema_metadata_and_private_custody()`.
The separate witness retains sensitive complete schema metadata; the caller must
keep it private and bind external cryptographic custody and producer identity.
`verify()` returns passive facts and does not replace any live apply check.

The new format is v1 and the supported recipe is
`graphql-orm-owned-planning-8e1-v1`, qualified to Rust 1.97.1. Authentic fixtures
prove the listed historical cases, not arbitrary historical receipt adoption or
all toolchains. Capture/codec/replay now enforce that exact compiler commit with
stock `x86_64-unknown-linux-gnu` host/target and no compiler wrappers at build time;
unqualified builds return `LimitExceeded` before those operations start. Ordinary
planning is unaffected. Nested structs remain JSON objects and enum values remain
strings, including buffered step payloads with `kind` last. The increased prepaid
decoder allowance can refuse additional near-ceiling witnesses. No missing
historical baseline can be recovered. Unsupported
recipes are rejected; old plans are never silently resealed. MSSQL capture is
unsupported before connection acquisition; ordinary MSSQL builds remain available.
See [the replay reference](docs/reference/graphql-orm/owned-planning-replay.md)
for limits, exact mapping, compatibility and remaining inspection-only obligations.
Release/pin adoption remains a separate review; this branch does not publish.

## 0.50.1 to 0.50.2: published Auth source alignment

Upgrade ORM/macros together. The workspace now selects the published
`agql-auth` `v0.21.3` tag, which peels to the same reviewed commit
`59c0a38f82e8c75459474238799a81f29d4ffa58` used by 0.50.1. No authentication
behavior, Rust API, token wire, stored-data or database-schema change is added.

Update every direct Auth dependency to the same selector:

```toml
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", tag = "v0.21.3", version = "0.21.3" }
```

Refresh the application's lockfile deliberately and inspect its selected
dependency graph. It must contain one `agql-auth` package source,
`git+https://github.com/Dastari/agql-auth.git?tag=v0.21.3`, locked to the commit
above. A `rev` selector is a different Cargo source even at the same commit;
mixing the two can produce incompatible principal/validator types. Update the
workspace release selector and direct Auth selector together, then recompile
the host's bridge, AI and router integrations. Do not move existing release tags.
Consumers upgrading from Auth 0.19.1 still need its refresh-provenance and atomic
challenge-store migrations; this patch does not remove those requirements.

## 0.50.0 to 0.50.1: rendered read bind budgets

Upgrade ORM/macros together. No stored-data/schema migration is required.
Ordinary runtime row and optional count queries now reject actual emitted bind
slots above `RuntimeQueryLimits::max_bind_parameters` before database I/O, using
existing `resource_limit`. SQLite suffix comparisons consume two slots; cursor
lexicographic branches may repeat earlier values on either backend. Requests
previously admitted by logical operand counts may now reject. Size the configured
cap for the rendered query within the backend's supported limits, or simplify the
filter/order. SQL, cursor wire forms, authorization and RLS semantics are unchanged.
The 0.50.0 canonical logical-construction compatibility below remains; execution
now independently applies the corrected emitted-slot check.

## 0.49.0 to 0.50.0: checked filter intent and offline matching

Upgrade ORM/macros together. This additive API requires no stored-data/schema
migration; existing predicates, key reads, record codecs and wire forms remain
compatible. Use the core `runtime_filter_builder` to construct private checked
nodes and `finish` them, or opt into `runtime-graphql` for
`prepare_runtime_filter(schema, collection, &Value, limits)`.

A Value map cannot recover duplicate wire keys. The caller must validate GraphQL
coercion and duplicate input keys **before** conversion to Value/Variables. The
current upstream parser and JSON Value decoder erase duplicate object keys
before runtime validation; this change does not repair or certify that wire
boundary. It preserves every occurrence present in the supplied structural Value,
including repeated Boolean children and null/empty entries. No raw JSON entry is
added. Construct trusted Values or supply a separately validated transport seam.

Authorize the complete `visit_intent` tree before protected SQL or matching:
`referenced_fields` alone still omits constant-lowered intent. Implicit object
conjunctions emit `ImplicitAnd`, distinct from explicit logical operators. Existing
flat RuntimeReadCheck authorization ignores implicit conjunction permissions and
retains its prior public behavior. Public checked null operators validate
filterability and operator/kind admission; existing canonical null skips remain
private compatibility behavior. Existing configured GraphQL scalar limits and
legacy logical bind admission remain unchanged; the new public checked artifact
reserves worst-case emitted binds, including SQLite suffix duplication. Existing
canonical execution is not upgraded to a new actual-bind policy by this change. The core builder has
fixed text/binary literal caps of 64 KiB/32 KiB and counts SQLite suffix binds twice;
imported/reused nodes revalidate full bounds on each composition/finalization.

`prepare_match` validates the full original operator profile; `matches` validates
all original loaded dependencies before evaluation. Missing is an error, never
NULL; SQL UNKNOWN remains UNKNOWN under NOT, and only TRUE selects. JSON
comparisons remain unsupported. Even JSON IS NULL, which can be a valid checked
SQL filter, is outside the first conservative offline profiles. MSSQL returns
`UnsupportedSemantics` before matching. Safe errors/Debug include no literals or
record values.

Profile selection is a trusted physical-storage assertion, not inferred from a
backend or decoded record. SQLite requires String TEXT/BINARY UTF-8, booleans
INTEGER 0/1, exact i64 INTEGER, finite REAL, bytes BLOB, lowercase canonical UUID
TEXT and microsecond UTC timestamp TEXT. PostgreSQL requires UTF-8/C String TEXT
(or proven equivalent VARCHAR, never fixed CHAR/bpchar), native typed values and
finite floats. Date-times are restricted to canonical years 0001–9999 without
leap seconds; control/NUL strings are rejected. Foreign storage spelling/classes
can be normalized by row decoding, so the matcher cannot detect prior violations
(e.g. SQLite integer 7 decoded as true or uppercase UUID text). Unsupported
profiles must fail closed; do not guess Rust comparison equivalence.

An authoritative immutable image remains a host premise. Matching supplies no
principal/tenant/field policy, snapshot authenticity, current authority or native
RLS proof. In particular it cannot establish a deleted row's current SELECT
visibility or bypass default-deny deleted-preimage release. See the canonical
[runtime query reference](docs/reference/graphql-orm/runtime-queries.md#checked-original-intent-and-offline-matching)
and host-neutral `examples/runtime_filter_match.rs`.

## 0.48.x to 0.49.0: checked owned complete-key reads

Upgrade ORM/macros together. The additive `runtime_key_read_request` and
`execute_runtime_key_read` API reuses `RuntimeKey`, checked projections and
ordinary predicates. Existing reads, mutations, static subscriptions, GraphQL
names and record/schema formats are unchanged. Adopting this read API requires
no data/schema migration. The separate default-change and guarded-repair
obligations in the corrective migration sections below still apply.

Authenticate and reauthorize current host authority, requested fields and
key-read intent before constructing a request. Supply a mandatory row predicate
and coherent `DbAuthContext`; an explicitly true predicate is a trusted host
choice, not authorization evidence. Do not infer complete filter intent from
`referenced_fields`: constant folding may erase dependencies. No new filter
matching/lowering capability is included.

Requests own the checked schema/key/projection/predicates and may be retained,
but the executor cannot discover a later host catalog revision. Rebuild and
authorize against the current trusted schema before each sensitive call.
Node/depth/list/projection/bind budgets are rechecked under the request limits;
hosts retain responsibility for inbound byte budgets. Missing and policy-hidden
rows return the same `None`. Unexpected physical multiplicity fails without a
record; a fixed two-row lookahead is retained even with a page-size limit of one.
Cancellation follows existing backend cleanup without automatic retry or a
cross-call snapshot guarantee. MSSQL runtime decoding remains unsupported.

See [runtime query trust and point reads](docs/reference/graphql-orm/runtime-queries.md)
and the [host boundary example](crates/graphql-orm/examples/runtime_key_read.rs).

## 0.48.2 to 0.48.3: unsupported PostgreSQL alterations fail closed

Upgrade ORM/macros together. Existing fallible managed planning and application
now reject unsupported changes to per-column primary-key or constraint-backed
unique flags, and spatial metadata changes with the same SQL type expression.
Owned paths return `InvalidPhysicalContract`; static managers return a fixed
`Protocol` error. Direct/owned rejection occurs before runtime/history
preparation. Legacy staged/ABI planning may initialize an empty validated history
table, but rejects the complete stage sequence before target DDL or history
rows. `allow_destructive` cannot bypass capability admission. No new constraint DDL, public API signature or automatic postapply contract is added.

PostgreSQL introspection obtains column uniqueness from unique constraints,
not standalone unique indexes. An explicit standalone unique index with unchanged
column flags remains supported; it does not repair an unsupported flag change.
Same-type means the existing case-insensitive expression comparison or exact
`TIMESTAMPTZ`/`timestamp with time zone` aliases. Changed SQL type expressions
retain the existing renderer path; this does not prove semantic type difference
or correspondence between `sql_type` and spatial metadata. Whitespace, other
aliases and incoherent host metadata remain outside this bounded guard. Trusted
direct `SchemaModel`/`OwnedSchemaModel` hosts own that coherence; the runtime IR
rejects all spatial metadata. The geometry positive test proves planning/rendering,
not a live conversion or target certification. Real owned primary-key vector
transitions were already rejected. SQLite/MSSQL behavior is unchanged.

Low-level infallible diff/render/build functions remain structural previews,
not certification that every requested step can execute. Do not bypass the
managed preflight by converting an unsupported preview to trusted raw SQL.
Review already recorded ineffective alterations against actual physical state;
remaining-work version reuse fails closed. Use a fresh reviewed supported repair
version or host-managed DDL, without rewriting historical records.

## 0.48.1 to 0.48.2: PostgreSQL default alterations

Upgrade ORM/macros together. PostgreSQL plans now execute real default changes
on existing columns rather than recording their target while leaving the old
physical default. The change affects future inserts that omit the column; it
never backfills or rewrites existing values. Static and owned targets share the
renderer. SQLite and MSSQL behavior, public API signatures and physical/hash
formats are unchanged. No automatic data migration is introduced.

Review pending plans before applying them: SET/replace/DROP DEFAULT is now
effective. Actual type changes remove an attached old default before conversion
and restore the target expression afterward; unsupported data conversions still
fail and roll back atomically. Existing guards, ownership/RLS boundaries and
explicit migration policy remain required.

If an older version recorded a default-only migration without reaching its
physical target, introspect and review the actual default. A fresh replan under
the same recorded version fails closed when work remains; it is not treated as
already applied. Do not edit/delete history or silently reuse that version.
Create a fresh unique migration version for the reviewed guarded repair, apply
it and confirm introspected default, omitted-value insert behavior, empty replan
and owned mutation certification. Retain the original history as evidence.

## 0.48.0 to 0.48.1: PostgreSQL string default round trips

Upgrade ORM/macros together. PostgreSQL TEXT literal defaults now compare equal
to the same complete literal with PostgreSQL's deparsed `::text` cast. This fixes
unchanged owned-migration replans and runtime mutation certification after a
successful apply. Different literal values and unsupported SQL/casts remain
distinct; this does not interpret arbitrary default expressions.

No stored-data, SQL schema, GraphQL SDL or migration-history rewrite is required.
Default SQL and physical/plan hash representations remain unchanged. Existing
guarded baselines still detect physical drift; regenerate plans only when the
actual live baseline changes. Runtime string rendering continues to use standard
SQL string semantics (`standard_conforming_strings=on` on PostgreSQL).
Legacy epoch-second DateTime rejection and host-managed Integer timestamps are
unchanged. The macros patch version keeps the paired packages aligned.
SQLite CHECK introspection also handles preceding Unicode defaults without a
UTF-8 boundary panic and retains the original constraint semantics.

## 0.47.0 to 0.48.0: SQL Server platform TLS

Upgrade ORM/macros together. The `mssql` feature now selects Tiberius native TLS
instead of its legacy Rustls 0.21 transport. The reviewed driver revision and
query-notification API stay the same. No SQL schema, GraphQL SDL or stored-data
migration is required. SQLite and PostgreSQL transport selection is unchanged.

For MSSQL deployments, install platform TLS dependencies in both build and
runtime images. Linux builds require OpenSSL development headers and `pkg-config`
(for example `libssl-dev` and `pkg-config` on Debian/Ubuntu); runtime images require
the matching OpenSSL shared libraries and CA certificates. Cross-compilation and
static/musl images must explicitly supply their target OpenSSL toolchain. Windows
uses Schannel and macOS uses Security Framework. Validate your deployment target
and do not assume a prior pure-Rust MSSQL image remains sufficient.

During TLS negotiation, the default trust configuration verifies the certificate
chain and hostname using platform roots. Set `Encrypt=true` to require TLS for
the full session. Omitting `Encrypt` from an ADO connection string allows
login-only encryption when the server does not require encrypted sessions.
For private PKI, provision the platform trust store or set
`TrustServerCertificateCA=/absolute/path/ca.pem` (PEM/CRT/DER supported); the private
CA augments the platform roots and hostname verification still applies. Test with
the actual server hostname and a certificate containing the matching SAN. Do not
set `TrustServerCertificate=true` for sensitive deployments.
That explicit escape hatch still disables chain and hostname validation and is
only used by isolated fixtures. TLS errors should be fixed through trust and
hostname configuration rather than by disabling verification.

## 0.46.0 to 0.47.0: current authority and coherent accepted requests

Upgrade ORM/macros together. `Database` now defaults to
`DeclaredPoliciesRequired`. Install the required entity, field and row providers
for declared policy keys before deployment. Missing providers now return
`AUTHORIZATION_MISCONFIGURED`; an explicit `LegacyPermissive` opt-in retains the
compatibility behavior. This default protects marked policy requirements only;
it does not authorize unmarked operations or create tenant isolation. Sensitive
applications should use `ExplicitPolicyForAllExposedOperations` with host policies.

Federation subgraphs with a declared entity `read_policy` must also register a
`RowPolicy`: generated entity-key resolvers check each fetched row with that
key after entity access succeeds. Authorize the typed row from current request
authority; an entity provider alone does not satisfy this route's row check.
Collection/relation paths retaining unrestricted entity-only visibility have a
separate contract. See the [strict authorization guide](docs/reference/graphql-orm/strict-authorization.md).

For immediate revocation with `auth-agql`, replace independently injected
`AuthSubject`/`DbAuthContext` values with
`AgqlAcceptedRequest::new(authenticated_principal).inject(request)?`. Authenticate
first using the trusted host transport. The fallible injector rejects existing
conflicting copies before mutation; checks also reject missing, mixed actor-kind,
shadowed conflicting or schema-only identities. The complete principal retains
bindings omitted from safe database projections. Never put per-user identity on
shared schema configuration or client-controlled claims into transport evidence.

Wrap the business entity policy in `AgqlCurrentAuthorityPolicy`, providing a
mandatory current resolver, token-status checker, trusted clock and binding
authorizer through `AgqlCurrentAuthorityEnforcement`. Revalidate each selected
sensitive entity operation, including nested relations and Federation lookups.
The binding callback must authorize every applicable restriction or deny. A
narrowed current grant requires fresh authentication/refresh, rather than mutation
of shared GraphQL context. Context-free repository convenience calls deny; custom
resolvers/raw pools/direct loaders must enforce current authority and business
policy explicitly. The custom guard performs immediate checks, not business policy.

The decision observes authoritative status at evaluation. For atomic fencing of a
write racing revocation, use a host transaction/session-version boundary. Read the
[bridge guide](docs/reference/graphql-orm/agql-auth-bridge.md) for transport evidence,
parent expiry, asynchronous expiry checks and unsupported resource restrictions.
No physical database schema or GraphQL SDL migration is required.


## 0.45.0 to 0.46.0: bounded protected offset and search reads

Upgrade runtime/macros together. Generated ordinary and append-only offset lists
with callback-only/prefilter visibility no longer fetch every candidate in one
unbounded allocation. Install the existing host-owned scan configuration before
exposing those operations:

```rust,ignore
let database = database.with_authorized_scan_config(AuthorizedScanConfig::new(
    host_scan_cursor_key, // Secret, randomly generated [u8; 32] from host configuration.
    64,                  // Maximum candidates fetched per SQL batch.
    4096,                // Maximum candidates examined per operation.
));
```

The same configuration controls existing bounded relation and cursor-scan paths;
there is no implicit unlimited fallback or legacy unbounded opt-in. Keep the key
stable across instances sharing encrypted scan cursors and rotate it deliberately.
Missing/zero bounds or exhaustion return `AUTHORIZATION_MISCONFIGURED` before
releasing nodes or misleading partial totals. Configure a finite page limit and
allow room to prove exhaustion: if the final batch exactly fills the remaining
budget, the operation fails closed rather than guessing there are no more rows.
Visible offsets and exact totals are retained only when scanning completes within
the host budget. Empty denied batches do not establish end-of-data. Filters that
require whole-result in-memory matching are rejected on these bounded paths;
move filtering into supported SQL predicates.

Generated search also uses bounded work. Native SQLite/PostgreSQL
ranking is retained and scanned in stable SQL batches when callbacks remain.
Fallback scoring (including MSSQL) scans bounded raw candidates and retains
only the requested highest-score window; its budget includes nonmatching and
unauthorized raw candidates. A complete SQL visibility predicate bypasses row
callbacks as specified by that contract. Complete native search continues using
SQL count/page queries under Complete/Unrestricted visibility. Native PostgreSQL relevance is explicitly widened from SQL real to double precision to match the public f64 score decoder; ordering is unchanged. Generated fallback always requires a scan budget, including when no `RowPolicy` is registered: database RLS is not discoverable through that registry. The
bounds constrain rows fetched/decoded and policy calls, not database planner or
COUNT work; host statement deadlines and indexes remain separate controls.

GraphQL shapes and successful exact count semantics are unchanged. AI/tool
consumers must handle the safe budget error as a failed operation; do not turn it
into an empty page, partial success or an unbounded retry. For large protected
sets use the existing explicit `...Scan` cursor contract where available, which
reports incomplete progress without inventing an exact total, or provide a
reviewed complete SQL visibility predicate. No database/schema migration is
required. Handwritten low-level `fetch_all` APIs remain host-owned full-fetch
boundaries; this change covers generated GraphQL protected offset/search operations.
`RepositorySearchQuery::fetch_all` retains its separate repository policy-record
contract and can materialize fallback candidates; a return-value limit is not
an authorization-work budget for that explicit Rust full-fetch API.

## 0.44.1 to 0.45.0: subscriptions authorize each release

Upgrade runtime/macros together. Existing subscription SDL and public changed
structs are preserved, but delivery is narrower. Every generated ordinary and
append-only event rechecks request auth, subscription scopes/assurance, current
entity policy and current read visibility using the original request Context.
Host revocation policies must consult current authority on each invocation.
Unauthorized events and policy/query failures are silently dropped before any
identifier, provenance or selected payload is released. Selected protected fields
(including aliases/fragments) and event identifiers require field authority;
outer event identifiers also require a subscribable primary key; arbitrary PK
guards are rejected at admission even through aliases/fragments. Requesting a
forbidden dynamic field-policy decision drops the whole event rather than
releasing outer metadata alongside a GraphQL field error.

Selected entity payloads support persisted scalars only. Subscriptions requesting
nested relations, object-valued persisted JSON/custom columns, computed/custom
resolver fields, or fields with arbitrary
GraphQL `guard`/dynamic `visible` behavior are rejected at admission
with `INVALID_INPUT`: their late resolver decisions cannot prove release of outer metadata. Subscribe to
invalidation metadata and refresh separately authorized queries for richer data.

Live payloads now contain the freshly loaded authorized current row. The
GraphQL witness channel retains metadata/typed keys for live and propagated
events; live delivery does not clone historical snapshots for rejected subscribers. Only explicit deleted
preimages retain a snapshot for the default-deny authorization hook. Events are
invalidation notifications, not an immutable change journal: queued historical
payloads, including a previous owner's data after ownership transfer, are not
released. Direct generated deletes suppress their trusted final preimages by default.
To opt in, implement `RowPolicy::can_read_deleted_preimage` with full current
request authorization for the prior-owner snapshot, in addition to ordinary
entity/row/selected-field policies. The new method defaults to deny; no RowPolicy
provider or a permissive `can_read_row` alone never authorizes a deleted snapshot.
`Unrestricted` means no ORM SQL predicate, not visibility under database RLS.
The opt-in callback must independently enforce every tenant/owner and database-
session condition that RLS or another SQL-only policy would have enforced.
Complete/prefilter SQL visibility still suppresses deleted preimages because its
predicate cannot be evaluated against a missing row. Use a separate explicitly
authorized audit/history service if the host cannot prove snapshot access.

Propagated events require current authorization of the target and every source
hop, including source primary keys, foreign keys and relation field. Missing,
deleted, denied or unresolvable sources suppress the event. Propagation stops at
16 relationship hops or 256 examined targets per original change; consumers must
refresh their own authorized queries instead of assuming complete graph history.
Repository-only entities have no public subscription field authority: a propagation chain containing such a source is suppressed in GraphQL, including when its target is public. Target-owned generated emission preserves repository event payload/identity redaction and never creates a repository GraphQL witness. Trusted Rust observers retain their existing event channel.
Only typed keys are retained for source proofs; no recursive sensitive source
snapshots are queued. Public raw/deserialized `ChangedEvent` values sent through
`Database::emit_event` remain available to Rust event observers but no longer
enter generated GraphQL subscriptions. Use generated writes for GraphQL
notifications; a host-owned emitter must implement its own reviewed authorization
boundary rather than injecting raw events. The internal generated witness types
are trusted server mechanisms, not cryptographic authority for untrusted inputs.

`filter.actions` is now honored, including an empty list matching no actions.
Remove `filter.dummy`: supplying it is rejected as `INVALID_INPUT`. No schema or
data migration is needed. AI consumers of generated operations inherit the same
current authorization; this change does not add subscriptions to AI tool APIs.

## Test runner migration: owned database fixtures

For repository contributors, unset `DATABASE_URL`, `TEST_DATABASE_URL`, and
`MSSQL_TEST_DATABASE_URL`; legacy tests no longer connect to supplied endpoints.
Install local Docker and run `scripts/run-owned-database-lanes.sh postgres-legacy`
or `mssql` to explicitly execute container-backed cases. Ordinary unit runs
report those cases as ignored rather than silently passing. Application data,
public library APIs, and persisted formats are unaffected.

## 0.44.0 to 0.44.1: generated resolver public errors

The workspace also raises its compatible `rust_decimal` minimum to 1.43.0,
which removes the upstream optional rkyv 0.7 bridge. ORM SQL/JSON decimal values
and public ORM signatures are unchanged. Applications separately enabling that
Decimal archival feature must migrate their archive integration using the
[upstream 1.43 migration notes](https://github.com/paupino/rust-decimal/releases/tag/1.43.0); do not assume persisted rkyv bytes can be read using another format
version. The ORM does not persist Decimal through rkyv. TLS/QUIC and yanked
dependency patch refreshes require no ORM data migration.

Upgrade runtime/macros together. Generated SQL and Federation failures now use
stable safe GraphQL messages and `extensions.code` instead of backend diagnostic
text. Match public codes such as `INTERNAL_ERROR`, `SERVICE_UNAVAILABLE` and
`CONSTRAINT_VIOLATION`; stop matching SQL text, table names or driver strings.
Existing legacy mutation result objects retain their `success`/`error` shape,
with a safe message in `error` rather than raw backend details. They do not gain
an error-code field in this patch. Typed SQLx constraint categories are retained;
SQL Server errors currently represented as SQLx Protocol retain the sanitizer's
safe `INTERNAL_ERROR` classification.

No data/schema/SDL migration is required. Internal details continue through the
existing opt-in `GRAPHQL_ORM_LOG_INTERNAL_ERRORS` server diagnostic path and are
excluded from client extensions. Host-defined errors and deliberately mapped
extensions remain host-owned. Low-level repository errors remain internal values. Loader String failures now
carry only a public category; handwritten loader consumers should use
`graphql_error_from_loader`. Applications exposing repository errors through
handwritten resolvers must use the public mapping instead of forwarding Display
text. Unknown host loader failures are sanitized as `INTERNAL_ERROR`.

## 0.43.1 to 0.44.0: authorization and relation partition identities

Upgrade `graphql-orm` and `graphql-orm-macros` together. No database schema or
GraphQL SDL migration is required. `AuthSubject::canonical_key()` and
`DbAuthContext::canonical_key()` now return `auth-v2:` plus a SHA-256 digest of
unambiguous, versioned structural fields. Invalidate stored authorization cache
keys and any host cursors derived from the old format; never compare old and new
keys. Hashes are opaque identifiers, not encryption or an authentication proof.

Use the new `partition_key()` value for in-memory authorization partitions.
Equality/hash include complete claims rather than a short fingerprint and
preserve absent versus present-empty metadata. Roles/scopes are treated as sets,
normalizing order and duplicates. JSON object property order is immaterial.
Structural keys retain claim bodies in memory and redact Debug output; external
canonical keys expose only the digest. Continue to exclude tokens/secrets from
claims and protect authorization metadata in host logs and caches.

`RelationKey::from_sql_values()` now preserves SQL type and lossless binary
identity. `RelationKey::new()`/`single()` construct text-only keys. Hosts manually
constructing numeric/UUID/composite loader keys should use `from_sql_values()`
for bind values, or `from_projected_parts(parts, kinds)` for non-null SQL text
projections with exactly one declared type per part. `parts()` remains a text
view, not a complete identity; binary entries now use lossless `bin:` hexadecimal.
Do not rebuild typed identities using `new(key.parts().to_vec())`. Recreate
application-owned loader caches during upgrade. Generated keys and SQL row
projections use matching declared types on SQLite/PostgreSQL/MSSQL. This fix does
not add binary/JSON/decimal source support to the relationship derive; those
values have lossless identity in the general SQL-value key constructor only.

## 0.43.0 to 0.43.1: host-supplied integer primary-key binding

No data migration, public signature, feature or GraphQL SDL change. Fields declared
`#[primary_key] #[graphql_orm(auto_generated = false)]` continue to require the
caller key in the generated create input. Integer keys now compile and bind as
integers in generated insert/readback and related single-key write paths, using
the existing typed key binder. No replacement key is generated, and event/display
IDs retain their existing wire format. String, UUID and binary keys retain their
native binds; composite writes continue to use the complete declared key.

MSSQL `external_writable` still requires a physically writable connection and
never synchronizes or migrates source schemas. SQLite/PostgreSQL managed schema
behavior is unchanged. Supported Rust key types and integer storage domains are
unchanged; the existing signed-i64 SQL-value domain is not widened by this fix.
For narrow PostgreSQL keys use matching externally provisioned `SMALLINT`/`INT`
columns for `i16`/`i32`. Existing static integer-to-`BIGINT` schema defaults and
strict PostgreSQL row decoding are unchanged; this fix does not revise them.
The external consumer `host_integer_key` example uses only a transaction-bound
repository insert; hosts retain allocation, policies and source-schema ownership.

## 0.42.0 to 0.43.0: opt-in runtime GraphQL mutations

The combined 0.43.0 candidate contains the reviewed independent string-binding
fix and runtime slices A–D. Intermediate unreleased version entries below record
reviewed branch contracts; they are not immutable published release identities.
Owner review/merge and an exact-source workspace release remain adoption gates.

No data migration or static API/SDL change. Mutation registration requires an
explicit options builder, checked mutation root, certified environment and host
write authority. Existing read authorities remain source-compatible; releasing
mutation records now requires an explicit post-commit callback override. Default
denial leaves committed writes/journals intact and reports a committed outcome.
Repeated mutation fields with the same validated response key now execute once
with a merged projection, including fragment repeats. Distinct aliases retain
separate transactions in document order. Repeated occurrences reuse terminal
payloads/errors and never restart canceled work. A host extension continuing
execution after resolver cancellation receives nonretryable committed or unknown
outcome errors according to commit confirmation; whole-request cancellation can
still prevent delivery.
See [runtime GraphQL](docs/reference/graphql-orm/runtime-graphql.md) for callback,
null/default/key behavior, cancellation uncertainty and compiled examples.

## 0.41.0 to 0.42.0: opt-in runtime GraphQL reads

Enable `runtime-graphql` alongside an explicit SQLite or PostgreSQL feature. No
stored-data migration or static SDL change is required. Hosts supply request
read authority and AEAD cursor protection before installing a protected module.
The dynamic scalar wire forms are new and do not replace static scalar mappings.
See [runtime GraphQL](docs/reference/graphql-orm/runtime-graphql.md) for composition,
limits, cursor rotation, SDK mappings and pending release/contract gates.

## 0.40.0 to 0.41.0: transactional runtime mutations

Adopt aligned ORM/macros 0.41.0. This is an additive, unpublished successor to A;
no stored-data migration, macro syntax or static GraphQL SDL change is required.
Use `TransactionMode::StateMachine`, a verified runtime mutation environment and
explicit host authority. The host pins its complete policy revision and external
DDL fence. Runtime mutation errors/cancellation now poison only transactions that
perform runtime work; static-only timeout behavior is unchanged. Use the guarded
before-commit hook for atomic host journal writes. Effects remain pending until
the runner commits. See [runtime mutations](docs/reference/graphql-orm/runtime-mutations.md)
for unsupported capabilities, exact omission/null semantics and ambiguous commits.

## 0.39.0 to 0.40.0: owned runtime migration targets

Adopt runtime and macros 0.40.0 together. This unpublished identity retains the released
0.39.0 static capabilities without changing the approved migration contract. Existing static enums, variant imports,
constructors, exhaustive matches, struct literals and function pointers remain
source-compatible; no call-site or stored-data migration is required.

Runtime hosts may opt into `ValidatedRuntimeSchema::physical_schema`, compose
static metadata, explicitly declare owned physical tables, preview a plan, then
apply its immutable artifact with ordinary `ApplyOptions`. Retain intentionally
removed tables in the ownership set to review drops. Baseline validation is always
required for owned plans; `require_clean_schema = false` does not disable their
immutable baseline binding. Replan after drift and after successful application.
Owned application does not reconcile PostgreSQL RLS. Unsupported backends fail
before connection acquisition; unsupported live preservation cases return structured
rejections. See [owned runtime migrations](docs/reference/graphql-orm/runtime-migrations.md).

Owned runtime operations reject unsupported live FK semantics with a scoped
`UnsupportedForeignKey` diagnostic. Use explicit delete `RESTRICT`, `CASCADE` or
`SET NULL`, default update `NO ACTION` and non-deferrable constraints in the initial
profile. Delete `SET DEFAULT`/`NO ACTION`, nondefault update actions and deferral
cannot be faithfully represented by the legacy model. Relevant system/unowned
incoming constraints are checked, including on the pinned apply transaction;
unrelated tables are left intact. Static migration APIs/storage behavior are
unchanged. See the reference for additional PostgreSQL scope/match limitations.

PostgreSQL planning recognizes the unmodified built-in `TIMESTAMPTZ` /
`timestamp with time zone` alias without rewriting metadata, defaults or hashes.
This prevents false alterations and certification failures after applying runtime
DateTime targets. It requires no data migration. Timestamp without time zone,
precision modifiers, arrays and qualified/quoted type names remain distinct;
this is not general SQL type-name normalization. SQLite comparison is unchanged.

The physical dependency environment is not a record-write API or an authorization
certificate. Hosts must fence external DDL and validate their complete public/policy
revision before future mutations; ORM fingerprints exclude host policy revisions.

Static-to-runtime conversion now rejects an epoch-second default on a DateTime
field with a scoped `UnsupportedDefault` diagnostic. Existing static defaults and
storage remain unchanged; Integer epoch-second defaults remain supported. Canonical
runtime datetime defaults retain RFC3339/native timestamp semantics. No implicit
legacy datetime storage conversion or data migration is performed.

## Runtime string predicates and PostgreSQL binding (0.39.1)

Adopt aligned ORM/macros 0.39.1. No data migration, declaration change, GraphQL
SDL change, or cursor-format change is required. SQLite runtime empty suffixes
now match every non-null string, as empty prefixes already do. SQL NULL remains
unknown under comparisons and NOT; use an explicit null predicate to include it.

`PostgresBackend::normalize_sql` is a complete-statement binding boundary:
SQL using only native `$n` parameters retains each index and repeated identity.
A positive rebasing start adds `start_index - 1` to each native index. Previously,
repeated references were incorrectly allocated separate slots; out-of-order
indices were also renumbered. Callers must supply values for the actual native
slots, rather than relying on occurrence order. Ordinary generated static queries
already compose contiguous native indices and require no call-site edits.

Anonymous or mixed placeholder forms retain legacy occurrence-based rebasing;
mixed forms do not express shared native identity. `SqlDialect::normalize_sql`
retains static-fragment occurrence rebasing, including generated fragments with
arbitrary or restarted native labels. Do not use fragment rebasing on a complete
native statement whose repeated parameters share a bound value.

See [runtime string semantics, executable consumer and backend checks](docs/reference/graphql-orm/runtime-queries.md).
Runtime MSSQL remains unsupported before I/O. This independent query fix does
not implement the proposed runtime migration/mutation/dynamic GraphQL contracts.

## Repository host timestamps (0.39.0)

Adopt aligned ORM/macros 0.39.0. Existing declarations require no edits and keep
legacy seconds-based timestamp writes and GraphQL SDL. Add per-field
`#[graphql_orm(timestamp = "host")]` only to `RepositoryEntity` Integer timestamp
fields whose Rust/physical name is `created_at` or `updated_at`. Writable non-null
create inputs now require that annotated field; update inputs use ordinary
omission/null semantics. Explicit UUID keys and `auto_generated = false` remain
unchanged. Host-managed creation timestamps are writable, including on upsert
conflict: enforce immutability through existing restrictions/policies when needed.

PostgreSQL's parentheses/capitalization when deparsing the known ORM epoch-second
default now compare equivalent, avoiding false column alterations. The expression,
storage units and runtime DateTime rules remain unchanged. Regenerate unapplied
migration plans with this revision so their guarded hashes use the same default
comparison.

The annotation alone changes no physical metadata/default or stored value and
replans to no-op. Database defaults remain available to other writers. Removing
defaults is a separate migration. Runtime Integer/DateTime distinctions are not
changed. Propagate transaction/hook/journal errors or cancel the whole transaction;
catching an inner timeout then returning `Ok` does not guarantee rollback.

See [repository timestamp behavior, examples and backend tests](docs/reference/graphql-orm/repository-only-entities.md#host-managed-integer-timestamps).
This repository capability is independent of the runtime migration/mutation and
dynamic GraphQL contracts; it does not implement those proposed APIs.

## Portable group pages and SQL-visible aggregates (0.38.0)

Adopt aligned ORM/macros 0.38.0. No physical schema, data, GraphQL SDL, existing
SQLite cursor wire format or call-site change is required. `fetch_group_page`
remains supported. PostgreSQL now supports single TEXT/VARCHAR group pages:
`Binary` uses bytewise `C` comparison; the retained `SqliteNoCase` variant uses
explicit ASCII A–Z folding then bytewise original-value tie-breaking. PostgreSQL
page grouping uses exact original text equality, independently of source collation;
ordinary aggregates and SQLite page native equality remain unchanged.

An installed row policy no longer automatically blocks ordinary `fetch`: current
`Unrestricted` or `Complete` visibility is required, with policy and caller filters
intersected before aggregation. Residual/callback-only policies still fail closed.
Generated aggregate-field enums supply an entity TypeId through a provided hook;
handwritten enums remain compatible, but must implement `entity_type_id` correctly
to use complete SQL visibility. Unrestricted visibility needs no token.

Keep trusted authorization/public-revision context in page requests, protect
host cursor envelopes as appropriate, and recheck current policy on every call.
Page cursors do not pin a database snapshot. Concurrent insertion/deletion/renaming
can change a multi-page enumeration; refresh from None when refreshing the dropdown.
MSSQL pages and other scalar/multiple grouping keys reject before I/O. Existing
MSSQL ordinary aggregate/provider/schema gates are unchanged.

See [provider, authorization, wire and consistency contract](docs/reference/graphql-orm/typed-aggregates.md)
and its executable standalone example and owned PostgreSQL test. This capability
is independent of runtime contracts A–D and host-managed repository timestamps.

## Policy-aware transaction counts (0.35.2)

Adopt aligned runtime/macros 0.35.2. No schema, stored-data, public API or
GraphQL SDL migration is required. Transaction-bound repository `count` and
`exists` now accept current explicit `Unrestricted` visibility or a complete
SQL predicate from the installed row policy. Complete predicates are combined
with caller filters before counting on the existing pinned transaction.

Entity authorization remains required. Callback-only/prefilter visibility and
residual query filters under an installed policy remain rejected; no full-row
scan replaces SQL authorization. Re-evaluate verified identity and ownership
for every call and never declare a partial predicate complete. Existing
no-policy behavior is retained. See [repository helpers](docs/reference/graphql-orm/runtime-and-writes.md#repository-helpers).

## Policy-aware typed projections (0.35.1)

Adopt aligned ORM/macros 0.35.1. No schema or GraphQL SDL migration is required.
A global `RowPolicy` no longer automatically rejects every projection: return
`ReadVisibility::Unrestricted` deliberately for an unrestricted model or
`Complete(ReadPredicate::from_filter(...))` for visibility fully expressed in SQL.
This is the same contract supported by complete SQLite group pages. The default
`CallbackOnly` and `Prefilter` continue to reject these reads before query I/O;
there is no full-entity fallback. Entity and selected-field policies remain active.

Re-evaluate visibility for every call using verified identity/current ownership.
A complete predicate replaces the residual callback on these supported paths;
do not label a partial predicate complete. Generated projections carry their
associated entity identity. Existing handwritten `ReadProjection` implementations
remain source compatible and support unrestricted visibility; complete predicates
require the provided `entity_type_id` hook to return that associated entity's
`TypeId`. No public GraphQL roots or private model fields are added.

See [typed projections](docs/reference/graphql-orm/read-projections.md) for a
compiled SQL-free private repository example and exact backend/authorization rules.

## Complete text-group pages (0.35.0)

Adopt aligned ORM/macros 0.35.0 after owner review/publication. This is an additive
static aggregate API with no stored-data, existing aggregate SDL or cursor-format
migration. Loop over `fetch_group_page` with a bounded `group_limit`, one text
grouping field and trusted host context. Existing bounded `fetch` remains unchanged;
use pages when the complete distinct set exceeds the group cap.

The initial page profile is SQLite only, with native group equality and explicit
BINARY/NOCASE ordering. Complete SQL row policies are reapplied before grouping;
residual callbacks cannot authorize groups after aggregation. Include verified
identity/public revision in host context and protect the bounded cursor JSON in
an application-owned envelope when required. This fingerprint is not authority.
See [typed aggregates](docs/reference/graphql-orm/typed-aggregates.md) and its
compiled private consumer example for exact null, collation and continuation rules.

Other backends/scalar/multi-key groups fail before query I/O for this new method.
Existing PostgreSQL/MSSQL aggregate behavior remains unchanged. Computed summaries,
joined/computed source reads and private generated GraphQL views remain independent
work; no runtime-schema/dynamic GraphQL adoption is required for this API.

## 0.33.4 to 0.33.5: authoritative generated relationship access

Adopt runtime and macros 0.33.5 together. Generated relationship resolvers no
longer trust preloaded objects as authoritative targets. Install `Database` in
request/schema data; target tables must remain available for current resolution.
Declare complete tenant/identity bindings. Existing public declarations, SDL and
cursor formats are unchanged; snapshots and cached objects grant no authority.

Entity, current row and selected field policies are enforced at traversal.
A denied nullable target is null with a safe child-path error; missing or moved
ownership targets ordinarily return null without errors. Field policy callbacks
may run during traversal preflight as well as ordinary field resolution; both
receive the actual child field context, including its arguments and alias/path.

For bounded batched pages/counts prefer a `ReadVisibility::Complete` SQL predicate.
Callback-only/prefilter policies on to-many links now require the existing
`AuthorizedScanConfig` on `Database`. The resolver scans bounded candidate batches,
counts authorized rows across the complete result and retains only the requested
visible window. Missing configuration or insufficient budget returns a safe error
instead of a partial count/exhaustion claim. This corrective behavior replaces
previously unchecked relation row policies. Concurrent external DML is not a
snapshot-isolation guarantee across separate read statements.

See [the authoritative relationship example](crates/graphql-orm-macros/fixtures/cross-crate-relations/source-models/examples/authorized_links.rs)
and [the relation contract](docs/reference/graphql-orm/entities-and-relations.md).

## 0.33.3 to 0.33.4: generated cross-crate relationships

Adopt runtime and macros 0.33.4 together from the same reviewed published tag.
Generated relationships no longer call the target entity's private placeholder
helper. Existing declarations compile across crate boundaries without application
calls to ORM implementation helpers, duplicate table entities or dynamic schemas.
No call-site, SDL, cursor or stored-data migration is required. Bind every tenant
and target identity member in `from`/`to`; a link grants no target authority.
See [the runnable external fixture](crates/graphql-orm-macros/fixtures/cross-crate-relations/source-models/examples/cross_crate_links.rs).
This focused fix does not add repository-backed GraphQL adapters or establish the
complete cached/preloaded row and field authorization contract.

## 0.33.2 to 0.33.3: repository aggregate enum compatibility

Pin runtime and macros 0.33.3 to the same reviewed full workspace revision.
`RepositoryEntity` aggregate-field enums now remain plain Rust enums, matching
its documented repository-only surface. No direct `async-graphql` dependency
is needed. Existing Rust aggregate calls, field traits, SQL and authorization
remain unchanged; ordinary GraphQL aggregate SDL is preserved. No stored-data
migration or runtime-schema API adoption is required.

Consumers that deliberately exposed a repository enum through GraphQL should
own a GraphQL wrapper or use `GraphQLEntity`; repository enums do not implement
GraphQL input/output traits. See the compiled
[external consumer fixture](crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/src/lib.rs).

## 0.33.1 to 0.33.2: migration compatibility fixture release identity

Adopt runtime and macros 0.33.2 together from the same published workspace tag.
The patch records the merged external migration compatibility fixture under a
new immutable package source identity. No application call-site, generated-code,
GraphQL SDL, configuration or stored-data changes are required.


## 0.33.0 to 0.33.1: Federation fixture release identity

Adopt runtime and macros 0.33.1 together from the same published workspace tag.
This patch records the Federation fixture's router/Hive dependency update;
no application call-site, generated-code, GraphQL SDL, configuration, or
stored-data changes are required. No data migration is needed.

## 0.32.0 to 0.33.0: authorization-aware pagination

Existing `RowPolicy` implementations keep compiling; generated callback-based
list offsets and totals retain their exact behavior. Implement the optional
`read_visibility` method to opt into complete SQL authorization or partial
prefiltering. Complete decisions must be sufficient without a row callback.

Enable `AuthorizedScanConfig` for bounded callback scans on keyset-enabled
entities, and teach clients to resume `continuation` after empty budget-limited
pages. Scan status deliberately differs from connection `hasNextPage`. Exact
callback totals remain on the legacy path. Store a random cursor encryption
key securely and share it across accepting server instances.

Standalone repository keyset helpers now use the authorized executor. Partial
or callback-only policies use `authorized_scan` to advance through denials.
Legacy repository keyset reads remain fail-closed; callback checks now cover
lookahead/probes as well as returned rows. Transaction keyset helpers retain
their existing behavior and do not yet consume the new predicates.

Regenerate SDL/operation catalogues for the additive `*Scan` fields on
keyset-enabled entities. Those fields inherit `keyset_list` scopes and
assurance, and fail closed until enabled. No database migration is required.
See the [canonical pagination guide](docs/reference/graphql-orm/pagination-migration.md)
for examples, supported relation predicates, backend limits, and consistency.


## 0.31.1 to 0.32.0: Federation entity keys and a shareable `PageInfo`

Adopt runtime and macros 0.32.0 together from one reviewed workspace release.
No Rust call site, persisted schema, or database migration changes.

Two exported-SDL changes affect every consumer, including those that do not use
Federation:

1. `PageInfo` now carries `@shareable`. A host that post-processes, snapshots,
   or fingerprints the SDL text must expect `type PageInfo @shareable {` in
   place of `type PageInfo {`. This directive is what allows two `graphql-orm`
   subgraphs to compose into one supergraph; without it, Federation v2
   composition rejects the duplicate non-entity object field.
2. The first entity that declares `federation_key` enables federation on that
   schema, adding `_entities(representations: [_Any!]!): [_Entity]!` and
   `_service: _Service!` to the *introspected* schema. The federation SDL export
   continues to omit them. Update any fixed introspection snapshot, schema
   fingerprint, or root-field count assertion accordingly. Schemas that declare
   no key are unaffected.

To opt an entity into the supergraph, add `federation_key` to its
`#[graphql_entity(...)]` attribute. The entity must also derive
`GraphQLOperations`; it now fails to compile otherwise rather than silently
exporting no key. Consumers with hand-written resolver types for an entity
should note that the generated `@key` is built from the generated resolver's
argument names, so a hand-written `{Entity}Queries` replacement will not export
a key.

Declare reference fields that point at a foreign entity as **nullable**. A
failed entity fetch produces `null` for the reference, and GraphQL null
propagation decides the rest: a nullable reference degrades to `null` and the
referencing subgraph's own fields still reach the caller, while a non-null
reference hung off a generated connection has no nullable ancestor before
`data` and costs the caller the whole response.

Before adding a key, confirm the entity's rows are safe to expose to every
subgraph that joins it. The router's authorization contract covers root fields
only and deliberately skips `_entities`, so a joined field is authorized solely
by the owning subgraph's entity policy, row policy, scope requirement, or
`auth = "required"`.

## 0.31.0 to 0.31.1: patched SQL Server XML dependency

Adopt runtime and macros 0.31.1 together from one reviewed workspace release.
The MSSQL notification parser now uses quick-xml 0.41. Existing input-size,
notification-shape and separately authorized Broker-connection requirements
remain in place. No Rust call-site, GraphQL SDL, persisted-schema or database
migration is required. This dependency update does not grant queue consumption,
DML or schema-management authority to an existing read-only connection.

## 0.30.0 to 0.31.0: native SQL Server query notifications

Adopt the runtime and macros 0.31.0 together from one reviewed full Git revision.
The optional MSSQL notification API uses a separately authorized connection;
existing read-only pools do not acquire queue-consumption or write capability.
External database owners must provision Broker and its dedicated queue/service.
See [SQL Server notification setup](docs/reference/graphql-orm/mssql.md#query-notifications).
No managed schema migration is required or performed. Notifications are one-shot
invalidation hints, not durable row-change events; applications own re-registration,
catch-up, permissions, and reconnect/resynchronization behavior.

## 0.29.0 to 0.30.0: checked calendar-date filters

Adopt `graphql-orm` and `graphql-orm-macros` 0.30.0 together from one reviewed
full Git revision. Backend-neutral semantic consumers must adopt
`graphql-orm-operation-catalog` 0.4.0 from that revision. The AI tool-profile
package remains 0.10.2; rebuild its generated capabilities against the new
semantic catalogue.

Generated calendar predicates now use half-open ranges over the persisted
column: today is `[today, tomorrow)`, future begins at tomorrow, the past ends
at today, `RecentDays(N)` includes exactly today plus `N - 1` preceding dates,
and `WithinDays(N)` includes exactly `N` dates beginning today. `LteRelative`
is inclusive through its selected calendar date by rendering an exclusive
next-date upper bound. Ordinary `Eq`, `Ne`, `Lt`, `Lte`, `Gt`, and `Gte`
continue binding the supplied value without normalization.

`DateRangeInput.start` and `DateRangeInput.end` are now required `String`
fields. Update Rust struct literals that supplied `Option<String>`. Both bounds
must be parseable date/timestamp values and start must not follow end.
`RecentDays` and `WithinDays` accept 1 through 36,600; signed relative offsets
accept -36,600 through 36,600. Generated query execution validates recursively
and returns `INVALID_INPUT` before database work. Handwritten `DatabaseFilter`
implementations remain source compatible because `validate` has a permissive
default; implement it when a custom filter has structured invariants.

The meaning of today remains backend-owned and is now internally consistent:
PostgreSQL uses session `CURRENT_DATE`, SQL Server uses the server-local date
from `GETDATE()`, and SQLite uses UTC `date('now')`. SQLite's in-memory spatial
fallback captures one UTC date per filtering pass and does not push
clock-dependent predicates into its SQL prefilter. This release does not add a
cross-backend application-timezone setting or normalize exact input strings.
A future typed date/time input and context-bound calendar-clock design requires
separate compatibility review.

Date semantic metadata and catalogue fingerprints change. Rebuild reviewed
catalogues, automatic query capabilities, and retained bindings. There is no
database schema, stored-data, backup, or migration-history change.

## 0.27.0 to 0.29.0: computed/relation ordering and complex relation composition

Computed fields can join generated ordering without a handwritten query:

```rust
#[graphql_orm(order_expression(
    name = "Duration",
    expression = "COALESCE(finished_at, :as_of) - started_at",
    parameters = "duration_order_parameters"
))]
```

The expression is trusted, backend-specific server configuration. Requests
continue to supply only `ASC` or `DESC`. For `:named` binds, the configured
synchronous function receives `&async_graphql::Context` and returns
`async_graphql::Result<OrderExpressionParameters>`; use `SqlValue` variants for
the values. Raw `?`, `$n`, and `@Pn` placeholders are rejected. Programmatic
queries using a contextual expression call `EntityQuery::order_by_with_context`.
Generated pagination now adds missing primary-key columns as ascending
tie-breakers. Review the resulting query plan and add an expression/index
strategy where the selected backend supports one.

`SortExpression` now carries bound `values` so query rendering can number
order-expression placeholders after filter placeholders on every backend.
Handwritten parameter-free sort construction should use
`SortExpression::unbound("column ASC")`; handwritten struct literals must add
`values: Vec::new()`.

A readable relation can expose a server-generated count order without a
projection:

```rust
#[relation(
    target = "StaffAssignment",
    from = "id",
    to = "policy_id",
    multiple,
    order_aggregate(name = "AssignedStaffCount", aggregate = "count")
)]
```

Requests again supply only `ASC` or `DESC`. The macro obtains the target table
from its entity type and generates the correlated count from the declared key
mapping. The initial contract supports `count` on unconditional, readable
relations; conditional relations fail compilation when combined with
`order_aggregate`.

For a relation entity that already has a handwritten `#[ComplexObject]` impl,
add `#[graphql_orm(compose_complex_object)]` to the entity and replace that
impl attribute with `#[graphql_complex_object]`. Handwritten methods remain
unchanged; generated relations retain their batching and arguments. Do not
leave both complex-object attributes on the impl.

There is no table, column, constraint, or stored-data migration. Regenerate
and review GraphQL SDL and semantic/capability fingerprints when adding a new
ordering field. Review the query plan for correlated relation counts and add
indexes on the target key columns used by frequently sorted relations.

## 0.26.0 to 0.27.0: conditional polymorphic relationships

Adopt `graphql-orm` and `graphql-orm-macros` 0.27.0 together from one reviewed
full Git revision. Existing relation declarations need no change.

Externally managed tables that reuse one reference column for multiple target
types can declare a fixed source discriminator:

```rust
#[relation(
    target = "Document",
    from = "reference_id",
    to = "id",
    source_condition(field = "reference_kind", equals = 1),
    emit_fk = false
)]
pub document: Option<Document>,
```

The inverse collection can constrain the target rows:

```rust
#[relation(
    target = "Activity",
    from = "id",
    to = "reference_id",
    target_condition(column = "reference_kind", equals = 1),
    multiple,
    emit_fk = false
)]
pub activity: Vec<Activity>,
```

Condition values accept string, integer, float, and boolean literals. A source
condition must name a persisted scalar Rust field with a matching type; target
conditions name a physical target column and use the same trusted, bound-value
SQL path as generated filters. Conditional relationships always require
`emit_fk = false` because the discriminator makes the join conditional rather
than a physical foreign-key contract.

No database, table, column, constraint, or stored-data migration is required.
After adding conditional relationships, regenerate the semantic catalogue and
any derived capability fingerprints so the new selectable graph is reviewed
and deployed atomically.

## 0.25.1 to 0.26.0: agql-auth 0.18.0 alignment

Git consumers using `auth-agql` must align direct `agql-auth` dependencies to
0.18.0 at merged revision `527d15d28e3c295a6f6b5e6d74559a7aecdc1322`.
Adopt aligned `graphql-orm` and `graphql-orm-macros` 0.26.0 from the same
reviewed monorepo revision. The bridge remains projection-only. Hosts adopting
typed authorization-role grants follow the upstream issuer/resource-server
rollout; no ORM schema, migration, generated-code, database-policy, or
stored-data change is required.

## 0.25.0 to 0.25.1: agql-auth 0.17.1 alignment

Git consumers using `auth-agql` must align direct `agql-auth` dependencies to
0.17.1 at merged revision `b792241b7d9bb46bad81dde4534ae5b39857f614`.
Adopt aligned `graphql-orm` and `graphql-orm-macros` 0.25.1 from the same
reviewed monorepo revision. The upstream patch makes default session-context
serialization compact; no API, schema, migration, generated-code,
database-policy, or stored-data change is required in this workspace.

## 0.24.0 to 0.25.0: agql-auth 0.17 alignment

Git consumers using `auth-agql` must align direct `agql-auth` dependencies to
0.17.0 at merged revision `82650a447f3b6875160254fa1293b3a0e305c224`.
The role-scope expansion contract is additive and opt-in; the ORM bridge does
not fetch catalogues or change authorization decisions. No schema, migration,
generated code, or database-policy change is required. Adopt aligned
`graphql-orm` and `graphql-orm-macros` 0.25.0 from the same reviewed revision.

## 0.23.0 to 0.24.0: agql-auth 0.16 alignment

Hosts enabling `auth-agql` or depending directly on `agql-auth` must align to
version 0.16.0 at merged revision
`3bc38cd94794f1e868a9cc3a5551047b95a32105`. Adopt `graphql-orm` and the
aligned `graphql-orm-macros` 0.24.0 from the same reviewed monorepo revision.
This retains one public auth type universe. Version 0.16 adds
consumer-supplied exact-only hierarchical matcher policy; the ORM bridge
remains projection-only and does not select or install that policy. No
database, schema, GraphQL SDL, generated-code, token, backup, or stored-data
migration is required.

## 0.22.1 to 0.23.0: explicit collection-bound semantics

Update `graphql-orm`, `graphql-orm-macros`, and
`graphql-orm-operation-catalog` together to 0.23.0 / 0.3.0 at one reviewed
full Git revision. No database, table, column, constraint, or AI
schema-module migration is required.

Regenerate semantic catalogues and automatic query capabilities. Many
relationships now require `collection_bound`. Old catalogue version `1`
payloads are rejected. Server-fixed object lists no longer accept a
model-selected `maximumItems` and use the declared semantic ceiling in
disclosure.

## 0.22.0 to 0.22.1: relationship semantic argument correction

Update `graphql-orm` and `graphql-orm-macros` together to 0.22.1 at one
reviewed full Git revision. No resolver SDL, database schema, table, column,
constraint, data, or AI schema-module migration is required.

The canonical semantic catalogue now describes a generated to-many
relationship's nullable ordering argument as `ChildOrderByInput`, matching the
resolver SDL. Root list queries continue to describe and accept
`[ChildOrderByInput!]` as before. Regenerate semantic catalogues and automatic
query capability fingerprints after adoption; do not preserve fingerprints
derived from the incorrect relationship argument shape.

`AiGraphqlQueryCapabilityCatalog::compile` remains strict. Any future
catalogue/SDL type, nullability, list-item, name, or argument-set mismatch
continues to fail readiness.

## 0.22.0 canonical GraphQL semantic catalogue

Update `graphql-orm` and `graphql-orm-macros` together to 0.22.0 at one
reviewed full Git revision. Backend-neutral consumers use
`graphql-orm-operation-catalog` 0.2.0.

Existing entity and operation declarations continue to compile. The former
minimal borrowed `GraphqlEntitySemanticMetadata` values are now owned,
serializable catalogue values with typed field capabilities. Code constructing
those structs directly must migrate to the expanded fields or, preferably,
consume `Entity::graphql_semantic_metadata()` and
`graphql_orm_semantic_catalog()`.

Descriptions resolve from an explicit semantic `description`, then Rust doc
comments, then a stable humanized public-name fallback. Use the same doc text
when an entity also derives `async_graphql::SimpleObject`; that external derive
owns its SDL field documentation. Sensitive fields are now structurally
`Secret` and `NeverExport`, while `private` and `read = false` fields remain
absent.

Handwritten resolver roots can publish canonical metadata beside their
`async-graphql` impl:

```rust,ignore
#[derive(Default)]
struct HealthQueries;

#[graphql_orm_custom_operations(kind = "query", authorization = true)]
#[async_graphql::Object]
impl HealthQueries {
    /// Returns current service health.
    #[graphql_orm(
        result_classification = "public",
        result_export = "exportable"
    )]
    async fn health(&self) -> String {
        "ok".to_owned()
    }
}

schema_roots! {
    entities: [Account],
    described_query_types: [HealthQueries],
}
```

The `described_query_types`, `described_mutation_types`, and
`described_subscription_types` lists replace the need to repeat one root in
both an `extra_*` list and `semantic_custom_operations`; direct
`GraphQLSemanticObject` results are collected as well. Legacy lists remain
supported, but the same root cannot use both forms.

Unclassified custom scalar/enum leaves now fail safe as
`Secret`/`NeverExport` and therefore do not become automatic AI capabilities.
Declare `result_classification` and `result_export` together only for a
reviewed exportable root. An exportable scalar/enum list additionally requires
positive `result_maximum_items`; custom enum/scalar wrappers may need the
explicit `result_type_kind`. Object results continue deriving authority-neutral
disclosure from selected fields, and a root declaration can only tighten it.

Generated aggregate operation fingerprints now include their owning public
entity identity. Compiled aggregate plan, projection, and disclosure
fingerprints also bind exact selected grouping fields and metric
field/operator pairs. Refresh any stored host allowlists; a response with
unselected or drifted aggregate identities is rejected before provider egress.

The catalogue and per-operation semantic fingerprints intentionally change
when public descriptions, fields, relationships, capabilities, classifications
or exposure change. They are drift evidence, never authority. There is no
database, stored-data, backup, migration-history, or AI schema-module migration.
Generated broadcast subscriptions are now described as best-effort and are not
eligible for a durable wait. A custom subscription may declare bounded
`replay_then_live` semantics only beside its resolver; downstream runtimes must
still register and verify the matching authoritative cursor/watermark source.

Public mutations now carry a closed AI execution classification in semantic
catalogue v1. Existing declarations remain `Prohibited` and therefore acquire
no AI capability. Opt in generated categories with `ai_mutations(...)`, or add
`ai_execution = "automatic" | "approval_required" | "prohibited"` to an
existing mutation `graphql_orm_custom_operations` declaration. This changes
semantic fingerprints only. It does not change SDL, resolver behavior,
database schema, stored rows, or authorization.

Typed grouped aggregates are additive. Every `GraphQLEntity` now emits a
closed aggregate-field enum for repository use, while no GraphQL root is added
unless the entity declares `aggregate = true`. That opt-in changes the schema
and operation catalogue by adding the bounded aggregate query. Decimal fields
must declare `#[graphql_orm(decimal(precision = P, scale = S))]`; SQLite stores
their exact scaled integer representation while PostgreSQL and SQL Server use
native fixed-precision types. Adding a new Decimal column is an ordinary
application schema migration, but adopting this release without declaring one
requires no database or stored-data migration. Logical backups containing a
Decimal field use the new exact Decimal value kind and include precision/scale
in the schema hash. Produce and restore those backups with 0.22.0 or later;
string-typed Decimal backup values are rejected instead of being coerced.

SQL Server compatibility constructors remain physically read-only, so existing
MSSQL applications acquire no write authority after upgrading. To adopt DML
against an externally managed schema, change both boundaries explicitly:

```rust,ignore
#[repository_entity(
    backend = "mssql",
    table = "dbo.WorkItems",
    schema_policy = "external_writable"
)]
struct WorkItem { /* externally reviewed columns */ }

let database = Database::<MssqlBackend>
    ::connect_ado_external_writable(connection_string)
    .await?;
```

Review the database principal, table contract, entity/field/row policies,
upsert key uniqueness, and transaction behavior before adopting this mode.
`ExternalWritable` permits generated row DML only. It does not enable SQL
Server migration planning/application, managed RLS/search structures,
runtime-schema writes, or backup/restore. No ORM schema or stored-data
migration is performed by this release.

Generated writes in 0.22.0 also tighten the row-authorization boundary without changing public
top-level method signatures. Update/delete/bulk helpers now evaluate the locked preimage and perform
exact-key DML on one pinned transaction. Generated upsert helpers select state-machine isolation.
If application code calls `MutationContext::upsert` inside an explicit
`Database::transaction(TransactionMode::Default, ...)`, change that transaction to
`TransactionMode::StateMachine` and retry the complete callback when a serialization conflict is
classified retryable. Default-mode upserts now fail closed instead of accepting an unfenced
absent-key decision. No table, column, stored-data, migration-history, backup, GraphQL SDL, or AI
schema-module migration is required.

## 0.21.1 agql-auth 0.15 session-bound delegation alignment

Update `graphql-orm` and `graphql-orm-macros` together to 0.21.1 at one
reviewed full Git revision. Applications that directly use `agql-auth` must
align that dependency to the same exact source:

```toml
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", rev = "e841ffd382082ad7419be259fe957f949b956ff7", version = "0.15.0" }
```

The optional `auth-agql` bridge remains a one-way principal projection and
requires no source changes. Version 0.15 adds upstream session-bound,
access-token-only delegation types; it does not make the ORM an issuer or
session authority. Hosts using those APIs must install an authoritative
`VerifiedActiveUserSessionResolver`, narrow current authority, and retain the
exact actor, resource, correlation, and registered-operation bindings.

There is no ORM schema, GraphQL SDL, migration-history, backup, stored-data, or
agql-auth database migration. Remove older direct pins rather than resolving
two auth package/type universes.

## 0.21.0 Backend-neutral operation catalog and companion derives

Update `graphql-orm` and `graphql-orm-macros` together to 0.21.0 at one
reviewed full Git revision. Existing imports of resolver-operation metadata
through `graphql_orm::graphql::orm` remain source compatible; the canonical
types now live in `graphql-orm-operation-catalog` 0.1.0 so database-neutral
manifest producers do not have to select an ORM backend.

Reusable companion packages exposing mutually exclusive `sqlite`, `postgres`,
and `mssql` features may replace `#[graphql_entity(...)]` with
`#[backend_selected_graphql_entity(...)]`. The latter accepts the same entity
arguments except `backend` and supplies the backend from the companion crate's
own selected feature. Application entities with a fixed backend should keep
the ordinary explicit `backend = "..."` declaration.

No database, SDL, migration-history, backup, operation fingerprint, or stored
data migration is required.

## 0.20.0 Semantic entity descriptions

Update `graphql-orm` and `graphql-orm-macros` together to 0.20.0 at one
reviewed full Git revision. Existing derives and handwritten `Entity`
implementations require no source changes.

Hosts may add a public entity description and public field descriptions:

```rust
#[graphql_entity(
    table = "records",
    plural = "Records",
    description = "Records visible in reviewed application workflows"
)]
struct Record {
    #[primary_key]
    #[graphql_orm(description = "Stable public record identity")]
    id: String,
}
```

Read the resulting `GraphqlEntitySemanticMetadata` through
`Entity::graphql_semantic_metadata`. The derive omits private/non-readable
fields and never includes physical column identities or policy keys. This is
descriptive metadata only; consumers must still explicitly select fields,
assign disclosure classification, and perform ordinary authorization.

There is no database, SDL, backup, or stored-data migration. The optional
`router-protocol` feature now resolves protocol crate 0.2.0, whose wire payload
remains protocol major 1.

## 0.19.0 Compound foreign keys and directional indexes

Update `graphql-orm` and `graphql-orm-macros` together to 0.19.0 at one
reviewed full Git revision. Existing entity declarations using the original
`index = "a,b"` and single-column relation forms remain source compatible.

Compound managed relations now produce one physical constraint. Declare the
ordered Rust source fields and ordered target database columns together:

```rust
#[relation(
    target = "SnapshotRecord",
    from = ["provider", "tenant_key", "generation"],
    to = ["provider", "tenant_key", "generation"],
    on_delete = "cascade"
)]
snapshot: Option<SnapshotRecord>,
```

Every target member must be present in the same managed schema model and the
ordered tuple must be an exact primary key, composite unique declaration, or
unconditional unique index. Source members are translated through
`#[graphql_orm(db_column = "...")]`; target members remain database-column
names. A migration that previously passed only the referencing entity may need
to include the referenced repository/schema entity so uniqueness and type
compatibility can be proven.

Named directional ordinary indexes use the nested form:

```rust
#[graphql_entity(index(
    name = "idx_snapshot_latest",
    columns = ["provider", "tenant_key", "generation"],
    directions = ["asc", "asc", "desc"]
))]
```

Omitting `directions` keeps every column ascending. If supplied, its arity must
match `columns`. Existing string index declarations keep generated names.

Use `#[graphql_orm(min_exclusive = 0)]` or
`#[graphql_orm(max_exclusive = 100)]` when an existing check uses strict
comparison. Use `#[graphql_orm(default = false)]` only to suppress the
conventional implicit default inferred for a `created_at` or `updated_at`
field; SQL-expression defaults remain string literals.

### Public model migration

`ForeignKeyModel` now owns `column_pairs: Vec<ForeignKeyColumnPairModel>` and
an optional observed `constraint_name` instead of scalar source/target column
fields. Replace single-column struct literals with
`ForeignKeyModel::single(...)`; compound callers construct the ordered pair
list. `IndexDef` adds `column_directions`; constructor-based callers need no
change, while direct struct literals must supply `&[]` for compatibility or an
exact direction slice. These changes are why the aligned packages advance as
a pre-1.0 minor version.

### Existing database adoption

There is no adoption flag or unchecked migration-history bypass. Plan against
the live database normally. When introspected primary keys, unique keys,
foreign-key members/order/target/delete action, ordinary index name/order, and
supported check expressions are semantically equal, the plan is empty and
ordinary `apply_migration` records the version without DDL. This preserves all
rows and constraints.

The check comparator intentionally accepts only a closed grammar of simple
identifiers, literals, comparison operators, parentheses, and commas. It
normalizes physical names, whitespace, keyword case, redundant balanced outer
parentheses, and quoting of ordinary lowercase portable identifiers;
case-sensitive quoted identifiers remain distinct. It does not infer
mathematical or type-affinity equivalence. Unsupported, weakened, partial,
reordered, or ambiguous contracts remain explicit drift and must not be
force-adopted.

Schema hashes now bind ordered compound-FK members and effective index
directions while treating physical check/FK names as non-semantic. Refresh any
reviewed hash expectations and run an empty replan plus backend integrity
checks before removing a legacy migration path. No application row or GraphQL
schema migration is otherwise required.

## 0.18.0 Router metadata and `agql-auth` 0.14 scopes

Update `graphql-orm` and `graphql-orm-macros` together to 0.18.0 at one
reviewed full Git revision. Generated operation authorization declarations,
the optional `router-protocol` export, and standard Federation authorization
metadata are additive source and schema capabilities, but generated operation
authorization fingerprints advance to version 2. Rebuild generated code and
review any consumer that persists or compares those fingerprints.

Existing databases, schema modules, RLS policy, and stored application data do
not require migration. Router adoption is separately opt-in and requires each
host to expose its finished SDL and matching protocol descriptor.

The workspace now pins `agql-auth` 0.14.0 at exact revision
`413fda3435f060604cd653c11e2cc18a668aace1`:

```toml
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", rev = "413fda3435f060604cd653c11e2cc18a668aace1", version = "0.14.0" }
```

Update any direct host dependency at the same time so Cargo resolves one
package and type universe. The `auth-agql` bridge continues consuming the
normalized `AuthPrincipal::scopes()` vector, so it has no Rust API, GraphQL,
database, RLS, or stored-data migration.

The upstream access-token wire default is breaking: newly issued access JWTs
use the OAuth space-delimited `scope` string instead of the pre-0.14 `scopes`
array. Upstream validators accept both by default for a bounded transition;
direct JWT decoders must add standard-claim support before issuer cutover.
Purpose tokens retain their separate `scopes` array. Follow the upstream
0.13-to-0.14 staged migration and wait for the maximum old access-token TTL
plus validation leeway before rejecting legacy claims.

## 0.17.0 Provider-Neutral Operation Assurance

Update `graphql-orm` and `graphql-orm-macros` together to 0.17.0 at the final
reviewed full revision. If `auth-agql` is enabled, match its exact upstream
dependency:

```toml
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", rev = "413fda3435f060604cd653c11e2cc18a668aace1", version = "0.14.0" }
```

This release is additive and opt-in. Existing schemas install no assurance
enforcement, apply no interactive-mutation default, and retain their current
query, mutation, subscription, authorization, RLS, SDL, and database behavior.

### Before

```rust
let schema = schema_builder(database).finish();
```

### After: staged adoption

First build a compatibility registry and audit the current mutation surface:

```rust
let registry = OperationAssuranceRegistry::builder(
    graphql_orm_operation_catalog(),
    AssuranceSchemaConfig::legacy(),
).build()?;
let audit = registry.audit();
```

Then register custom resolver fields, assign machine/service/safety-teardown
actor classes, and give every exposed mutation either `require(...)` or
`exempt(...)`. Custom fields use `DeclaredAssuranceGuard`; generated fields
already call the runtime hook.

Next configure an interactive default while strict mode remains off:

```rust
let config = AssuranceSchemaConfig::legacy()
    .with_default_interactive_mutation_policy("interactive.recent-auth")?;
```

Install `AssuranceEnforcement` only after the upstream policy set, injected
clock, accepted request principal, error handling, and step-up UX are ready.
With `auth-agql`, use `AgqlAssuranceEvaluator`. Server denials expose lowercase
extension key `code` with `STEP_UP_REQUIRED`, `UNAUTHENTICATED`, or
`FORBIDDEN`.

Finally enable `with_strict_mutation_classification(true)` and make
`ensure_complete()` or `audit().assert_complete()` a CI gate. Export
`manifest()` for client codegen and `schema_metadata()` for directive-aware
schema tooling. Both are descriptive/advisory; keep server enforcement enabled
for every protected operation.

### Compatibility and rollback

No database, stored-data, GraphQL-data, session, or token migration is needed.
Queries and subscriptions never inherit the mutation default. Ordinary
resolver auth and authorization continue independently; an assurance exemption
does not grant authority. Machine/API-token principals cannot satisfy a user
session requirement through the upstream evaluator and must be explicitly
classified according to server policy.

To roll back, remove `AssuranceEnforcement` from schema data and restore
`AssuranceSchemaConfig::legacy()` (or stop building the registry), then return
both ORM crates to the prior reviewed exact revision. Remove the client
manifest expectation at the same time. Existing database rows and
authentication sessions require no rewrite.

See [Operation assurance](docs/architecture/operation-assurance.md) for the complete API and
trust boundaries.

## 0.16.0 Generated Resolver Operation Metadata

Update both Git-only graphql-orm crates to 0.16.0 at the reviewed final full
revision. Existing entity declarations and `schema_roots!` blocks require no
source changes. `GraphQLOperations` now also implements
`GraphqlOperationMetadata`; `schema_roots!` adds the
`graphql_orm_operation_catalog()` helper in the same module as its existing
schema helpers.

Hosts that previously reconstructed resolver names from entity/plural names
should instead select the exact generated root coordinate from the catalog:

```rust
let operation = graphql_orm_operation_catalog()
    .resolve(GraphqlOperationKind::Query, "users")
    .expect("reviewed generated operation remains exposed");
let generated_fingerprint = operation.fingerprint();
```

Do not substitute the catalog fingerprint for a complete finished-schema
registry fingerprint when a host composes custom roots. Do not treat metadata
discovery as registration, enablement, authorization, or disclosure approval.
Document operation names, server-authored document hashes, selected result
fields, model-facing argument schemas, static disclosure contracts, and
current resolver/RLS authorization remain separate host/downstream bindings.

Generated mutation none/allowlist/denylist policy now appears as
`is_exposed() == false` on the corresponding resolved descriptors; repository
writes and generated subscription behavior are unchanged. A root-level
`external_read_only` policy also reports derive-generated mutation and
subscription descriptors as unexposed because that root composes neither
operation type. Private typed read projections remain repository-only and do
not add descriptors or change generated GraphQL operation fingerprints. List
categories describe connection shape rather than a fixed runtime bound; retain
explicit host/tool output limits because `PaginationConfig` remains
configurable.

This is an additive pre-1.0 public API/generated-code release. Existing
GraphQL SDL, resolver execution, database schema, stored data, migrations,
backups, authorization, and RLS behavior do not change. No database or data
migration is required. Refresh lockfiles, update both aligned versions, and
rerun backend, naming, schema exposure, and consumer fingerprint gates.

## 0.15.0 Exact Bounded-Mutation Sentinels

Update both Git-only graphql-orm crates to 0.15.0 at the final reviewed full
revision. The release corrects generated single/composite bounded update and
delete plus retention purge when `MutationLimit` is 100 or greater. Hosts do
not need to change existing calls: the generated implementation now obtains
the exact `maximum + 1` sentinel without applying the public 100-row page cap.

Public GraphQL, connection, repository, runtime-query, and `PageInput` limits
are unchanged. No uncapped public read surface is introduced. Residual or
in-memory filters on bounded mutations now return stable `INVALID_INPUT`
before mutation; replace such filters with a completely database-rendered
predicate. Database-renderable filters preserve exact all-or-nothing results,
and any selected-versus-affected cardinality change fails the transaction.

No database schema, stored data, GraphQL SDL, entity declaration, or backend
migration is required. Refresh lockfiles and rerun high-ceiling overflow,
authorization/RLS, event, rollback, and restart checks. The optional agql-auth
bridge remains released 0.12.0 at exact revision
`3f3b0c5365adfbe436514a681d977b600991b797`; matching direct host dependencies
must keep that exact version and full revision so one type universe resolves.

## 0.14.0 agql-auth 0.12 Bridge Alignment

Update both Git-only graphql-orm crates to 0.14.0 at the final reviewed full
revision. When the host also depends on agql-auth, its dependency must match the
bridge exactly:

```toml
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", rev = "3f3b0c5365adfbe436514a681d977b600991b797", version = "0.12.0" }
```

This yields one agql-auth package/type universe. Do not use a branch, local path
override, abbreviated revision, or different version requirement.

The public converter functions and exact identity/authorization-context
mappings remain available. The bridge now omits malformed or structurally
inconsistent session assurance, and it copies only the documented string
`policy_version` from `AccessTokenMetadata.additional`. Hosts that read other
custom values from `AuthSubject.claims.additional` must move those values into
an explicit application-owned request context. This observable narrowing makes
the release 0.14.0; macro syntax/output is unchanged, but the companion version
advances under the aligned release policy.

Direct agql-auth users must separately follow its 0.10→0.12 migration. The 0.11
`AuthRateLimitStore` contract is atomic and versioned; graphql-orm supplies no
implementation and requires no ORM schema migration. The 0.12 typed
`EssentialAcrs` request and `matched_acrs` outcome remain provider evidence.
They do not enter the bridge or imply local MFA. Only a host-constructed,
session-bound, structurally consistent `SessionAssurance` can populate
`AuthAssurance`/`DbAuthContext.assurance`; the exact `MfaAcceptance` decision is
retained. A mapped `microsoft-entra/acrs/c1` context remains distinct from
standard scalar `acr`, AMR, roles, scopes, tenant, and `policy_version`.

No database schema, stored data, GraphQL SDL, generated code, resolver naming,
or backend migration is required. Refresh lockfiles and inspect `cargo tree` for
one agql-auth revision before running the host's authorization and live restart
gates; compilation alone is not endpoint approval.

## 0.13.0 Runtime Relation Batching and PostgreSQL Constraint Introspection

This Git-only 0.13.0 release contains two coordinated prompts: validated
runtime relation batching and PostgreSQL constraint-index upgrade idempotency.
Update both aligned crates to the final reviewed full revision.

Existing static entities, GraphQL SDL, generated repositories/relations,
transactions, authorization/RLS, top-level `gormrq1` and legacy cursors,
`RuntimeRecord`, serialized `RuntimeSchema`, third-party backend traits, and
stored rows require no source or data migration. The new `gormrr1` relation
cursor is accepted only by `RuntimeRelationBatchRequest` and must not be
translated to or from another cursor family. Hosts opting into runtime
relations replace host SQL/N+1 loaders with an anchored parent read followed by
explicit bounded relation layers. Request next-layer relation keys with
`runtime_relation_batch_request_with_relation_keys`, then pass the opaque
anchors returned by `RuntimeRelationBatch::relation_parents` into the next
batch request; see
[Validated runtime relation batching](docs/reference/graphql-orm/runtime-relations.md).

PostgreSQL operators should replan a complete generated target before rollout.
Constraint-owned PRIMARY KEY/UNIQUE backing indexes are now classified as
constraints, not secondary indexes, so an unchanged schema or table-additive
upgrade produces no `DropIndex` for them. Composite UNIQUE metadata now renders
as `UNIQUE (a, b)` during new table creation and introspects in key order. No
existing constraint or index is rewritten merely by upgrading the library. If
an older managed table was created without a declared composite UNIQUE because
of the former CREATE TABLE omission, the live/target mismatch is real and
requires an explicit reviewed migration; do not mark an old module version as
newly applied.

MSSQL static reads remain compatible and runtime relation execution returns
`unsupported_backend` before I/O. No macros or entity declarations changed;
the macros version advances only under the repository's aligned Git-only
release policy.

## 0.12.0 Runtime Query Execution

Update both Git-only crates to 0.12.0 at the reviewed full Git revision. This
is an additive pre-1.0 public runtime API release; macro declaration/generated
behavior is unchanged, but the aligned companion version advances under the
repository release policy.

Existing static entities, repository/GraphQL reads, mutations, transactions,
authorization, RLS, keyset cursors, third-party `OrmBackend` implementations,
database schemas, and serialized `RuntimeSchema` documents need no source,
cursor, schema, or data migration. The new `gormrq1` cursor is used only by
`RuntimeReadRequest` and is intentionally distinct from static/legacy cursors.
Do not translate or accept offset cursors on this boundary.

Runtime-schema hosts may replace their own SQL/filter/order/row-decoding layer
with the validated constructors on `ValidatedRuntimeSchema` and
`Database::execute_runtime_read`. Re-resolve handles after schema activation;
old fingerprints fail closed. Hosts must compile authorization constraints to
a second `RuntimePredicate` and structurally combine it with the application
predicate. See [Runtime queries](docs/reference/graphql-orm/runtime-queries.md).

No migration is required. MSSQL continues to compile and retain static reads,
but returns the stable unsupported capability for runtime execution.

## 0.11.0 Repository-Only Entities

Update both Git-only crates to 0.11.0 at the final reviewed full Git revision.
This is an additive pre-1.0 minor release: it introduces the public
`RepositoryEntity` derive, `#[repository_entity(...)]` declaration attribute,
`RepositoryQuery`, repository field-policy callbacks, and generated ordinary
Rust DTO/input types. Existing `GraphQLEntity`, `GraphQLSchemaEntity`,
`GraphQLOperations`, schema roots, SDL, and stored schemas remain compatible.

To move a persisted entity out of GraphQL entirely, replace its GraphQL derives
and `#[graphql_entity(...)]` with:

```rust
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "credentials",
    plural = "Credentials",
    default_sort = "username ASC"
)]
struct Credential { /* unchanged persisted fields */ }
```

Remove it from `schema_roots!`; attempting to register it is now an intentional
compile error. Replace the legacy pool-bound list builder with
`Credential::query(&database)`. Primary/unique lookups, writes, projections,
and `MutationContext` calls retain their generated names. Repository-only
create/update types include writable private fields, so host duplicate DTOs are
not needed.

Existing `FieldPolicy` implementations compile unchanged. Fields with no
declared key keep the existing repository decision; a field carrying
`read_policy`/`write_policy` is denied by the new default repository callbacks
until the provider implements `can_read_repository_field` and/or
`can_write_repository_field`. This deliberate fail-closed behavior prevents a
missing GraphQL `Context` from becoming authority.

Changing only the generation surface does not alter `SchemaModel`, migration
plans, schema/module fingerprints, backup descriptors, tables, indexes,
constraints, RLS, or data. No data or schema migration is required. Sensitive
repository mutation-hook snapshots/events are more restrictive by design: hook
state is redacted and cannot be downcast to the original entity, and change
events omit the entity payload when the declaration has a sensitive field.
Before-write input hooks remain typed and can deliberately transform the value.
See [Repository-only entities](docs/reference/graphql-orm/repository-only-entities.md).

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
[Runtime values, records, handles, and row decoding](docs/reference/graphql-orm/runtime-records.md).

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
[Bounded append-only retention maintenance](docs/operations/runbooks/retention-maintenance.md).

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
[Testing and verification](docs/development/testing.md).

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
in [Federation operation roots](docs/reference/graphql-orm/federation.md) before promotion.

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
fields. See [Typed Composite-Key and Bounded Mutations](docs/reference/graphql-orm/composite-mutations.md).

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
No GraphQL query or DTO is added. See [Typed Read Projections](docs/reference/graphql-orm/read-projections.md).

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

- No JWT, OIDC, cookie, wildcard, or application-specific scope logic was added to `graphql-orm`.
- PostgreSQL RLS helper functions still use exact scope matching.
- The current `auth-agql` feature targets `agql-auth` 0.16.0 at revision
  `3bc38cd94794f1e868a9cc3a5551047b95a32105`; earlier release sections above
  retain their historical pins.
