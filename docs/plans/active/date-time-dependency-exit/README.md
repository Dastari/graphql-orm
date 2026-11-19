---
title: Date-time dependency exit plan
kind: plan
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-19
review_by: 2027-02-17
supersedes: []
---

# Date-time dependency exit

Design plan for [#768](https://github.com/Dastari/graphql-orm/issues/768).
On 2026-09-14 RustSec marked `chrono` (RUSTSEC-2026-0543), `chrono-tz`
(RUSTSEC-2026-0555) and `humantime` (RUSTSEC-2026-0542) unmaintained. These
notices report no vulnerability. They are recorded as dated exceptions in
`dependency-audit-dispositions.json:52-66`, with a review date of 2027-02-17.
This plan holds the inventory and the recommended path. Nothing here is
implemented yet.

Every claim below comes from reading the source at `origin/main` 3e9c34a5f
(graphql-orm 0.183.0) and `Cargo.lock`. Nothing was built or run for this
plan. Where a fact still needs a build to confirm it, the plan says so.

## Outcome

- The ORM's own date-time logic (parsing, canonical forms, calendar
  arithmetic and backend codecs) runs on a maintained crate.
- No third-party date-time type appears in an ORM signature, including
  `#[doc(hidden)]` items that generated code uses.
- Hosts choose the date-time type of their entity fields with an explicit
  ORM feature. Hosts on chrono keep working, unchanged, until the breaking
  release. After it, they add one feature flag.
- Wire output that the ORM produces stays byte-identical across the switch:
  GraphQL values, `RuntimeDateTime` text, backup values and row hashes, and
  MSSQL text renderings.
- `chrono-tz` and `humantime` are only ever reached through the router's
  pinned `hive-router`. They remain dated exceptions until upstream drops
  them. The workspace does not fork or patch anything to remove them.

## Non-goals

- Forking or patching chrono, sqlx, tiberius, async-graphql or hive-router
  (standing policy: no new forks; supply-chain exceptions stay recorded).
- Contacting the upstream projects.
- Changing which instants or dates the ORM accepts, or how it rounds or
  orders them. Any difference found during the switch is a bug in the
  switch.
- Moving `graphql-orm-ai`, `graphql-orm-router` or `graphql-orm-storage`.
  They already use `time` and have no chrono code (see the inventory).
- Time-zone database support. No workspace crate uses `chrono-tz`.

## Inventory

### Who depends on what (from `Cargo.lock`)

| Crate | Locked version | Reached from |
| --- | --- | --- |
| `chrono` | 0.4.45 | `graphql-orm` (direct), `sqlx-core`/`sqlx-postgres`/`sqlx-sqlite`/`sqlx-mysql` 0.8.6, `tiberius` 0.12.4 (fork), `hive-router` 0.2.19, `object_store` 0.13.2, `opentelemetry-stdout` 0.32.0, `serde_with` 3.21.0, `syslog_loose` 0.22.0, `vrl` 0.33.1, `chrono-tz` |
| `chrono-tz` | 0.10.4 | `vrl` only. `vrl` is reached from `hive-router` and `hive-console-sdk` |
| `humantime` | 2.4.0 | `hive-router`, `hive-console-sdk`, `humantime-serde` and `object_store`, all reached only from `hive-router` |
| `time` | 0.3.53 | `graphql-orm` (dev only), `graphql-orm-ai`, `graphql-orm-router`, `graphql-orm-storage`, `agql-auth`, and others |
| `jiff` | 0.2.35 | `domain` (via `vrl`) only |

`Cargo.lock` is the union of every lane. Stage 0 records which of these
edges each feature lane actually builds (`cargo tree -i chrono -e features`
per lane).

The exception for `humantime` gives the reason "humantime wraps chrono". That
is inaccurate: `humantime` 2.4.0 has no dependencies in `Cargo.lock`. Stage 0
corrects the wording. The exception itself still applies.

