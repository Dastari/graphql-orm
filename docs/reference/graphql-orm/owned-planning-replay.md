---
title: Owned planning replay
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-19
review_by: 2027-02-01
supersedes: []
---

# Owned planning replay

ORM/macros 0.51.0 adds explicit capture of a complete successful owned planning
observation and pure reproduction of its exact result. A migration-review CLI
could archive that evidence for later inspection; independently, a CI schema
artifact or deployment approval tool could verify it after the database changes.
These are potential independent consumers, not claims of existing integrations.
The contract contains physical ORM facts, without product policy or receipts.

`build_migration_plan` and `SchemaManager::plan_migration[_with_options]` remain
appropriate for pure legacy planning from supplied schemas. They cannot retain
the actual full observation of `plan_owned_migration`, its private owned adapters
or its complete ownership-bound integrity preimage. Reconstructing a source from
a target or making a second observation does not supply that evidence. The new
path reuses the existing validation/scoping/diff/classification/render/hash engine.

ORM/macros 0.52.0 adds an explicitly selected native Rust 1.98.1 recipe. The
historical API still selects its original Rust 1.97.1 recipe. A compiler upgrade
does not select or upgrade evidence automatically.

## Enable and capture

```toml
# Existing workspace/package selector remains separately reviewed.
graphql-orm = { path = "../graphql-orm/crates/graphql-orm", default-features = false, features = ["sqlite", "owned-planning-replay"] }
```

The exact new feature graph is `owned-planning-replay = []`. Defaults remain
`default = ["sqlite"]`; no existing feature enables replay and replay enables no
backend or dependency. Select `postgres` instead of `sqlite` for PostgreSQL.
MSSQL capture returns `UnsupportedBackend` before pool acquisition. The existing
MSSQL and backend coexistence builds retain their ordinary contracts.

```rust,ignore
let limits = OwnedPlanningReplayLimits::default();
let (plan, witness) = database.schema().plan_owned_migration_with_replay_witness(
    version, description, &target, &ownership, options, limits,
    OwnedPlanningReplayCaptureConsent::accept_sensitive_schema_metadata_and_private_custody(),
).await?;
let private_bytes = witness.to_json(limits)?;
// Host stores private_bytes privately, with external cryptographic custody and
// authenticated producer source/artifact identity. Do not log these bytes.
let retained = OwnedPlanningReplayWitness::from_json(&private_bytes, limits)?;
let facts = retained.verify()?;
assert_eq!(facts.plan_hash(), plan.plan_hash());
```

The [compiled disposable example](../../../crates/graphql-orm/examples/owned_planning_replay.rs)
closes its in-memory database before decoding/verifying and prints hashes only:

```sh
cargo run -p graphql-orm --no-default-features \
  --features sqlite,owned-planning-replay --example owned_planning_replay
```

Capture returns the unchanged live `OwnedPlannedMigration` and a separate opaque,
owned `OwnedPlanningReplayWitness`. It retains the actual original complete
introspection result, including unowned tables, and preserves the original live
dependency and plan validation observations. It adds no observation and grants no
transactional snapshot guarantee. The original planning methods perform no replay
accounting, encoding, retention or extra clones even when the feature is enabled.

Complete schema metadata can contain sensitive default, predicate and search
policy literals. The explicit per-call consent token has no default constructor
and acknowledges private custody; it grants no authority. Capture does not read
application rows, credentials, environment, product state or a separate store.
Required metadata is never redacted while claiming exact reproduction. Debug and
Display for the new outer types omit metadata; the trusted caller can explicitly
inspect a retained original planning error or disclose JSON/fact getters.

## Public surface and ownership

The replay types and additive manager methods are reexported at
`graphql_orm::graphql::orm`. Witness and verified facts are opaque, own their data,
and are `Send + Sync`. They have no public serde implementation, mutable accessor,
unchecked constructor, apply-plan conversion, Deref or execution capability.

