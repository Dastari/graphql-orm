---
title: Private static query and generated-view backlog
kind: plan
status: draft
owner: graphql-orm-maintainers
last_reviewed: 2026-10-18
review_by: 2027-01-18
supersedes: []
---

# Private static query and generated-view backlog

Provide parameterized, typed repository reads and reviewed redacted-view
adapters without application query SQL, public CRUD roots, or dynamic runtime
schema adoption. The five requirements below remain unimplemented consumer
contracts. This backlog allocates no implementation, database access, release,
or deployment authority.

## Current checkpoint

This page continues the outstanding requirements from
[PR #91](https://github.com/Dastari/graphql-orm/pull/91). Its historical plan
retains the earlier review and execution evidence; its old version and release
chronology is not current guidance. Closing that PR as superseded by this page
does not mark these requirements complete.

Implemented foundations have their own canonical references:

- [Typed aggregates](../../../reference/graphql-orm/typed-aggregates.md) cover
  bounded text-group enumeration and identify the remaining computed-query
  and SQL Server limitations.
- [Entities and relations](../../../reference/graphql-orm/entities-and-relations.md)
  describe cross-crate generated relationships and current target authority.
- [Repository-only entities](../../../reference/graphql-orm/repository-only-entities.md)
  describe private records and the unsupported generated-view combination.
- [Strict authorization](../../../reference/graphql-orm/strict-authorization.md)
  owns existing visibility, projection and relationship policy requirements.

The fixes tracked by PRs #96, #98, #99 and #102 are already merged. Their
building blocks do not establish the five complete contracts below. The
separate [runtime platform capability work](../../completed/runtime-platform-capabilities/README.md)
is not a substitute for static query or generated-view support.

## Remaining contracts

1. **SQLite token/user joined and computed reads.** Support typed bounded
   joins and projections, database-clock status expressions, literal prefix
   and search matching, and mixed-direction computed keyset boundaries.
   Exercise sort ties, nulls, ownership isolation and pre-I/O cursor rejection
   in a standalone synthetic consumer.
2. **SQLite computed activity reads.** Reuse the typed query machinery for
   left joins, safe JSON properties, trim/nonblank fallback, coalesce/case
   expressions and explicit collation. Prove tenant isolation and complete
   bounded continuation; trusted SQL order fragments are not the interface.
3. **SQL Server computed labour groups and totals.** Define typed computed
   grouping, widened arithmetic and label grouping. Calculate whole-set totals
   before a display limit such as the first 25 groups. Disposable execution
   must cover overflow and qualifying groups outside the displayed subset.
4. **External read-only SQL Server computed pages.** Define typed conversion,
   date arithmetic, null/collation semantics and computed multi-column cursor
   boundaries. Preserve externally managed schema policy and prove that the
   read path performs no schema-management or data-write operation. Existing
   native-field keysets do not establish this computed contract.
5. **Private repository-backed redacted-view adapters.** Design a generated
   adapter that retains repository policy metadata and exposes only reviewed
   fields and relations. Preserve complete ownership keys, independent current
   target authorization, nullable denial, identity-isolated batches and stable
   bounded continuation. The existing `RepositoryEntity` plus
   `GraphQLRelations` rejection must not be bypassed or described as support.

## Dependencies and boundaries

The two SQLite requirements share an expression/query design. The SQL Server
summary and page requirements share a backend expression renderer. Private
adapters build on the existing cross-crate relation and authorization contracts.
These dependencies do not require dynamic runtime GraphQL adoption.

Reuse the owning query, backend and authorization machinery. Consumer reads
must not use raw fetches, `QueryBuilder`, SELECT/CTE strings or arbitrary SQL
escape hatches. Preserve private storage fields, immutable historical snapshots,
complete ownership bindings and current entity, row and selected-field policy.
Apply visibility before result limits and totals; cached targets are not authority.

Use synthetic SQLite or test-owned disposable SQL Server fixtures only. Never
access a consumer or operational database for qualification. Externally owned
sources remain read-only and externally schema-managed. The
[SQL Server reference](../../../reference/graphql-orm/mssql.md) remains the
authority for existing backend limitations.

## Acceptance before claiming support

Each contract needs an upstream-owned runnable consumer example, an exact
null/collation/authorization/pagination contract, focused execution regressions,
compatibility and migration notes, and explicit backend limitations. Measure
actual batch query counts and exercise bounded dispatch and continuation.
Compile-only, rendering-only and ignored evidence must remain distinct from
executed backend tests.

Run the affected bounded formatting, test, warnings-denied Clippy and Rustdoc
lanes, dependency/package compatibility checks, and documentation checks when
implementation is assigned. Preserve existing authorization and native-query
regressions. Consumer adoption requires a reviewed immutable published release;
a plan, open PR or merged unpublished branch is not an adoptable release.
