---
title: Private static query and generated relationship capabilities
kind: plan
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-09-30
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
[workspace-2026.09.30.4](https://github.com/Dastari/graphql-orm/releases/tag/workspace-2026.09.30.4),
resolved to `a90f229da66416e70f094c27e01f0bf4b4edc6bd`, ORM/macros 0.33.1.
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

## Overlap and dependency matrix

Source references point into the audited upstream code. A supported subprimitive
is not proof that the complete consumer query is supported. Reproductions and
execution tests must confirm each remaining diagnosis.

| Requirement | Published baseline | #88 relevance | A–D overlap | Independent static change / smallest evidence | Contract dependency |
| --- | --- | --- | --- | --- | --- |
| 1. Private token/user query, SQLite | Partial primitives: repository models, parameter binds, literal LIKE escaping, configurable 1,000-row bounds. No demonstrated typed joined/computed query, mixed-direction computed boundary, or status expression | Removes accidental GraphQL dependency from private aggregate enums only | C shares authorization, bounded pagination and private cursor concerns; dynamic registration is not required | Typed bounded join/projection/expression support in existing query engine; synthetic token/user execution fixture for exact prefix, DB clock status, literal search, sort ties, nulls and pre-I/O cursor rejection | New static query surface must preserve existing backend/query contracts; no dependency on A–D completion |
| 2. Computed activity query, SQLite | Same primitives; trusted `order_expression` SQL is not an acceptable consumer solution. No demonstrated typed safe-JSON/fallback/NOCASE continuation | None | C shares bounded reads and relation batching only | Reuse #1's typed query support; LEFT join, safe JSON property, trim/nonblank/coalesce/case and collation expressions; execution fixture proving tenant isolation and complete pages | Shared query expression design with #1; independent of A–D |
| 3a. Complete event identities, SQLite | Grouped aggregates are bounded by configured group limit. Increasing a finite limit does not prove complete continuation. Native field grouping does not express blank-only trim exclusion with distinct original identities and independent NOCASE ordering | Plain repository aggregate enums become usable without direct async-graphql; no continuation or distinct semantics added | None | First test existing grouped APIs. Add stable bounded distinct/group continuation only where missing; fixture exceeds configured maximum and retains case/padding identities | Shares typed predicate/collation support with #1/#2; no runtime dependency |
| 3b. Labour summary, SQL Server | Grouping/metrics use native generated fields; computed widened arithmetic, label grouping and whole-set total before top-25 are not established | Same enum compatibility only | None | Typed computed grouping/metrics and whole-set totals in existing aggregate engine; disposable SQL Server execution including overflow and groups outside displayed 25 | Shares expression renderer with #4; no A–D dependency |
| 4. External read-only computed pages, SQL Server | Existing typed source reads/keysets and schema-policy boundary are reusable; trusted SQL order fragments do not meet this contract | None | C's cursor/authorization principles overlap, but runtime APIs initially reject MSSQL | Typed computed multi-column boundary, conversion/date arithmetic/null/collation support; owned synthetic MSSQL source fixture; prove no schema-management calls | Shares query machinery with #1/#2 and expression renderer with #3b; independent of A–D |
| 5. Generated cross-crate relations | Source-level defect: generated target `__gom_placeholder` is `pub(crate)` and relations invoke it from the source entity's crate | None; aggregate fix must stay narrow | C's runtime relation executor is separate and does not establish generated static compilation | Separate focused macro fix after an external two-crate reproducer; public supported backend metadata/helper contract, composite ownership, independently authorized targets and measured batching | No runtime dependency; test with #7 authorization fixtures |
| 6. Private repository-backed redacted views | Explicit `RepositoryEntity` + `GraphQLRelations` rejection; runtime relation availability does not establish a generated adapter | Keeps repository aggregate helpers private; does not add an adapter | C dynamic composition is not a replacement | Generated view adapter that retains repository policy metadata and exposes only reviewed fields/relations; release/artifact fixture with organization ownership and stable bounded continuation | New adapter contract; depends on #5's cross-crate contract and #7's authorization verification, not A–D |
| 7. Nullable target denial and loaders | Integration report requires executable reproduction. Do not classify an ORM bug before selecting a nullable child in an actual GraphQL response | None | C independently requires current authority and identity-isolated batches | First add actual GraphQL response regressions distinguishing generated resolver and calling adapter behavior; change ORM only for demonstrated violations; policy/cache/alias/composite/batch/page/count fixtures | Preserves outer scope guards; shared verification for #5/#6, not dynamic-schema adoption |

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