| Type | Methods |
| --- | --- |
| `OwnedPlanningReplayLimits` | `new(usize, RuntimeMigrationLimits) -> Result<Self, OwnedPlanningReplayError>`, `Default`, `max_encoded_bytes()`, `schema()` |
| `OwnedPlanningReplayCaptureConsent` | `accept_sensitive_schema_metadata_and_private_custody() -> Self` |
| `OwnedPlanningReplayRecipe` | `Historical8e1V1`, `Rust1981V1`, `identifier()`; non-exhaustive, no Default/string constructor |
| `OwnedPlanningReplayWitness` | `from_json(&[u8], limits)`, `from_json_for_recipe(&[u8], limits, expected_recipe)`, `to_json(&self, limits) -> Result<Vec<u8>, _>`, `verify(&self) -> Result<VerifiedOwnedPlanningReplay, _>` |
| `VerifiedOwnedPlanningReplay` | `format_version()`, `recipe()`, `backend()`, `version()`, `description()`, three hash getters, `ownership()`, `options()`, `steps()`, `statements()` |

The manager signature takes `version: impl Into<String>`,
`description: impl Into<String>`, `&OwnedSchemaModel`, `&ManagedTableSet`,
`PlanOptions`, limits and consent, and asynchronously returns
`Result<(OwnedPlannedMigration, OwnedPlanningReplayWitness), OwnedPlanningCaptureError>`.
Limits are private, checked and narrow-only, with a privately retained effective
profile. Encoding intersects a supplied profile with the saved profile;
verification cannot widen it.

Separate non-exhaustive errors preserve existing exhaustive enum callers.
`OwnedPlanningCaptureError` distinguishes `Planning(RuntimeMigrationError)` and
`Witness(OwnedPlanningReplayError)`; outer formatting never delegates to the
original potentially sensitive diagnostics. Replay codes are `UnsupportedFormat`,
`UnsupportedRecipe`, `UnsupportedBackend`, `LimitExceeded`, `InvalidWitness` and
`ReplayMismatch`, with fixed non-sensitive formatting.

## Passive verification and recipe

Verification applies desired-target validation only to the target. The observed
source receives structural/resource checks without target-only physical rules.
It reconstructs the original scope, diff, risks/reasons, SQL and three hashes,
then compares every ordered private step field exactly. In particular, empty
index directions remain distinct from an explicit ascending vector, even though
the engine's semantic `IndexModel::PartialEq` treats them as equivalent.

`VerifiedOwnedPlanningReplay` returns passive facts. Verification uses no database,
filesystem, policy callback or host state. It grants no authorization, current
baseline, crash recovery or data-validity assurance. Apply still requires the
original live plan and every existing policy/baseline/backend/application check.

Format 1 supports two separate compiler-scoped recipes:

| Exact identifier | Exact Rust/compiler commit |
| --- | --- |
| `graphql-orm-owned-planning-8e1-v1` | 1.97.1 / `8bab26f4f68e0e26f0bb7960be334d5b520ea452` |
| `graphql-orm-owned-planning-8e1-rust1981-v1` | 1.98.1 / `48a229ceaefd4985c50990b14116b6d856af0985` |

Both freeze the complete native planning behavior rooted in exact
`8e1e5698437d9ea5c0d6ff2c0f29cf7af58d45a0` (ORM 0.50.2), including schema
hashes, scope/diff/order/classification/reasons, rendering, native Debug, semantic
FNV bytes and the `owned-migration-v1` integrity wrapper. They require stock
`x86_64-unknown-linux-gnu` host **and** target, stock std and no compiler wrappers.
The new recipe is for new captures; it is not historical Rust 1.98.1 receipt
adoption or a compiler-independent hash redesign.

The library enforces each pair separately. Its dependency-free build script
checks exact release/commit/host and Cargo host/target. The private historical
qualification cfg is emitted only for 1.97.1; a distinct native cfg is emitted
only for 1.98.1. Nonempty `RUSTC_WRAPPER` or `RUSTC_WORKSPACE_WRAPPER` prevents
qualification. Unknown or unavailable pairs return `LimitExceeded` before ledger
creation, input inspection/conversion, codec allocation or database access.
No runtime environment/filesystem/process probe or override participates.
Build inputs and toolchain provenance remain trusted build-system responsibilities.
Ordinary planning has no recipe/profile gate and retains its native hashes.

