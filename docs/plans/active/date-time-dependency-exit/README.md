---
title: Date-time dependency exit plan
kind: plan
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-25
review_by: 2027-02-17
supersedes: []
---

# Date-time dependency exit

Plan for [#768](https://github.com/Dastari/graphql-orm/issues/768).
On 2026-09-14 RustSec marked `chrono` (RUSTSEC-2026-0543), `chrono-tz`
(RUSTSEC-2026-0555) and `humantime` (RUSTSEC-2026-0542) unmaintained. These
notices report no vulnerability. They are recorded as dated exceptions in
`dependency-audit-dispositions.json` (finding keys listed under
[Dispositions](#dispositions)), all with review date 2027-02-17. This plan
holds the inventory, the recommended replacement and the phased path.

**Verification basis.** The facts below were gathered at `origin/main`
3d31436cb (graphql-orm 0.201.0, Rust 1.97.1) with `git grep`, `cargo tree`
(offline, against `Cargo.lock`), the `Cargo.lock` files, and the crates.io
metadata API on 2026-11-22. Nothing was built or run beyond `cargo tree`.
Claims that came from reading source, or that were carried over from the
earlier revision of this plan and not re-checked, are marked *(read)*.

## Outcome

- The ORM's own date-time logic (parsing, canonical forms, calendar
  arithmetic and backend codecs) runs on a maintained crate.
- No third-party date-time type appears in an ORM signature unless the host
  enabled the ORM feature for that crate. That includes `#[doc(hidden)]`
  items that generated code uses, and public enum payloads and `From` impls.
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
- Upgrading sqlx to 0.9, tiberius to 0.13 or async-graphql to 8. Those are
  separate decisions. This plan only needs the `time` feature, which every
  one of them offers (see [Options](#options)).

## Dependencies

- Maintainer confirmation of the `time` engine and the dual-feature shape
  (the decision in [Recommendation](#recommendation)).
- Maintainer decision on the `TemporalKind` shape (open question 1 below).
- `next/breaking` for Phase 2. Phase 1 needs only `main`.
- A reviewed `hive-router` release that drops chrono, `vrl`/`chrono-tz` and
  `humantime`, for the last step of Phase 4. Nothing in this workspace can
  bring that forward.
- The tiberius fork decision (#516) is independent. The fork revision
  `4d67cd1` and tiberius 0.13.0 both offer the `time` feature.

## Inventory

### Who depends on what

Direct dependencies, per feature lane, from `cargo tree -e normal -i <crate>`
(sqlx 0.8.6 throughout):

| Lane | chrono reached through | chrono-tz | humantime |
| --- | --- | --- | --- |
| `graphql-orm --no-default-features --features sqlite` | `graphql-orm` direct only | absent | absent |
| `graphql-orm … --features postgres` | `graphql-orm` direct; `sqlx-core` and `sqlx-postgres` through `sqlx/chrono` | absent | absent |
| `graphql-orm … --features mssql` | `graphql-orm` direct; `tiberius` (fork `4d67cd1`) | absent | absent |
| `graphql-orm-ai` | `graphql-orm` (its default `sqlite` lane) | absent | absent |
| `graphql-orm-backup`, `-storage`, `-ai-tool-profiles`, `-operation-catalog`, `-macros`, `-router-protocol` | none (no chrono in the normal tree) | absent | absent |
| `graphql-orm-router` | `hive-router` and `hive-console-sdk`, `vrl`, `object_store`, `opentelemetry-stdout`, `syslog_loose`, `chrono-tz` | `vrl` ← `hive-console-sdk`, `hive-router` | `hive-router`, `hive-console-sdk`, `humantime-serde`, `object_store` |

Notes:

- With dev-dependencies (`-e normal,dev`), the sqlite lane also builds
  `sqlx-sqlite` and `async-graphql` with chrono. `crates/graphql-orm/Cargo.toml:83-85`
  turns on `sqlx/chrono`, `async-graphql/chrono` and a serde `chrono` for
  the test builds only. Tests must keep compiling after Phase 2.
- `chrono` is locked at 0.4.45, `chrono-tz` at 0.10.4, `humantime` at 2.4.0,
  `time` at 0.3.53 (MSRV 1.88) and `jiff` at 0.2.35. `jiff` is built only by
  `domain` ← `vrl`, so only in the router lane.
- `graphql-orm-router` pins `hive-router = "=0.2.19"` (`Cargo.toml:48`). The
  newest release on crates.io, 0.3.19 (2026-11-20), still depends on `chrono`,
  `humantime`, `humantime-serde`, `object_store`, `opentelemetry-stdout` and
  `vrl`. A `hive-router` bump therefore does not remove the router
  exceptions.
- `humantime` 2.4.0 has no dependencies in `Cargo.lock`
  (`Cargo.lock:3891-3894`). The disposition reason "humantime wraps chrono"
  is wrong. Phase 1 corrects the wording. The exception itself still applies.
- `graphql-orm-ai`, `graphql-orm-router` and `graphql-orm-storage` use `time`
  (`crates/graphql-orm-ai/Cargo.toml:78`,
  `crates/graphql-orm-router/Cargo.toml:56`,
  `crates/graphql-orm-storage/Cargo.toml:57`). Their sources contain no
  chrono paths (`git grep -w chrono -- '*.rs'` finds code only in
  `graphql-orm`, `graphql-orm-macros` and their tests; the vendored SMB
  tree and several crates match only the word "synchronous"). #768's
  mention of "AI timestamps" therefore needs no work.
- `graphql-orm` itself uses `time` only as a dev-dependency
  (`crates/graphql-orm/Cargo.toml:90`). It is not in the ORM's normal
  tree yet (`cargo tree -e normal -i time` finds nothing for the postgres
  lane). Phase 1 adds it.

Lockfile inventory: 11 tracked `Cargo.lock` files. All 11 record `chrono`.
Four record `chrono-tz` and `humantime`: the root lock, `crates/graphql-orm-router/fuzz`,
`crates/graphql-orm/tests/fixtures/federation-end-to-end` and
`crates/graphql-orm/tests/fixtures/graphql-features`. The other seven locks
(the three macro fixtures, `backend-coexistence`,
`migration-api-compatibility`, `repository-aggregate-consumer` and
`skills/verify`) have no router and can be made chrono-free.

### Direct use in workspace crates

**Manifests and features**

- `crates/graphql-orm/Cargo.toml:63` declares
  `chrono = { version = "0.4", default-features = false, features = ["std"] }`
  as a normal, non-optional dependency.
- `crates/graphql-orm/Cargo.toml:30`: the `postgres` feature enables
  `sqlx/chrono`. This is what lets a host field of type
  `chrono::DateTime<Utc>` decode and encode on PostgreSQL.
- `Cargo.toml:63`: the workspace `tiberius` entry (the first-assist fork at
  `4d67cd1`) enables tiberius's `chrono` feature for every MSSQL build.
- `crates/graphql-orm/Cargo.toml:62` and `Cargo.toml:34`: async-graphql is
  built with `dataloader`, `uuid` and `decimal` only. The ORM enables no
  date-time scalar. A host that serves `chrono::DateTime<Utc>` enables
  async-graphql's `chrono` feature itself.

**Public and hidden API surface** *(read; line numbers at 3d31436cb)*

- `crates/graphql-orm/src/lib.rs:325-326`: `#[doc(hidden)] pub use chrono;`.
  Generated code relies on this re-export.
- `crates/graphql-orm/src/graphql/orm/core.rs:84-90`: the public
  `#[non_exhaustive] enum TemporalKind` carries chrono payloads:
  `Date(chrono::NaiveDate)`, `DateTime(chrono::NaiveDateTime)` and
  `DateTimeOffset(chrono::DateTime<chrono::FixedOffset>)`. A host reads them
  through `TemporalValue::kind()`. **The earlier revision of this plan
  missed this item.**
- `core.rs:141-174`: four public `From` impls make
  `SqlValue: From<chrono::NaiveDate | NaiveDateTime | DateTime<Utc> |
  DateTime<FixedOffset>>`. **Also missed earlier.** The `TemporalValue`
  doc comment (`core.rs:67`) tells hosts to use them. `SqlValue::temporal_text`
  (`core.rs:118-136`) parses with chrono internally, which is not API.
- `crates/graphql-orm/src/db/mssql.rs:417` is `pub trait MssqlScalar`. Its
  impls for `chrono::NaiveDate`, `NaiveDateTime`, `DateTime<FixedOffset>`
  (`mssql.rs:1633-1636`) and `DateTime<Utc>` (`:1638-1659`) put chrono types
  in the bounds that generated code and hosts can reach. Also missed earlier.
- `crates/graphql-orm/src/graphql/orm/query.rs:338-345`: the hidden trait
  method `DatabaseFilter::matches_entity_at(&self, entity, _calendar_today:
  chrono::NaiveDate)`. The derive emits implementations at
  `crates/graphql-orm-macros/src/entity.rs:7033,7042,7223` and computes
  today's date with `::graphql_orm::chrono::DateTime::<Utc>`.
- `crates/graphql-orm/src/graphql/orm/spatial.rs:491-509`: the hidden public
  functions `date_filter_matches_at` and `date_filter_truth_at` take
  `today: chrono::NaiveDate`. They are used by generated SQLite fallback
  code and by `crates/graphql-orm/tests/date_filters.rs`.
- `RuntimeDateTime` (`crates/graphql-orm/src/graphql/orm/runtime_record.rs:275`)
  is a `String` newtype. Its public methods take and return strings. Its
  chrono constructor `from_utc` (`:366`) is `pub(crate)`.
- `crates/graphql-orm-operation-catalog/src/semantic/client.rs:157` is a doc
  comment that names `chrono::DateTime<Utc>`. It has no code dependency.

**Host entity field types (macros)** *(read)*

- Column types: a field whose type's last path segment is `DateTime` maps to
  `TIMESTAMPTZ` on PostgreSQL and `DATETIME2` on MSSQL, and to `TEXT` on
  SQLite. On MSSQL the derive also recognizes the chrono type names
  directly: `NaiveDate` as `DATE`, `NaiveDateTime` and `DateTime<Utc>` as
  `DATETIME2`, `DateTime<FixedOffset>` as `DATETIMEOFFSET`
  (`entity.rs:7827-7850`, used at `:9131-9134`). Every other unknown type,
  `OffsetDateTime` included, maps to `TEXT` or `NVARCHAR(MAX)`.
- Row decode: on PostgreSQL, a `DateTime` field is read with
  `row.try_get(column)`, so the host's type needs a sqlx codec. On SQLite and
  MSSQL it is read as text through `str_to_datetime`
  (`crates/graphql-orm/src/db.rs:2577`), which returns the stored string
  unchanged.
- Lifetime metadata (#670): a `DateTime` lifetime field must contain `Utc`
  (`entity.rs:10045-10052`). `docs/reference/graphql-orm/client-metadata.md:131`
  documents `DateTime<Utc>` as a supported lifetime field type.
- Scalar recognition for semantic descriptors already lists `Date`, `Time`,
  `DateTime` and `OffsetDateTime`
  (`crates/graphql-orm-macros/src/custom_operations.rs:1263-1266`,
  `crates/graphql-orm-macros/src/semantic_object.rs:194-197`).

**Internal logic (no public types)** *(read)*

| Area | Location | What chrono does |
| --- | --- | --- |
| Runtime date-time canonical form | `runtime_record.rs:13,289-370` | RFC 3339 parse, extended-year fallback parse, UTC normalization, rounding to the microsecond, `SecondsFormat::Micros` output |
| PostgreSQL `TIMESTAMPTZ` decode | `runtime_record.rs:1453-1490` | Range-checked binary decode that refuses `±infinity`, using chrono date arithmetic |
| Backup export | `graphql/orm/backup.rs:1783-1793`, `:2428-2430` | Parse and typed `DateTime<Utc>` read on PostgreSQL |
| Schema default equivalence | `default_equivalence.rs:136-161` | `timestamptz` literal parsing with a fixed offset |
| Runtime literal checks | `runtime_literal.rs:328,338` | Distinguish non-canonical from malformed RFC 3339 |
| Date filters | `graphql/filters.rs:836-845`, `spatial.rs:578-677`, `query.rs:4827,5851,5876,5983` | Parse dates, calendar "today" in UTC, day arithmetic |
| MSSQL codecs | `db/mssql.rs:1592-1602`, `:1840-1853`, `:2057-2090` | tiberius chrono types rendered as RFC 3339, `NaiveDate`/`NaiveTime` text, `datetime` tick rounding, range checks and offset arithmetic on encode |
| Dynamic `SqlValue::temporal_text` | `core.rs:118-136` | Classify text as date, offset date-time or naive date-time |
| Runtime execution | `graphql/runtime/execution.rs:2466` | `DateTime::UNIX_EPOCH` placeholder |

**Tests and fixtures**

- `crates/graphql-orm/tests/date_filters.rs`,
  `temporal_bind_compatibility.rs` (uses `graphql_orm::chrono`),
  `implicit_timestamp_defaults.rs` and
  `mssql_owned_queries/mssql_correctness.rs` use chrono directly.
- `tests/backup_date_time_and_self_cycles.rs` and
  `tests/postgres_runtime_repository/runtime_records_postgres.rs` cover
  chrono's range limits (comments name chrono). They remain the regression
  tests for the replacement.
- Nested lockfiles that record chrono are listed under "Lockfile inventory".
  They refresh with any manifest change.

### Formats the switch must preserve

- `RuntimeDateTime`: UTC, exactly six fractional digits, `Z`, 27 bytes for
  years 0001-9999 (`runtime_record.rs:345-370`;
  `docs/reference/graphql-orm/backup-runtime.md:50-66`). Backup values and
  row hashes are computed from this text, so any change would invalidate
  stored backup evidence.
- MSSQL text renderings: `datetime2`/`datetime`/`smalldatetime` as UTC
  RFC 3339 with `AutoSi` fractions, `datetimeoffset` with its offset, and
  `date` and `time` in chrono's `Display` form (`db/mssql.rs:1592-1602`).
  `time`'s `Display` output is not the same format. The replacement must
  format explicitly and pin the result with tests.
- `SqlValue::from(chrono value)` stores `Display` text (`core.rs:141-174`),
  which MSSQL and text-compatible binders reuse. That text is observable and
  must not change when the value is built from a `time` type.
- GraphQL output of a host-chosen type is the host's choice. async-graphql
  7.2.1 names the scalar `DateTime` for chrono `DateTime<Utc>`, time
  `OffsetDateTime` and jiff `Timestamp`, so SDL is unchanged *(read, carried
  over)*. The values differ, though. chrono serializes with `to_rfc3339()`
  (`+00:00`); time uses its strict `Rfc3339` format (`Z`); jiff uses
  `Display` (`Z`). Input parsing also differs: chrono's `FromStr` is lenient,
  time's `Rfc3339` parser is strict, and jiff accepts Temporal ISO 8601. A
  host that switches its field type therefore changes client-visible output.
  That is why switching stays a host decision and is never a side effect of
  an ORM upgrade.

## Options

Facts as published on crates.io on 2026-11-22:

| | Keep chrono (exception) | `time` 0.3 | `jiff` 1.0 |
| --- | --- | --- | --- |
| Maintenance | Unmaintained notice 2026-09-14; last release 0.4.45 on 2026-06-04 | Maintained; 0.3.55 on 2026-08-01, six releases between 2026-06-13 and 2026-08-01; already in the workspace graph | Maintained; 1.0.0 on 2026-11-15, 1.0.2 on 2026-11-21 |
| MSRV (workspace limit 1.97.1) | 1.62 | 1.88 | 1.85 (0.2.x: 1.70) |
| Time zones | with `chrono-tz`, which is also unmaintained | fixed offsets only; no tz database | Built-in time-zone support |
| RFC 3339, durations | Yes | Yes (`Rfc3339` well-known format; strict parser) | Yes (Temporal ISO 8601, `Span`) |
| sqlx 0.8.6 / 0.9.1 | `chrono` feature | `time` feature | No feature in either. Needs a third-party integration crate or ORM-owned codecs |
| tiberius (fork `4d67cd1`, and 0.13.0) | `chrono` feature | `time` feature | None |
| async-graphql 7.2.1 / 8.0.0-rc.5 | `chrono` (optional dep) | `time` (optional dep) | `jiff` feature |
| API stability | Stable, frozen | 0.3 line stable; minor releases raise MSRV (locked 0.3.53 is 1.88; 0.3.55 is 1.88) | 1.0, so the earlier "expected to change at 1.0" risk is gone |
| Host migration cost | None | One feature flag at the breaking release; field-type change optional | Hosts would also need codecs for PostgreSQL and MSSQL |
| Removes chrono from the build? | No | From ORM lanes without the `chrono` feature; not from the router, which gets it via `hive-router` | Same as `time` |

Rejected:

- **Keep chrono with no plan.** The exception is right for now, but it
  leaves the ORM's correctness-critical canonical forms on code that will
  not receive fixes. It also keeps chrono in ORM signatures, so any later
  exit is breaking.
- **jiff as the internal engine.** This revision changes the reason. The
  earlier plan rejected jiff because it was pre-1.0. It reached 1.0 on
  2026-11-15, so that no longer holds. The decisive reason is the drivers:
  neither sqlx (0.8.6 or 0.9.1) nor tiberius (the fork or 0.13.0) supports
  jiff, so the ORM would own the PostgreSQL and MSSQL codecs for every host
  type. `time` needs no such code. Hosts may still use jiff types through
  async-graphql's `jiff` feature on SQLite, where fields are text. Revisit
  jiff as a third host feature if sqlx or tiberius add support; the
  `CalendarDate` boundary in Phase 2 keeps that option open.
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
   `chrono` codecs, `From<chrono::…> for SqlValue`, `MssqlScalar` for chrono
   types) and `time` (the same for `time` types). Both can be on at once.
3. New column mappings are chosen per field by an explicit attribute, never
   by a Cargo feature. Features unify across a build, so a feature-driven
   mapping would silently change the declared type of an existing
   `OffsetDateTime` field from `TEXT` to `TIMESTAMPTZ`. Per the maintainer
   decision, a change that would silently alter results must be opt-in.

Open questions for the maintainer, to settle before Phase 2 starts:

1. **`TemporalKind` payloads.** Recommended: make the variants payload-free
   (`Date`, `DateTime`, `DateTimeOffset`, `Unparsed`). `TemporalValue`
   already retains the original text, and the MSSQL encoder can parse that
   text with `time` when it binds. The alternative, `time` payloads, would
   put `time` in the default public API. Check for host users of
   `TemporalValue::kind()` before choosing.
2. Whether `SqlValue: From<time::…>` ships in Phase 1 (additive, behind the
   `time` feature) or only in Phase 2. Recommended: Phase 1.

## What is breaking

These items go to `next/breaking` and the single breaking release, with
**Action needed** entries in the root `MIGRATION.md`:

- `postgres` stops enabling `sqlx/chrono` (`crates/graphql-orm/Cargo.toml:30`),
  and the workspace `tiberius` entry stops enabling `chrono` (`Cargo.toml:63`).
  Hosts with chrono fields add `features = ["chrono"]`. Without it, the
  build fails to compile; nothing changes silently.
- The four `From<chrono::…> for SqlValue` impls and the chrono impls of
  `MssqlScalar` move behind the `chrono` feature.
- `TemporalKind` changes shape (open question 1).
- The hidden re-export `graphql_orm::chrono` (`lib.rs:325-326`) is removed.
- `DatabaseFilter::matches_entity_at` and the two `date_filter_*_at`
  functions take `CalendarDate`. These items are hidden and implemented by
  generated code, and `graphql-orm` and `graphql-orm-macros` are released in
  lockstep. A host that implemented the trait by hand must still change,
  so these changes ship with the batch.
- `graphql-orm` stops depending on `chrono` unconditionally: `chrono` becomes
  an optional dependency enabled by the `chrono` feature.

These are not breaking and can go to `main` (**No action** or opt-in):

- Moving internal logic to `time` with byte-identical output.
- Adding the `chrono` feature (implied by `postgres` and `mssql` until the
  breaking release) and the `time` feature.
- The per-field opt-in for `OffsetDateTime` column mapping, PostgreSQL and
  MSSQL decode, and lifetime metadata.
- Correcting the `humantime` disposition wording.

## Dispositions

Current entries in `dependency-audit-dispositions.json` (recorded 2026-11-17,
`review_by` 2027-02-17):

- `RUSTSEC-2026-0543:chrono:0.4.45`
- `RUSTSEC-2026-0555:chrono-tz:0.10.4`
- `RUSTSEC-2026-0542:humantime:2.4.0`

The central table is `docs/reference/dependency-security-disposition.md:53-55`.

How `scripts/check-dependency-audit.py` constrains the exit:

- It audits every tracked `Cargo.lock` (11 today) and takes the union of
  findings. An exception can only retire when the crate is gone from all of
  them.
- It fails on a *stale* disposition: one whose finding no lock reports
  (`check_policy`). So the row must be removed in the same change that
  removes the last lock entry, never before and never after.
- Findings are keyed by exact version (`chrono:0.4.45`). A chrono bump
  re-keys the row, and the row's review date restarts under the policy.
- An optional dependency stays in a lock if cargo resolves it. Phase 2 must
  confirm, per lock, that making `chrono` optional drops it from the seven
  router-free locks.

Conclusion: ORM work alone narrows the exceptions to the router (and shrinks
the lock set that records them), but cannot retire them. They retire only when
`hive-router` stops depending on the crates. That is outside this workspace.
Keep the exceptions until then; at the 2027-02-17 review, re-check the
`hive-router` release notes and renew.

## Phased path

### Phase 1: internal-only replacement (main)

Packages: `graphql-orm` and `graphql-orm-macros` minor bumps; the documentation
and disposition wording fixes need none.

- Correct the `humantime` reason in `dependency-audit-dispositions.json` and
  `docs/reference/dependency-security-disposition.md`. No review date changes.
- Add `time` as a normal dependency of `graphql-orm`. Move the internal logic
  in the inventory table to it, one area per PR, each with a differential
  test.
- Add the `chrono` and `time` features. `postgres` and `mssql` imply `chrono`,
  so hosts see no change. Add the per-field `OffsetDateTime` opt-in, decode
  for it on PostgreSQL and MSSQL, and lifetime metadata.
- chrono stays an unconditional dependency, kept by the public items listed
  above.

Acceptance gates:

- Differential property tests (the workspace `proptest`) compare the old
  chrono and new `time` implementations. Inputs cover:
  - `RuntimeDateTime::parse`, `parse_exact` and `parse_stored`, including
    offsets, lowercase separators and leap seconds;
  - sub-microsecond rounding and the 0001/9999 boundaries;
  - signed extended years;
  - PostgreSQL binary `TIMESTAMPTZ` with `±infinity`;
  - MSSQL renderings, including `datetime` ticks;
  - `SqlValue::temporal_text` classification and `Display` text of the
    `From` impls;
  - date-filter semantics (`date_filter_truth_at`) at calendar boundaries.

  The tests pass with identical output, and the chrono side is removed only
  after they pass.
- Existing backup fixtures give identical `BackupValue` text and row hashes.
  `tests/backup_date_time_and_self_cycles.rs` and
  `tests/postgres_runtime_repository/runtime_records_postgres.rs` pass on the
  owned PostgreSQL lane, and the MSSQL date-time tests pass on the owned MSSQL
  lane.
- `scripts/compare-generated-schema.sh` shows byte-identical SDL for existing
  entities.
- trybuild cases show that a host `chrono::DateTime<Utc>` field compiles on
  each backend with `chrono`, and a `time::OffsetDateTime` field compiles with
  `time` and the per-field opt-in.
- The pinned 1.97.1 compiler builds every lane (`scripts/check-msrv.sh` and
  the CI MSRV jobs; `scripts/check-msrv.py` only checks the declared
  `rust-version`, not locked crates). `time` minor releases raise MSRV, so a
  lockfile refresh that would cross 1.97.1 is refused in review.
- Fresh `cargo tree -e normal -i chrono` per lane is recorded in the PR; it
  still lists chrono, which is expected here.
- The release-policy check, inventory check, `cargo fmt`, Clippy, Rustdoc
  and the sqlite, postgres and mssql compile lanes pass. `MIGRATION.md`
  entries are **No action**.

### Phase 2: public API changes (`next/breaking`)

Packages: `graphql-orm` and `graphql-orm-macros`, bumped to the next minor
above `next/breaking`, merged from `origin/main` before each push.

- Add `CalendarDate` and switch `matches_entity_at`, the `date_filter_*_at`
  functions and the derive's generated code to it.
- Remove `postgres` → `sqlx/chrono` and tiberius's `chrono` feature from the
  workspace entry; add them to the ORM's `chrono` feature. Make `chrono` an
  optional dependency.
- Move the `From<chrono::…> for SqlValue` impls and the chrono `MssqlScalar`
  impls behind `chrono`. Apply the decision on `TemporalKind`.
- Remove the hidden `graphql_orm::chrono` re-export.
- Narrow the three disposition reasons to "reached only through
  `hive-router`" in the same PR, and update the central table. Review dates
  stay.
- Add the top `MIGRATION.md` entry under `## Breaking release (unreleased)`,
  marked **Action needed**, explaining the `chrono` feature and warning that
  switching a field to `time` changes GraphQL output from `+00:00` to `Z`.

Acceptance gates:

- `cargo tree -e normal -i chrono` for the sqlite, postgres and mssql lanes
  without the `chrono` feature finds nothing (no `chrono`, no
  `sqlx-*`/`tiberius` chrono edge).
- With `chrono` enabled, the Phase 1 trybuild cases and differential tests
  still pass. Without it, the chrono trybuild case fails with a message that
  names the feature.
- The seven router-free lockfiles no longer contain `chrono`, confirmed by
  `grep -c '^name = "chrono"$'` on each. If any still does, record the
  resolver reason in this plan and keep the disposition scoped to that lock.
- Fixture and `trybuild` hosts that rely on chrono declare the feature
  explicitly; `crates/graphql-orm/tests/fixtures/*` build.
- All of Phase 1's gates still hold.

### Phase 3: removal of the old dependencies

Not scheduled; each item has its own trigger.

- **ORM `chrono` feature.** Remove it in a later breaking release, and only if
  no host depends on it. Trigger: a maintainer decision after at least one
  breaking release with the feature available. Gate: the Phase 2 `cargo tree`
  checks, run with `-e normal,dev`, also find no `chrono` in the ORM lanes
  (the dev-dependency lines at `crates/graphql-orm/Cargo.toml:83-85` replaced
  by `time` equivalents).
- **Router.** `chrono`, `chrono-tz` and `humantime` leave `Cargo.lock` only
  when a reviewed `hive-router` release drops them. Re-check at each
  `hive-router` bump and on the review date. This workspace never patches
  or forks to speed it up.
- **Jiff.** If sqlx or tiberius add jiff support, consider a third host
  feature behind the same `CalendarDate` boundary. This is optional.

### Phase 4: retire the dispositions

Executed per crate, atomically with the change that removes the crate from the
last tracked lock.

- **Remove the three rows and their table entries** in the same PR that
  removes the last lock entry, because `check_policy` rejects stale rows.
  All three leave with the router release that drops them, so expect one
  change per crate or one combined change, depending on that release.
- Until then, renew the rows at the 2027-02-17 review: keep the reason scoped
  to the router and move `review_by` out by the policy interval.

Acceptance gates:

- `python3 scripts/check-dependency-audit.py` passes with the rows removed, so
  no lock records the crates.
- `docs/reference/dependency-security-disposition.md` has no row for a retired
  crate.
- `cargo tree --workspace -e normal -i <crate>` for each retired crate reports
  that the package ID spec did not match.
- This plan moves to `docs/plans/completed/` with status `accepted` and the
  final evidence, once all three are retired.

## Current checkpoint

The Phase 1 internal engine migration is complete. The ORM uses `time` 0.3
for canonicalization, calendar arithmetic, and SQL Server codecs. String
decoding for SQL Server `date`, `datetime2`, and `datetimeoffset` uses chrono
when the stored or local year falls outside ±9999.
The workspace lockfile resolves `time` to 0.3.53. PostgreSQL keeps SQLx's chrono
codecs, and the compatibility layer preserves chrono payloads and accepted
legacy spellings.
The `humantime` exception now identifies its dependency path.

The maintainer approved the separate host features and explicit
`OffsetDateTime` mapping on 2026-11-24. That opt-in host API is the next
additive step. The `TemporalKind` and `CalendarDate` changes follow on
`next/breaking`. The dependency exceptions retain their 2027-02-17 review.
