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
capabilities. The owner has authorized merge/release of the independent ORM fixes;
consumer deployment and consumer repository/database access remain outside scope.

The latest published baseline is
[workspace-2026.10.01.2](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.10.01.2),
annotated tag object `aebd78455e58ca19c7a2126d760b2e6e4f8b64c0` resolved to
`37e45fb7f2837806a16ceaf3781ac8c8b5b1c148`, ORM/macros 0.33.3 and AI 0.106.1.
Its AI-only source identity is preserved. Coordination with its release owner
reserved `workspace-2026.10.01.3` for the ORM fixes through
[the release coordination thread](https://github.com/Dastari/graphql-orm/pull/101#issuecomment-5926104394)
and [the completed `.2` run](https://github.com/Dastari/graphql-orm/actions/runs/36824574352).
The published `.2` includes the aggregate compatibility fix, but does not implement
the independent query/relationship/projection changes below. An open or merged
unpublished branch is not an adoptable consumer release.
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
`33205fe5277c2a8898ae87a6feb76abb071d6783`, ORM/macros 0.33.4.
The implementation reviewed at `4617b332014758387a3c271a7b30a5aff4873453`
remains unchanged; only the current main/AI release and generated inventory were reconciled. Its executable
SQLite fixture binds complete tenant/endpoint ownership, retains historical
snapshots, measures two real target SELECTs for multiple parents and demonstrates
nullable target entity denial. PostgreSQL/MSSQL lanes are compile-only. Merged at `c2cd8b71de20f925d102d6a686bc4b9eff45e23c` after all ten checks
passed, this fix does not establish all of #7's row/field/cache/preload checks,
implement #6's private generated adapters or close #1–4. Its package identity
is preserved by the subsequent 0.33.5/0.35.0/0.35.1 independent release set.

[PR A / #97](https://github.com/Dastari/graphql-orm/pull/97) has corrected
review head `e41866d582c36fd920044f6461cb15c6bd446f17`, ORM/macros 0.36.0.
It incorporates the committed static 0.35.1 predecessor while leaving migration,
owned-FK scanning and guarded-apply modules byte-identical to corrected
`aeb07b1b631b3aa9901c4a06b70b31a7ace10ff3`. The unpublished identity advances
without changing the approved contract or legacy datetime/FK corrections.
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
`1126941f4f3e735af2857b2ff6d0e693bf1c3845`, ORM/macros 0.33.5. The
reviewed `f534bc9d479394fbd53c00b04aa3641fd04c129a` implementation is retained
while reconciling the committed #96 predecessor and current main. Generated
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
not a private-view adapter. All ten CI checks passed at the reconciled #98 head;
it merged at `6e7a3eafe2630ccd3fa23a2788a4d430b1e389e7`. Publication of the
combined release remains required before consumer adoption.

[PR #99](https://github.com/Dastari/graphql-orm/pull/99) implements complete bounded
SQLite text-group pages at `0e48a76197b39ef62ac43fbf7cd083d866cc79cc`, aligned
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
All eleven checks passed at the reconciled head; it merged at
`6bfd52212f4b77d712c9e9916bdeeaf1c0336096`. This remains an unpublished
capability until the coordinated release completes.
Computed SQL Server grouping/totals, joined/computed reads and private generated
views remain missing. Its private visibility accessor matches #98's shared
accessor; its reconciled head incorporates committed #96/#98/current main without a
new policy layer, retaining the implementation verified at
`ae67b01d9222fae5787e02b553e9a24c21e31797`.

A separate synthetic compatibility checkout combines corrected #97 with the
committed #96/#98/#99 implementation sources, leaving all review heads unchanged.
It passes 107 SQLite and 78 PostgreSQL core/migration/group/profile tests, one
additional actually executed PostgreSQL aggregate regression, and six generated
relationship regressions on each SQLite/PostgreSQL lane. Each two-crate relation
workspace also has two illustrative generated doctests ignored; these are not
backend execution evidence. The combined MSSQL lane passes 31 library tests and
two pre-I/O rejection tests, with no live server execution. Warnings-denied
combined core/macros Clippy passes independently on all three backends. This
checks shared implementation compatibility; it is not a merged/reviewed release
or a substitute for owner review. The original #99 head passed all eleven checks; reconciled release heads receive
fresh CI rather than claiming the prior head's status. Explicit
package-selected patch-level API analysis separately passes 223 checks for
corrected #97 against published 0.33.3 and #98 against #96 (30 inapplicable
checks skipped in each). [Issue #100](https://github.com/Dastari/graphql-orm/issues/100)
tracks the local runner's observed no-analysis pass without explicit package
selection; the functional evidence uses commands that actually analyze the
Git-only libraries. Keep this verification correction separate from feature
PRs and preserve publication/license policy.

The independent merge/release order is #96 (0.33.4), #98 (0.33.5), #99 (0.35.0),
then #102 (0.35.1). Functional query/group/private-view work has no dependency on
B–D completion. PR A remains gated on corrected migration review; its unpublished
identity is now advanced to 0.36.0, preserving the prospective independent
package set rather than downgrading it. Keep the approved datetime/FK/ownership
acceptance requirements intact during that reconciliation.

[PR #102](https://github.com/Dastari/graphql-orm/pull/102) fixes the additional
confirmed projection blocker using the existing `ReadVisibility` contract.
Its exact review head is `5245040c761032b582134e84d077d7c32f3b74a2`,
aligned ORM/macros 0.35.1. A global provider may deliberately return `Unrestricted` or an entity/backend-bound
`Complete` typed predicate; projections retain entity/selected-field checks and
apply current SQL visibility before limits and key lookups, including pinned
transactions. `CallbackOnly`/`Prefilter` still reject before SELECTs, without full
entity fallback or private-key retrieval. Existing trait implementations remain
source compatible through a provided identity hook; a missing/wrong identity for
complete visibility fails closed. No static SDL, storage or cursor changes.
All twelve checks passed at the exact head; merged at
`73ef09dbdb58b756ad0466900b905c08c282a2b4`. The coordinated source-only `.3`
release selects that immutable source; publication remains required for adoption.

Four new SQLite and four isolated PostgreSQL regressions execute actual views
that fail if the private-key expression is selected. They test all pool/transaction
fetch methods, key helpers, current visibility changes, denied entities/selected
fields, residual/wrong-entity policies, handwritten projection compatibility and
actual bounded query counts. A non-superuser PostgreSQL reader also proves SQL
visibility intersects RLS, pinned/pool auth is retained and settings do not leak.
Complete SQLite group pages execute under the installed
provider for unrestricted/complete visibility and reject residual checks.
The [SQL-free private repository example](../../../../crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/examples/policy_projections.rs)
requires no direct async-graphql dependency or public CRUD roots. Generated MSSQL
projections remain unsupported. Older ambient-URL PostgreSQL projection fixtures
are not counted as this owned execution evidence.

The combined 0.35.1 implementation executes 45 SQLite core/group/projection tests,
33 PostgreSQL core/profile/projection tests, and six generated relationship tests
on each SQLite/PostgreSQL lane. Each source-crate lane ignores one illustrative
generated doctest; execution tests are not skipped. MSSQL executes 32 library/
pre-I/O profile tests and compiles generated relations, with no live server evidence.
Both dependency-minimal examples execute. Warnings-denied Clippy/Rustdoc across
explicit backends, documentation/inventory/dependency/version gates and explicit
package-selected patch semver pass (223 checks; 30 inapplicable checks skipped).
This evidence does not establish the remaining joined/computed/MSSQL/view contracts.

## Overlap and dependency matrix

Source references point into the audited upstream code. A supported subprimitive
is not proof that the complete consumer query is supported. Reproductions and
execution tests must confirm each remaining diagnosis.

| Requirement | Published baseline | #88 relevance | A–D overlap | Independent static change / smallest evidence | Contract dependency |
| --- | --- | --- | --- | --- | --- |
| 1. Private token/user query, SQLite | Partial primitives: repository models, parameter binds, literal LIKE escaping, configurable 1,000-row bounds. No demonstrated typed joined/computed query, mixed-direction computed boundary, or status expression | Removes accidental GraphQL dependency from private aggregate enums only | C shares authorization, bounded pagination and private cursor concerns; dynamic registration is not required | Typed bounded join/projection/expression support in existing query engine; synthetic token/user execution fixture for exact prefix, DB clock status, literal search, sort ties, nulls and pre-I/O cursor rejection | New static query surface must preserve existing backend/query contracts; no dependency on A–D completion |
| 2. Computed activity query, SQLite | Same primitives; trusted `order_expression` SQL is not an acceptable consumer solution. No demonstrated typed safe-JSON/fallback/NOCASE continuation | None | C shares bounded reads and relation batching only | Reuse #1's typed query support; LEFT join, safe JSON property, trim/nonblank/coalesce/case and collation expressions; execution fixture proving tenant isolation and complete pages | Shared query expression design with #1; independent of A–D |
| 3a. Complete event identities, SQLite | Missing in published `.2` (ORM 0.33.3): existing group limit truncates the complete set | Plain repository aggregate enums become usable without direct async-graphql; no continuation or distinct semantics added | None | PR #99 implements existing-builder text-group pages, original native identities, independent NOCASE/BINARY ordering, complete SQL visibility, HAVING continuation and a private SQL-free executable example; SQLite tests exceed the secure cap and exercise 1,000 groups plus lookahead | Independently reviewable; no runtime or joined-query prerequisite |
| 3b. Labour summary, SQL Server | Grouping/metrics use native generated fields; computed widened arithmetic, label grouping and whole-set total before top-25 are not established | Same enum compatibility only | None | Typed computed grouping/metrics and whole-set totals in existing aggregate engine; disposable SQL Server execution including overflow and groups outside displayed 25 | Shares expression renderer with #4; no A–D dependency |
| 4. External read-only computed pages, SQL Server | Existing typed source reads/keysets and schema-policy boundary are reusable; trusted SQL order fragments do not meet this contract | None | C's cursor/authorization principles overlap, but runtime APIs initially reject MSSQL | Typed computed multi-column boundary, conversion/date arithmetic/null/collation support; owned synthetic MSSQL source fixture; prove no schema-management calls | Shares query machinery with #1/#2 and expression renderer with #3b; independent of A–D |
| 5. Generated cross-crate relations | Source-level defect: generated target `__gom_placeholder` is `pub(crate)` and relations invoke it from the source entity's crate | None; aggregate fix must stay narrow | C's runtime relation executor is separate and does not establish generated static compilation | PR #96 reproduces E0624 in two real crates and fixes generated calls through existing `OrmBackend::placeholder`; SQLite execution proves composite ownership, current entity denial and measured batches. Broader row/field/cache authority checks remain #7 | No runtime dependency; test with #7 authorization fixtures |
| 6. Private repository-backed redacted views | Explicit `RepositoryEntity` + `GraphQLRelations` rejection; runtime relation availability does not establish a generated adapter | Keeps repository aggregate helpers private; does not add an adapter | C dynamic composition is not a replacement | Generated view adapter that retains repository policy metadata and exposes only reviewed fields/relations; release/artifact fixture with organization ownership and stable bounded continuation | New adapter contract; depends on #5's cross-crate contract and PR #98's authorization verification, not A–D |
| 7. Nullable target denial and loaders | Actual nullable target entity denial is supported: #96's fixture preserves parent fields, returns null and reports sanitized child-path errors; missing targets return null without error. The published baseline has a confirmed preloaded-target bypass; independent PR #98 fixes it and executes current row/field/cache/identity regressions on SQLite/PostgreSQL | None | C independently requires current authority and identity-isolated batches | PR #98 supplies actual GraphQL response regressions and authoritative-target fixes, including the generated-parent eager-preload failure; retain its execution fixtures when generating private adapters | Preserves outer scope guards; shared verification for #5/#6, not dynamic-schema adoption |

| Additional global-policy projections | Published `.2` rejects projections whenever any RowPolicy provider is installed, even when it declares the model unrestricted | None | C shares fail-closed projection authority; static projections require no dynamic schema | #102 reuses current `ReadVisibility` for generated SQLite/PostgreSQL projections; unrestricted/complete SQL modes work, residual evaluation fails closed; compiled private example and isolated execution tests | Depends only on the existing typed projection/visibility machinery; independent of A–D; combined with #99 group pages in `.3` |

## Source evidence and reusable contracts

- [Repository entity documentation](../../../reference/graphql-orm/repository-only-entities.md)
  describes database-aware private records and absence of generated public roots.
- [`query.rs`](../../../../crates/graphql-orm/src/graphql/orm/query.rs) owns
  `PaginationConfig::legacy` (1,000 rows), `contains_like_pattern`,
  `GroupedAggregateQuery::group_limit`, and `GroupedAggregateSqlQuery`.
- [`entity.rs`](../../../../crates/graphql-orm-macros/src/entity.rs) emits the
  crate-private placeholder helper and trusted SQL `order_expression` metadata.
- [`relations.rs`](../../../../crates/graphql-orm-macros/src/relations.rs) calls
  the public backend placeholder contract after #96 and owns generated static relationship resolvers.
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
release. The owner has authorized the coordinated `.3` release of these independent fixes.
Use the protected official release workflow after reviewed heads and combined checks;
identify the immutable published tag, resolved commit, checksummed manifests and
remaining gaps. A pending release-environment approval is not publication. Keep all consumer changes as handoff instructions.