The released `plan_owned_migration_with_replay_witness` and `from_json` remain
historical-only, with their original signatures/defaults and 1.97.1 admission.
They are neither read-only nor silently routed to the new recipe. On 1.98.1,
select the new recipe explicitly:

```rust,ignore
let recipe = OwnedPlanningReplayRecipe::Rust1981V1;
let (plan, witness) = database.schema().plan_owned_migration_with_replay_witness_for_recipe(
    version, description, &target, &ownership, options, recipe, limits,
    OwnedPlanningReplayCaptureConsent::accept_sensitive_schema_metadata_and_private_custody(),
).await?;
let private_bytes = witness.to_json(limits)?;
let retained = OwnedPlanningReplayWitness::from_json_for_recipe(&private_bytes, limits, recipe)?;
let facts = retained.verify()?;
assert_eq!(facts.recipe(), recipe.identifier());
assert_eq!(facts.plan_hash(), plan.plan_hash());
```

The additive capture signature inserts `recipe: OwnedPlanningReplayRecipe`
between options and limits. The decoder requires the encoded recipe to equal the
caller's expected recipe. Unknown/different encoded recipes return
`UnsupportedRecipe` on an admitted profile. There is no trial verification,
hash-based fallback or relabeling. Witnesses retain their recipe privately;
`to_json` and `verify` admit only that stored recipe, and passive facts report its
exact identifier. The [native example](../../../crates/graphql-orm/examples/owned_planning_replay_rust1981.rs)
runs with `cargo +1.98.1 run -p graphql-orm --no-default-features --features
sqlite,owned-planning-replay --example owned_planning_replay_rust1981`.

Rust 1.98.1 changed Unicode Debug escaping. A valid U+3164 default produces
different native plan hashes with unchanged source/target hashes in the retained
[paired ordinary-producer comparison](../../../crates/graphql-orm/tests/fixtures/owned-planning-replay-rust1981/README.md).
Never normalize/restrict that input or substitute the historical hash. Equal
hash strings on other inputs do not imply recipe equivalence: the native plan
hash is not a domain-separated portable digest. Hosts must cryptographically
bind the **complete canonical witness**, including format/recipe, and authenticated
producer artifact identity, rather than key custody/adoption on `planHash` alone.

Source review and independent complete ordinary-producer facts are required:
[unchanged twelve historical fixtures](../../../crates/graphql-orm/tests/fixtures/owned-planning-replay-8e1/README.md)
run only on their 1.97.1 recipe; [sixteen fresh native fixtures](../../../crates/graphql-orm/tests/fixtures/owned-planning-replay-rust1981/README.md)
run only on their 1.98.1 recipe. Shared helper names do not prove conformance.
No missing baseline is recovered, old approval reissued or old receipt upgraded.
Separate archival verifier integration, future portable hashes, historical artifact
adoption, release publication and consumer/pin changes require separate review.

## Complete v1 wire mapping

All members below are required, including explicit null options. Strings and
vectors preserve exact bytes/order; there is no normalization or unknown-member
extension bag. Ownership is the validated strictly sorted unique table list.
Object input order/whitespace may vary, including a step's `kind` appearing last;
structs require objects and enum values require strings at every nested level.
The strict decoder is re-entered at each internally buffered step payload field;
Serde's positional structs and object-form enums are not wire alternatives.
Duplicates, unknowns and missing members are rejected. Integer fields use
nonnegative checked machine-size values (except
signed i32 SRID), with overflow rejected. Hashes are 16 lowercase hex characters.

Canonical output is compact UTF-8 without a BOM/newline. Fields follow the order
below, `kind` precedes step fields, and integers are decimal. Quote/backslash and
short control escapes are used; remaining U+0000–U+001F use lowercase four-digit
Unicode escapes. Other Unicode and solidus remain unescaped. No producer identity,
checksum, receipt or approval-policy member is added to this engine format.