### Direct use in workspace crates

Only `graphql-orm` and the code that `graphql-orm-macros` generates use
chrono. `graphql-orm-ai`, `graphql-orm-router` and `graphql-orm-storage` use
`time` (`crates/graphql-orm-ai/Cargo.toml:83`,
`crates/graphql-orm-router/Cargo.toml:56`,
`crates/graphql-orm-storage/Cargo.toml:57`), and their sources contain no
chrono paths. #768's mention of "AI timestamps" therefore needs no work.

**Manifests and features**

- `crates/graphql-orm/Cargo.toml:78` declares
  `chrono = { version = "0.4", default-features = false, features = ["std"] }`.
- `crates/graphql-orm/Cargo.toml:30`: the `postgres` feature enables
  `sqlx/chrono`. This is what lets a host field of type
  `chrono::DateTime<Utc>` decode and encode on PostgreSQL.
- `Cargo.toml:63`: the workspace `tiberius` (the first-assist fork at
  `4d67cd1`) enables tiberius's `chrono` feature for every MSSQL build.
- `crates/graphql-orm/Cargo.toml:77` and `Cargo.toml:34`: async-graphql is
  built with `dataloader`, `uuid` and `decimal` only. The ORM enables no
  date-time scalar. A host that serves `chrono::DateTime<Utc>` enables
  async-graphql's `chrono` feature itself.
- `crates/graphql-orm/Cargo.toml:102`: `time` is already a dev-dependency.

**Public and hidden API surface**

- `crates/graphql-orm/src/lib.rs:325-326`: `#[doc(hidden)] pub use chrono;`.
  Generated code relies on this re-export.
- `crates/graphql-orm/src/graphql/orm/query.rs:315-322`: the hidden trait
  method `DatabaseFilter::matches_entity_at(&self, entity, calendar_today:
  chrono::NaiveDate)`. The derive implements it at
  `crates/graphql-orm-macros/src/entity.rs:6806-6821` and `:6993-6997`, and
  computes today's date with `::graphql_orm::chrono::DateTime::<Utc>`.
- `crates/graphql-orm/src/graphql/orm/spatial.rs:491-509`: the hidden public
  functions `date_filter_matches_at` and `date_filter_truth_at` take
  `today: chrono::NaiveDate`. They are used by generated SQLite fallback
  code and by `crates/graphql-orm/tests/date_filters.rs:14,79-80`.
- `RuntimeDateTime` (`crates/graphql-orm/src/graphql/orm/runtime_record.rs:275`)
  is a `String` newtype. Its public methods take and return strings. Its
  chrono constructor `from_utc` (`:366`) is `pub(crate)`.
- `crates/graphql-orm-operation-catalog/src/semantic/client.rs:157` is a doc
  comment that names `chrono::DateTime<Utc>`. It has no code dependency.

**Host entity field types (macros)**

- Column types: a field whose type's last path segment is `DateTime` maps to
  `TIMESTAMPTZ` on PostgreSQL, `DATETIME2` on MSSQL and `TEXT` on SQLite
  (`crates/graphql-orm-macros/src/entity.rs:8931-8937`). Every other unknown
  type, `OffsetDateTime` included, maps to `TEXT` or `NVARCHAR(MAX)`
  (`:8939-8940`).
- Row decode: on PostgreSQL, a `DateTime` field is read with
  `row.try_get(column)` (`entity.rs:8311-8312`, `:8494-8495`), so the host's
  type needs a sqlx codec. On SQLite and MSSQL it is read as text through
  `str_to_datetime` (`crates/graphql-orm/src/db.rs:2548`), which returns
  the stored string unchanged.
