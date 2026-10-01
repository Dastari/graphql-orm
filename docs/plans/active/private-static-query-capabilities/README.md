---
title: Private static query and generated relationship capabilities
kind: plan
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-01
review_by: 2026-12-31
supersedes: []
---

# Private static query and generated relationship capabilities

## Outcome and current checkpoint

Provide supported, parameterized repository queries and generated redacted
relationships for the self-contained consumer contract, without application
query SQL, application databases, or dynamic runtime schema adoption. This is an
implementation and verification plan; missing interfaces below are not implemented
capabilities. No release publication or consumer deployment is authorized.

The latest published baseline verified through GitHub is
[workspace-2026.10.01.1](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.01.1),
resolved from annotated tag object `6f42a4fbc948cd066280818154660ca7dd2001e2`
to `82503290c0d0b96b78c687b4e75e5e279439eb59`, ORM/macros 0.33.3. The attached
JSON/Markdown release manifests match their published SHA256SUMS. This owner-
published baseline includes the aggregate compatibility fix; it adds no joined or
computed query, grouped continuation or generated-view adapter capability.
[PR #86](https://github.com/Dastari/graphql-orm/pull/86)'s approved contract head is
`c53a5de966ca5089a5e4aa2d727e144bae83e6bd`; its merge commit is
`9ed46db5d1c5c6214f83ef9084352030e14905ad`. This approval covers its bounded A–D
interfaces, not new static query interfaces. [PR #88](https://github.com/Dastari/graphql-orm/pull/88)'s
reviewed fix is `b29b00af6f78697dcbcb203556d79758cdf768d6`; its final head is
`7873984c2b981696a8535fd3d82dd63e1460816b`, merged at
`e6c5d744a25fb3bead59446aa31f417e5417ba17`. It includes the contract, fixture locks,
and independently merged AI/release maintenance. The reviewed fix,
dependency-minimal consumer, static SDL and plain aggregate behavior are retained.
The independent fixture maintenance [PR #92](https://github.com/Dastari/graphql-orm/pull/92)
had already consumed 0.33.2; [PR #94](https://github.com/Dastari/graphql-orm/pull/94)
merged at `82503290c0d0b96b78c687b4e75e5e279439eb59` and corrects the aggregate fix's aligned package identity to 0.33.3 without behavior changes.
Neither PR implements the query or generated-adapter gaps below.
[PR #90](https://github.com/Dastari/graphql-orm/pull/90), merged at
`ad054a66abdcdb1b7fb7e06479cc3e8a985985d9`, concerns native AI authentication
preflight. It adds no query, aggregate continuation or generated-view capability.
Its inherited schema-module fixture assertion was corrected independently in
[PR #95](https://github.com/Dastari/graphql-orm/pull/95);
[issue #93](https://github.com/Dastari/graphql-orm/issues/93) is closed.

[PR #96](https://github.com/Dastari/graphql-orm/pull/96) independently fixes the
cross-crate compilation defect through the existing public backend trait, at
`4617b332014758387a3c271a7b30a5aff4873453`, ORM/macros 0.33.4. Its executable
SQLite fixture binds complete tenant/endpoint ownership, retains historical
snapshots, measures two real target SELECTs for multiple parents and demonstrates
nullable target entity denial. PostgreSQL/MSSQL lanes are compile-only. This
unpublished reviewable PR does not establish all of #7's row/field/cache/preload checks,
implement #6's private generated adapters or close #1–4. Its package identity
must be realigned if independent 0.34.0 PR A merges before this patch.

[PR A / #97](https://github.com/Dastari/graphql-orm/pull/97) has corrected
review head `aeb07b1b631b3aa9901c4a06b70b31a7ace10ff3`, ORM/macros 0.34.0.
Focused verification passes 96 SQLite and 77 PostgreSQL tests (including seven
owned disposable execution cases), plus one MSSQL pre-I/O rejection test.
Actual ORM apply at the earlier reviewed head reproduced omitted FK action/
deferral loss. Runtime-only checks now reject unsupported FK semantics with scoped
`UnsupportedForeignKey` diagnostics, including relevant unowned/system incoming
sources, and recheck on the pinned apply transaction. Successful SQLite rebuilds
retain a restored connection and in-memory data; canceled/unrestored leases are
discarded. Static APIs and the approved scoped legacy DateTime rejection remain
unchanged; Integer epoch defaults and representable equivalence remain supported.
B remains gated on corrected, reviewed A. B–D are approved proposals, not implemented
interfaces. MSSQL runtime migrations remain unsupported; static query/view work
continues independently of these review gates.

[PR #98](https://github.com/Dastari/graphql-orm/pull/98) independently fixes the
confirmed preloaded-target authorization defect at
`f534bc9d479394fbd53c00b04aa3641fd04c129a`, ORM/macros 0.33.5. Generated
resolvers ignore cached/preloaded snapshots as authority, resolve complete
ownership keys against current database targets and check current entity, row
and selected-field policies. Selected-field preflight receives the actual
child context; aliases/fragments retain their authorization identity. Nullable
denial preserves the parent and reports a sanitized relationship-path error.
Generated parents stop issuing pool-only eager target preloads. Current SQL
visibility binds page/count queries and batch identity; residual row callbacks
use the existing host scan budget and fail closed on exhaustion.

Six execution regressions pass in each of the SQLite and test-owned disposable
PostgreSQL lanes. They cover a generated parent with an absent denied target
table, forged/stale preloads, a primed application loader cache, ownership
reassignment, changed policies, two/expired identities, nullable row/field denial,
provider-error sanitization, aliases/fragments and bounded page/count behavior.
The SQL-free `authorized_links` example executes. MSSQL remains compile-only;
one illustrative generated doctest is ignored in each execution lane. Existing
core relation/authorization regressions pass (54 tests; one large benchmark
ignored), along with six macro tests and warnings-denied/backend compatibility
checks. This is an implementation PR based on #96 for its cross-crate fixture,
not a published/adoptable release or a private-view adapter. All ten CI checks
are green at the exact #98 head; owner review/merge is still pending.

[PR #99](https://github.com/Dastari/graphql-orm/pull/99) implements complete bounded
SQLite text-group pages at `ae67b01d9222fae5787e02b553e9a24c21e31797`, aligned
ORM/macros 0.35.0 (macros alignment only). It extends the existing aggregate
builder with `fetch_group_page`, one TEXT-affinity group, optional existing
metrics, native grouping equality, ascending BINARY/NOCASE order and typed
host-context continuation. Source authorization precedes grouping; HAVING
continuation preserves complete metrics. SQL NULL sorts first, native TRIM
blank exclusion preserves padded nonblank strings, and every page is bounded
by 1–1,000 plus one lookahead under the existing stricter configuration.

The [standalone private consumer](https://github.com/Dastari/graphql-orm/blob/ae67b01d9222fae5787e02b553e9a24c21e31797/crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/examples/complete_events.rs)
executes 225 original event types without query SQL/direct async-graphql/public
roots. Six new SQLite regressions and existing library/aggregate tests pass
(38 tests); PostgreSQL and MSSQL unsupported profiles reject before pool I/O
(29/32 library/profile tests). One existing PostgreSQL aggregate regression
actually executes on a disposable owned container via `--ignored`, with no
skipped execution misrepresented. No live MSSQL execution is claimed. The
external consumer compiles all three backend lanes and executes on SQLite.
Warnings-denied Clippy/Rustdoc, documentation, inventory, dependency and package
checks pass. Explicit package-selected patch-level semver analysis passes 223
checks (30 inapplicable checks skipped), independent of version-bump heuristics.
The [canonical reference at its exact head](https://github.com/Dastari/graphql-orm/blob/ae67b01d9222fae5787e02b553e9a24c21e31797/docs/reference/graphql-orm/typed-aggregates.md)
specifies cursor bounds, current SQL policy checks and storage/collation limits.
This is reviewable implementation, not a published/adoptable capability.
Computed SQL Server grouping/totals, joined/computed reads and private generated
views remain missing. Its private visibility accessor matches #98's shared
accessor; combine the independent reviewed branches without a new policy layer.

Preferred package identity order is #96 (0.33.4), #98 (0.33.5), #97 (0.34.0),
then #99 (0.35.0).
If owner review chooses another order, realign the unpublished patches and rerun
combined checks. Functional query/group/private-view work has no dependency on
B–D completion; only shared generated-relation fixes require committed
predecessors. No new release is published or authorized here.

## Overlap and dependency matrix

Source references point into the audited upstream code. A supported subprimitive
is not proof that the complete consumer query is supported. Reproductions and
execution tests must confirm each remaining diagnosis.

| Requirement | Published baseline | #88 relevance | A–D overlap | Independent static change / smallest evidence | Contract dependency |
| --- | --- | --- | --- | --- | --- |
| 1. Private token/user query, SQLite | Partial primitives: repository models, parameter binds, literal LIKE escaping, configurable 1,000-row bounds. No demonstrated typed joined/computed query, mixed-direction computed boundary, or status expression | Removes accidental GraphQL dependency from private aggregate enums only | C shares authorization, bounded pagination and private cursor concerns; dynamic registration is not required | Typed bounded join/projection/expression support in existing query engine; synthetic token/user execution fixture for exact prefix, DB clock status, literal search, sort ties, nulls and pre-I/O cursor rejection | New static query surface must preserve existing backend/query contracts; no dependency on A–D completion |
| 2. Computed activity query, SQLite | Same primitives; trusted `order_expression` SQL is not an acceptable consumer solution. No demonstrated typed safe-JSON/fallback/NOCASE continuation | None | C shares bounded reads and relation batching only | Reuse #1's typed query support; LEFT join, safe JSON property, trim/nonblank/coalesce/case and collation expressions; execution fixture proving tenant isolation and complete pages | Shared query expression design with #1; independent of A–D |
| 3a. Complete event identities, SQLite | Missing in published 0.33.3: existing group limit truncates the complete set | Plain repository aggregate enums become usable without direct async-graphql; no continuation or distinct semantics added | None | PR #99 implements existing-builder text-group pages, original native identities, independent NOCASE/BINARY ordering, complete SQL visibility, HAVING continuation and a private SQL-free executable example; SQLite tests exceed the secure cap and exercise 1,000 groups plus lookahead | Independently reviewable; no runtime or joined-query prerequisite |
| 3b. Labour summary, SQL Server | Grouping/metrics use native generated fields; computed widened arithmetic, label grouping and whole-set total before top-25 are not established | Same enum compatibility only | None | Typed computed grouping/metrics and whole-set totals in existing aggregate engine; disposable SQL Server execution including overflow and groups outside displayed 25 | Shares expression renderer with #4; no A–D dependency |
| 4. External read-only computed pages, SQL Server | Existing typed source reads/keysets and schema-policy boundary are reusable; trusted SQL order fragments do not meet this contract | None | C's cursor/authorization principles overlap, but runtime APIs initially reject MSSQL | Typed computed multi-column boundary, conversion/date arithmetic/null/collation support; owned synthetic MSSQL source fixture; prove no schema-management calls | Shares query machinery with #1/#2 and expression renderer with #3b; independent of A–D |
| 5. Generated cross-crate relations | Source-level defect: generated target `__gom_placeholder` is `pub(crate)` and relations invoke it from the source entity's crate | None; aggregate fix must stay narrow | C's runtime relation executor is separate and does not establish generated static compilation | PR #96 reproduces E0624 in two real crates and fixes generated calls through existing `OrmBackend::placeholder`; SQLite execution proves composite ownership, current entity denial and measured batches. Broader row/field/cache authority checks remain #7 | No runtime dependency; test with #7 authorization fixtures |
| 6. Private repository-backed redacted views | Explicit `RepositoryEntity` + `GraphQLRelations` rejection; runtime relation availability does not establish a generated adapter | Keeps repository aggregate helpers private; does not add an adapter | C dynamic composition is not a replacement | Generated view adapter that retains repository policy metadata and exposes only reviewed fields/relations; release/artifact fixture with organization ownership and stable bounded continuation | New adapter contract; depends on #5's cross-crate contract and PR #98's authorization verification, not A–D |
| 7. Nullable target denial and loaders | Actual nullable target entity denial is supported: #96's fixture preserves parent fields, returns null and reports sanitized child-path errors; missing targets return null without error. The published baseline has a confirmed preloaded-target bypass; independent PR #98 fixes it and executes current row/field/cache/identity regressions on SQLite/PostgreSQL | None | C independently requires current authority and identity-isolated batches | PR #98 supplies actual GraphQL response regressions and authoritative-target fixes, including the generated-parent eager-preload failure; retain its execution fixtures when generating private adapters | Preserves outer scope guards; shared verification for #5/#6, not dynamic-schema adoption |

## Source evidence and reusable contracts

- [Repository entity documentation](../../../reference/graphql-orm/repository-only-entities.md)
  describes database-aware private records and absence of generated public roots.
- [`query.rs`](../../../../crates/graphql-orm/src/graphql/orm/query.rs) owns
  `PaginationConfig::legacy` (1,000 rows), `contains_like_pattern`,
  `GroupedAggregateQuery::group_limit`, and `GroupedAggregateSqlQuery`.
- [`entity.rs`](../../../../crates/graphql-orm-macros/src/entity.rs) emits the
  crate-private placeholder helper and trusted SQL `order_expression` metadata.
- [`relations.rs`](../../../../crates/graphql-orm-macros/src/relations.rs) calls
  the target placeholder helper and owns generated static relationship resolvers.
- [`lib.rs`](../../../../crates/graphql-orm-macros/src/lib.rs) explicitly rejects
  combining `RepositoryEntity` with `GraphQLRelations`.
- [Strict authorization](../../../reference/graphql-orm/strict-authorization.md),
  [SQL Server](../../../reference/graphql-orm/mssql.md), and
  [the separate A–D contract](../runtime-platform-capabilities/README.md)
  remain authoritative for their existing boundaries.

## Reviewable changes and order

Keep #88 independently reviewable and A → B → C → D in its accepted workstream.
Additional static work is split into: cross-crate generated helper compatibility;
nullable target/loader authorization reproductions and demonstrated fixes;
typed joined/computed reads with SQLite examples; complete grouped/distinct
continuation and SQL Server summary; SQL Server computed cursor conformance;
and private repository-backed generated view adapters. Share the existing query,
backend and authorization machinery rather than introducing a local ORM or
arbitrary SQL escape hatch. Commit reviewed predecessors for dependent branches;
run combined regression lanes after independent changes are ready.

## Acceptance gates

Every numbered requirement needs a standalone upstream-owned runnable consumer
example, source references, exact null/collation/authorization/pagination contract,
focused regression tests, compatibility notes, and backend limitations. Examples
may configure schema/setup through the ORM; consumer reads must not use raw fetches,
QueryBuilder, SELECT/CTE strings or arbitrary SQL fragments. Never access consumer or
operational databases. Externally owned read-only execution must emit no schema
management. Preserve private storage fields, immutable historical snapshots,
complete ownership bindings, current independent target authority, bounded reads
and dispatch, exact mixed-direction boundaries, and parameterized values.

Measure actual batch query counts; compile declarations alone are insufficient.
Execute meaningful SQLite and disposable MSSQL lanes; label rendering/compile-only
and ignored/skipped evidence accurately. Run bounded builds, formatting,
warnings-denied Clippy/Rustdoc, dependency checks and documentation checks. Track
unrelated target-gating failures separately in [issue #89](https://github.com/Dastari/graphql-orm/issues/89).

## Release handoff for the consumer release coordinator

Return the final capability/PR dependency matrix, exact heads and merged commits,
versions/features, runnable examples, commands/results, limitations and migration
notes. A reviewed branch or merged unpublished commit is **not** an adoptable consumer
release. No new release is authorized here; provide release-ready changes for owner
review/publication, then identify the immutable published tag, resolved commit and
release metadata when available. Keep all consumer changes as handoff instructions.