```text
Witness { formatVersion: 1, recipe: "graphql-orm-owned-planning-8e1-v1" | "graphql-orm-owned-planning-8e1-rust1981-v1",
  backend: "sqlite" | "postgres", version: String, description: String,
  ownership: [String], options: Options, source: Schema, target: Schema,
  expected: Expected }
Expected { sourceSchemaHash: Hash16, targetSchemaHash: Hash16, planHash: Hash16,
  steps: [Classified], statements: [String] }
Limits { maxCollections: usize, maxFields: usize, maxIndexes: usize, maxRelations: usize, maxKeyMembers: usize, maxDefaultBytes: usize, maxTargetBytes: usize, maxPlanSteps: usize, maxPlanStatements: usize, maxPlanBytes: usize }
Schema { limits: Limits, extensions: Vec<String>, tables: Vec<Table> }
Table { entityName: String, tableName: String, primaryKey: String, primaryKeys: Vec<String>, defaultSort: String, columns: Vec<Column>, indexes: Vec<Index>, compositeUniqueIndexes: Vec<Vec<String>>, foreignKeys: Vec<ForeignKey>, searchIndexes: Vec<Search>, appendOnly: bool, retentionPurge: bool, checkConstraints: Vec<Check> }
Column { name: String, sqlType: String, spatial: Option<Spatial>, nullable: bool, isPrimaryKey: bool, isUnique: bool, default: Option<String> }
Spatial { kind: SpatialKindWire, geometryType: GeometryWire, srid: i32 }
Index { name: String, columns: Vec<String>, columnDirections: Vec<DirectionWire>, isUnique: bool, method: MethodWire, isSpatial: bool, predicate: Option<Predicate> }
Predicate { column: String, values: Vec<String> }
ForeignKey { constraintName: Option<String>, columnPairs: Vec<Pair>, targetTable: String, isMultiple: bool, onDelete: DeleteWire }
Pair { sourceColumn: String, targetColumn: String }
Check { name: String, expression: String }
Search { name: String, tableName: String, entityName: String, primaryKey: String, strategy: StrategyWire, language: String, tokenizer: String, minTokenLen: usize, fallbackEnabled: bool, fields: Vec<SearchField>, jsonPaths: Vec<SearchPath>, relations: Vec<SearchRelation> }
SearchField { fieldName: String, columnName: String, weight: WeightWire, alias: Option<String>, policy: Option<String> }
SearchPath { fieldName: String, columnName: String, path: String, weight: WeightWire, policy: Option<String> }
SearchRelation { relationField: String, targetType: String, fields: Vec<String>, weight: WeightWire, maxItems: usize, policy: Option<String> }
Options { ignoreUnmanagedTables: bool }
Classified { step: Step, risk: RiskWire, reason: String }
SpatialKindWire = "geometry"
GeometryWire = "geometry" | "point" | "lineString" | "polygon" | "multiPoint" | "multiLineString" | "multiPolygon" | "geometryCollection"
DirectionWire = "asc" | "desc"
MethodWire = "default" | "gist"
DeleteWire = "restrict" | "cascade" | "setNull"
WeightWire = "A" | "B" | "C" | "D"
StrategyWire = "postgresTsvector" | "sqliteFts5" | "fallbackTable" | "mysqlFullText" | "mssqlFullText"
RiskWire = "additive" | "compatible" | "risky" | "destructive"
Step =
  { kind: "enableExtension", name: String }
  { kind: "createTable", table: Table }
  { kind: "dropTable", tableName: String }
  { kind: "addColumn", tableName: String, column: Column }
  { kind: "dropColumn", tableName: String, columnName: String }
  { kind: "alterColumn", tableName: String, before: Column, after: Column }
  { kind: "createIndex", tableName: String, index: Index }
  { kind: "dropIndex", tableName: String, indexName: String }
  { kind: "createSearchIndex", tableName: String, index: Search }
  { kind: "dropSearchIndex", tableName: String, indexName: String }
  { kind: "alterSearchIndex", tableName: String, before: Search, after: Search }
  { kind: "addForeignKey", tableName: String, foreignKey: ForeignKey }
  { kind: "dropForeignKey", tableName: String, foreignKey: ForeignKey }
  { kind: "setAppendOnly", tableName: String, enabled: bool, retentionPurge: bool }
  { kind: "setCheckConstraints", tableName: String, before: Vec<Check>, after: Vec<Check> }
```