- Lifetime metadata (#670): a `DateTime` lifetime field must contain `Utc`
  (`entity.rs:9761-9767`). Any other type name is refused (`:9783-9787`).
  `docs/reference/graphql-orm/client-metadata.md:131` documents
  `DateTime<Utc>` as a supported lifetime field type.
- Scalar recognition for semantic descriptors already lists `Date`, `Time`,
  `DateTime` and `OffsetDateTime`
  (`crates/graphql-orm-macros/src/custom_operations.rs:1263-1266`,
  `crates/graphql-orm-macros/src/semantic_object.rs:194-197`).

**Internal logic (no public types)**

| Area | Location | What chrono does |
| --- | --- | --- |
| Runtime date-time canonical form | `runtime_record.rs:13,289-370` | RFC 3339 parse, extended-year fallback parse, UTC normalization, rounding to the microsecond, `SecondsFormat::Micros` output |
| PostgreSQL `TIMESTAMPTZ` decode | `runtime_record.rs:1453-1490` | Range-checked binary decode that refuses `±infinity`, using chrono date arithmetic |
| Backup export | `backup.rs:1778-1788`, `:2422-2430` | Parse and typed `DateTime<Utc>` read on PostgreSQL |
| Schema default equivalence | `default_equivalence.rs:136-161` | `timestamptz` literal parsing with a fixed offset |
| Runtime literal checks | `runtime_literal.rs:328,338` | Distinguish non-canonical from malformed RFC 3339 |
| Date filters | `filters.rs:764-773`, `spatial.rs:578-677`, `query.rs:4547,5387,5412,5512` | Parse dates, calendar "today" in UTC, day arithmetic |
| MSSQL codecs | `db/mssql.rs:1424-1436`, `:1639-1652` | tiberius chrono types rendered as RFC 3339, `NaiveDate`/`NaiveTime` text, `datetime` tick rounding |
| Runtime execution | `graphql/runtime/execution.rs:2466` | `DateTime::UNIX_EPOCH` placeholder |

**Tests and fixtures**

- `crates/graphql-orm/tests/date_filters.rs:79-80` uses the hidden
  re-export. `tests/implicit_timestamp_defaults.rs:67,412,485` parse with
  chrono directly.
- `tests/backup_date_time_and_self_cycles.rs:301` and
  `tests/runtime_records_postgres.rs:380` cover chrono's range limits
  (comments name chrono). They remain the regression tests for the
  replacement.
- Nested lockfiles that record chrono:
  `crates/graphql-orm-macros/fixtures/{cross-crate-relations,full-stack,relationship-capability}/Cargo.lock`,
  `crates/graphql-orm/tests/fixtures/{backend-coexistence,federation-end-to-end,graphql-features,migration-api-compatibility,repository-aggregate-consumer}/Cargo.lock`,
  `crates/graphql-orm-router/fuzz/Cargo.lock` and `skills/verify/Cargo.lock`.
  They refresh with any manifest change.

### Formats the switch must preserve

- `RuntimeDateTime`: UTC, exactly six fractional digits, `Z`, 27 bytes for
  years 0001-9999 (`runtime_record.rs:345-370`;
  `docs/reference/graphql-orm/backup-runtime.md:50-66`). Backup values and
  row hashes are computed from this text, so any change would invalidate
  stored backup evidence.
- MSSQL text renderings: `datetime2`/`datetime`/`smalldatetime` as UTC
  RFC 3339 with `AutoSi` fractions, `datetimeoffset` with its offset, and
  `date` and `time` in chrono's `Display` form (`db/mssql.rs:1424-1436`).
  `time`'s `Display` output is not the same format. The replacement must
  format explicitly and pin the result with tests.
- GraphQL output of a host-chosen type is the host's choice. async-graphql
  7.2.1 names the scalar `DateTime` for chrono `DateTime<Utc>`, time
  `OffsetDateTime` and jiff `Timestamp`, so SDL is unchanged. The values
  differ, though. chrono serializes with `to_rfc3339()` (`+00:00`); time
  uses its strict `Rfc3339` format (`Z`); jiff uses `Display` (`Z`). Input
  parsing also differs: chrono's `FromStr` is lenient, time's `Rfc3339`
  parser is strict, and jiff accepts Temporal ISO 8601. A host that
  switches its field type therefore changes client-visible output. That is
  why switching stays a host decision and is never a side effect of an ORM
  upgrade.

## Options

| | Keep chrono (exception) | `time` 0.3 | `jiff` 0.2 |
| --- | --- | --- | --- |
| Maintenance | Unmaintained notice; no security fixes expected | Maintained; already in the workspace graph | Maintained; pre-1.0, with 1.0 planned |
| MSRV (workspace limit 1.97) | 1.62 | 1.88 (0.3.53) | 1.70 (0.2.35) |
| sqlx 0.8.6 | `chrono` feature | `time` feature | No feature. Would need a third-party integration crate or ORM-owned codecs |
| tiberius (fork `4d67cd1`, and 0.13.0) | `chrono` feature | `time` feature | None |
| async-graphql 7.2.1 | `chrono` | `time` | `jiff` |
| API stability | Stable, frozen | 0.3 line stable; minor releases raise MSRV | Expected to change at 1.0, forcing a second migration |
| Host migration cost | None | One feature flag at the breaking release; field-type change optional | Hosts would also need codecs for PostgreSQL and MSSQL |
| Removes chrono from the build? | No | From ORM lanes without the `chrono` feature; not from the router, which gets it via `hive-router` | Same as `time` |

Rejected:

- **Keep chrono with no plan.** The exception is right for now, but it
  leaves the ORM's correctness-critical canonical forms on code that will
  not receive fixes. It also keeps chrono in ORM signatures, so any later
  exit is breaking.
- **jiff as the internal engine.** Neither sqlx 0.8 nor tiberius supports
  it, so the ORM would have to own codecs for PostgreSQL and MSSQL. It is
  also pre-1.0. Hosts may still use jiff types through async-graphql's
  `jiff` feature on SQLite, where fields are text. Full jiff field support
  can be added as another host feature once the drivers support it.
- **ORM-owned codecs for every type, with no date-time crate.** The
  `TIMESTAMPTZ` binary decode is already hand-written
  (`runtime_record.rs:1453-1490`), but RFC 3339 parsing, extended years and
  calendar arithmetic are where bugs hide. A maintained crate is the safer
  owner.

## Recommendation

Use **`time` 0.3** as the ORM's internal engine. Keep chrono only as an
explicit host compatibility feature. Concretely:

1. The ORM's own logic moves to `time`. The hidden signatures take an
   ORM-owned `CalendarDate` newtype, so no third-party type crosses the
   runtime-macros boundary.
2. `graphql-orm` gains two host features: `chrono` (sqlx and tiberius
   `chrono` codecs, for `chrono::DateTime<Utc>` fields) and `time` (sqlx and
   tiberius `time` codecs, for `time::OffsetDateTime` fields). Both can be
   on at once.
3. New column mappings are chosen per field by an explicit attribute, never
   by a Cargo feature. Features unify across a build, so a feature-driven
   mapping would silently change the declared type of an existing
   `OffsetDateTime` field from `TEXT` to `TIMESTAMPTZ`. Per the maintainer
   decision, a change that would silently alter results must be opt-in.

## What is breaking

These items go to `next/breaking` and the single breaking release, with
**Action needed** entries in the root `MIGRATION.md`:

- `postgres` stops enabling `sqlx/chrono` (`Cargo.toml:30`), and the
  workspace `tiberius` entry stops enabling `chrono` (`Cargo.toml:63`).
  Hosts with chrono fields add `features = ["chrono"]`. Without it, the
  build fails to compile; nothing changes silently.
- The hidden re-export `graphql_orm::chrono` (`lib.rs:325-326`) is removed.
- `DatabaseFilter::matches_entity_at` and the two `date_filter_*_at`
  functions take `CalendarDate`. These items are hidden and implemented by
  generated code, and `graphql-orm` and `graphql-orm-macros` are released in
  lockstep. A host that implemented the trait by hand must still change,
  so these changes ship with the batch.

These are not breaking and can go to `main` (**No action** or opt-in):

- Moving internal logic to `time` with byte-identical output.
- Adding the `chrono` feature (implied by `postgres` until the breaking
  release) and the `time` feature.
- The per-field opt-in for `OffsetDateTime` column mapping, PostgreSQL and
  MSSQL decode, and lifetime metadata.

## Staged path

- **Stage 0, docs and evidence (main).** Keep all three exceptions. Correct
  the `humantime` reason. Record per lane which edges build chrono
  (`cargo tree -i chrono -e features` for the sqlite, postgres and mssql
  lanes and for the router), and add that evidence to this plan.
- **Stage 1, internal switch (main, graphql-orm/macros minor).**
  - Move the internal logic in the inventory table to `time`.
  - Add the `chrono` feature and make `postgres` imply it. Add the `time`
    feature and the per-field `OffsetDateTime` opt-in.
  - chrono stays a dependency through the `chrono` feature and the hidden
    signatures. Hosts see no change.
- **Stage 2, decouple (`next/breaking`).**
  - Remove the `postgres` → `chrono` implication and the re-export, and add
    `CalendarDate`.
  - Move tiberius's `chrono` feature from the workspace entry into the ORM's
    `chrono` feature.
  - Hosts on chrono add one feature. They keep working on chrono for as long
    as that feature exists.
- **Stage 3, not scheduled.** Remove the `chrono` feature in a later breaking
  release, and only if no host depends on it. The router's `chrono`,
  `chrono-tz` and `humantime` exceptions end only when a reviewed
  `hive-router` release drops them. Re-check them at each `hive-router` bump
  and on the review date.

The tiberius fork decision (#516) is independent of this plan. The fork
revision and tiberius 0.13.0 both offer the `time` feature, so stage 1 works
with either.

## Acceptance gates

- Differential property tests (the workspace `proptest`) compare the old
  chrono and new `time` implementations. Inputs cover:
  - `RuntimeDateTime::parse`, `parse_exact` and `parse_stored`, including
    offsets, lowercase separators and leap seconds;
  - sub-microsecond rounding and the 0001/9999 boundaries;
  - signed extended years;
  - PostgreSQL binary `TIMESTAMPTZ` with `±infinity`;
  - MSSQL renderings, including `datetime` ticks.

  The tests pass with identical output, and the chrono side is removed only
  after they pass.
- Existing backup fixtures give identical `BackupValue` text and row hashes.
  `tests/backup_date_time_and_self_cycles.rs` and
  `tests/runtime_records_postgres.rs` pass on the owned PostgreSQL lane, and
  the MSSQL date-time tests pass on the owned MSSQL lane.
- `scripts/compare-generated-schema.sh` shows byte-identical SDL for
  existing entities in every stage.
- trybuild cases show that a host `chrono::DateTime<Utc>` field compiles on
  each backend with `chrono`, and a `time::OffsetDateTime` field compiles
  with `time` and the per-field opt-in. After stage 2, the chrono case fails
  with a clear message when the feature is missing.
- After stage 2, `cargo tree -e normal -i chrono -p graphql-orm` for the
  sqlite, postgres and mssql lanes without the `chrono` feature finds
  nothing.
- No locked crate's MSRV exceeds 1.97. `time` minor releases raise MSRV, so
  a lockfile refresh that would cross 1.97 is refused in review.
- `MIGRATION.md` explains the `chrono` feature and warns that switching a
  field to `time` changes GraphQL output from `+00:00` to `Z`.

## Current checkpoint

Inventory and recommendation written; no code. Next boundary: the
maintainer confirms the `time` engine and the dual-feature shape. Then
stage 0's per-lane dependency evidence and the `humantime` reason
correction land in a small PR, ahead of the 2027-02-17 exception review.