Here `Vec<T>` is a JSON array, `Option<T>` is the complete T value or explicit
`null`, and `usize` is a JSON integer checked for the compiling target. The
private typed adapters in `owned_planning_replay/wire.rs` own this mapping.

## Intersecting ceilings

| Ceiling | Fixed maximum |
| --- | ---: |
| Encoded bytes | 67,108,864 |
| JSON depth (root = 1) | 64 |
| Values and object members | 2,000,000 |
| One string | 8,388,608 UTF-8 bytes |
| Aggregate decoded string/key bytes | effective encoded ceiling |
| Tables / columns per table / indexes per table / FKs per table | 512 / 256 / 128 / 128 |
| Ordered key members / default bytes | 32 / 65,536 |
| Definition bytes / steps / statements / SQL bytes | 8,388,608 / 16,384 / 32,768 / 8,388,608 |
| Version / description | 256 nonblank / 4,096 bytes |
| Cumulative work W / generated text T | 16,777,216 / 67,108,864 |

The v1 defaults are frozen private constants, independent of future changes to
ordinary ORM defaults. All ten embedded schema limits are retained exactly and must be positive and no
wider than the pinned defaults. Some otherwise accepted old/custom plans therefore
cannot be captured. There is no truncation, clipping, deduplication credit or
hash-only substitution. Conservative admission may refuse much smaller inputs;
these are intersecting ceilings, not guaranteed accepted sizes. Decoder admission
alone reserves `32 * encoded_bytes + 256` work, so W often dominates byte maxima.

The [accounting schedule](owned-planning-replay-accounting.md) defines units,
repeated passes, pre-expansion placement and implementation envelopes. It bounds
new pure work and cumulative logical UTF-8 generation, including temporary copies;
it does not bound total process memory, instructions or latency. Original live
introspection/dependency/plan checks and caller `Into<String>` implementations are
outside this bound, while their returned data is bounded before new engine work.

## Compatibility and release impact

| Area | Impact |
| --- | --- |
| Source/API | 0.52.0 adds a non-exhaustive recipe enum and two explicit-selection methods; all released signatures/fields/required traits/exhaustive enums unchanged |
| Behavior | Existing methods, defaults, limits, live observations and apply checks unchanged; capture is explicit per call |
| Feature graph | Empty default-off feature, with no incoming edges from existing features |
| Dependencies | No added or updated external dependency; reuse existing serde/serde_json; std-only build qualification |
| Serialized formats | Format 1 mapping/canonicalization unchanged; explicit new recipe string, with actual length charged; old witness bytes/provenance unchanged |
| Hashes | Existing compiler-native algorithms unchanged; native 1.98.1 Debug hashes may differ from 1.97.1; distinct recipes, no resealing/rehashing |
| Performance | Existing production paths have no accounting/retention cost; explicit capture/codec/replay pay bounded extra work; build script has fixed setup and queries rustc only for opt-in builds |
| Semver/migration | ORM/macros 0.52.0 aligned additive preparation; no database/wire migration; explicit new API required on 1.98.1, with historical evidence still verified on 1.97.1 |

Runtime conformance is limited to the documented normal positive gates. Malformed
input (including rejected nested shapes), unsupported-profile refusal, resource
refusal/overflow, sensitive disclosure, failed live checks, authority,
concurrency and cancellation properties are **code-inspection-only and runtime
unverified**. No adversarial/fuzz, negative-auth, fault/cancellation or security
concurrency scenarios, or aggregate suites containing them, are part of this
acceptance. Full release and production adoption remain separate decisions.
