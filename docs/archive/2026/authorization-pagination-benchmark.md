---
title: "Authorization pagination query evidence and benchmark"
kind: investigation
status: archived
owner: graphql-orm-maintainers
last_reviewed: 2026-09-20
review_by: none
supersedes: []
---

# Authorization pagination query evidence and benchmark

This records implementation evidence for 0.33.0. The
[active pagination guide](../../reference/graphql-orm/pagination-migration.md)
owns the API, compatibility, consistency, and operational contracts.

## Baseline confirmed

The implementation started at `7114dbf`, with runtime/macros version 0.29.0,
not the later revisions mentioned in the issue report. In this checkout,
both generated ordinary list branches in `operations.rs` tested
`db.row_policy().is_some()`, fetched all matching entities, authorized every
row, and applied visible offset/limit in Rust. The generated GraphQL keyset
resolver rejected any installed row policy. These match the reported mechanisms.

The new default callback-only list path intentionally preserves that behavior
and supplies the benchmark baseline. It does not simulate the old path by
turning authorization off. Complete SQL policies and callback scans are tested
against the same entity and data.

## Reproduce

Run from the workspace root:

```sh
cargo test -p graphql-orm --release --test authorized_pagination benchmark -- --ignored --nocapture
```

The ignored integration benchmark creates a fresh in-memory SQLite database,
279,287 rows, a 1,024-byte payload per row, and an index matching the declared
nullable keyset order. Full persisted entities are decoded. Every request asks
for 50 visible rows. The configured scan batch maximum is 256 and candidate
budget is 4,096; execution also caps each batch by the remaining requested
visible rows, so actual benchmark batches contain at most 50 rows.

Dense visibility permits every row. Sparse visibility permits only rows with
rank at least 279,000 (287 rows). The explicit unrestricted case grants all
rows regardless of that threshold. Offset requests retain exact totals; scans
omit counts. Runtime includes schema/request construction and query execution,
excluding seeding/index creation. This is a local release-mode example, not a
cross-platform performance guarantee or statistical service latency estimate.

`ReadQueryObserver` records actual SQL and fetched row counts before entity
decoding. The benchmark separately counts row-policy callback invocations.
COUNT result rows are excluded from entity-row figures. Integration tests
assert that row SELECT statements carry LIMIT or SQL Server FETCH NEXT and
that counts are absent when not requested; response size alone is not used as
proof of database pagination.

## Measured release run

| Visibility / path | Entities fetched and decoded | Largest batch | Policy callbacks | SQL statements | Returned nodes | Milliseconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Dense callback, legacy exact offset | 279,287 | 279,287 | 279,287 | 1 | 50 | 1,447.67 |
| Dense callback, bounded scan | 50 | 50 | 50 | 1 | 50 | 3.11 |
| Dense complete predicate, exact offset | 50 | 50 | 0 | 2 | 50 | 85.16 |
| Dense complete predicate, scan without count | 50 | 50 | 0 | 1 | 50 | 0.85 |
| Explicit unrestricted, exact offset | 50 | 50 | 0 | 2 | 50 | 52.56 |
| Explicit unrestricted, scan without count | 50 | 50 | 0 | 1 | 50 | 0.78 |
| Sparse callback, legacy exact offset | 279,287 | 279,287 | 279,287 | 1 | 50 | 1,278.62 |
| Sparse callback, bounded scan | 4,096 | 50 | 4,096 | 82 | **0** | 17.40 |
| Sparse complete predicate, exact offset | 50 | 50 | 0 | 2 | 50 | 57.56 |
| Sparse complete predicate, scan without count | 50 | 50 | 0 | 1 | 50 | 8.67 |

**The sparse callback scan is a budget-limited response, not a completed
50-row page.** Its status is `BUDGET_EXHAUSTED`, and its encrypted continuation
must be used to continue traversing denied rows. Getting to the first allowed
row still requires examining the preceding denied candidates. No speedup to a
completed sparse callback page is claimed. Partial prefilters can reduce this
work only when the permission structure supplies a useful SQL restriction.

Dense and complete/unrestricted scan responses have status `PAGE_FULL`, which
does not claim another visible row exists. Ordinary authorized keyset
connections use a bounded lookahead and an authorized opposite-direction probe
instead; their no-count behavior is covered by integration query capture.

## Process memory

Separate fresh processes measured Linux maximum resident set size with
`/usr/bin/time`. Seeded SQLite storage and index memory are included; these are
whole-process peaks, not allocator-level per-request measurements. Select a
single scenario using `PAGINATION_BENCH_CASE`. After the release build, the
benchmark executable path is printed by Cargo; run that executable directly
so compilation is excluded from the measurement, for example:

```sh
/usr/bin/time -f 'peak_rss_kib=%M process_seconds=%e' \
  env PAGINATION_BENCH_CASE=dense-callback-offset \
  target/release/deps/authorized_pagination-<build-hash> \
  benchmark --exact --ignored --nocapture
```

| Selected case | Peak RSS (KiB) | Query latency (ms), separate run |
| --- | ---: | ---: |
| `dense-callback-offset` | 1,267,404 | 1,432.31 |
| `dense-callback-scan` | 448,004 | 1.12 |
| `dense-complete-offset` | 448,012 | 98.85 |
| `sparse-callback-scan` | 448,044 | 18.77 |

The exact callback path's peak exceeds bounded paths by about 800 MiB in this
fixture. Much of the bounded paths' roughly 438 MiB is the in-memory database
itself. Latency varies with compilation/system contention and cache state;
fetched rows and callback counts are the deterministic comparison.

## Coverage and remaining costs

SQLite integration cases cover explicit unrestricted grants, complete predicates
with caller filtering/sorting, partial predicates, wholly denied batches,
budget boundaries, exhaustion, unique tie-breaking and null placement,
continuation without duplicates/skips, count opt-in, predicate errors,
entity/backend identity validation, equality relations, unauthenticated
requests, exact scopes, entity/field denial, cross-user cursor reuse, tampering,
and permission changes. Existing RowPolicy implementations, legacy offsets,
binary keys, and repository-only entities have regression coverage.

The shared parity fixture also runs on test-owned disposable PostgreSQL and
SQL Server containers. It exercises parameter binding, authorized counts,
forward keyset continuation, nullable order, equality-relation predicates,
callback scans through denied batches, and actual query limits. Reproduce with:

```sh
cargo test -p graphql-orm --no-default-features --features postgres \
  --test authorized_pagination_postgres -- --ignored
cargo test -p graphql-orm --no-default-features --features mssql \
  --test mssql_writes mssql_authorized_pagination -- --ignored
```

The integration harnesses create and identify their own loopback-only
containers, and delete only those containers. They do not consume ambient
application database URLs.

Bounded returned database rows do not imply constant database CPU or I/O.
Exact totals, sorting without suitable indexes, and low-selectivity predicates
can remain expensive. Callback-only exact visible offsets/totals retain their
legacy full scan; search and transaction-specific APIs retain their documented
prior contracts. Sparse callbacks on the explicit scan path have bounded work
per request but potentially substantial work over a complete traversal.

## Verification at handoff

- Ran the full `graphql-orm` and `graphql-orm-macros` package suites with
  `--no-fail-fast`. The two failures were expected additive-operation and
  compiler-diagnostic fixture changes; corrected those expectations and reran
  both targets successfully without snapshot-overwrite mode. The authorization
  pagination and binary-key regression targets also passed in that final run.
- Disposable PostgreSQL and SQL Server parity tests passed, including the
  parameterized relation predicate and nullable keyset scan cases.
- `cargo fmt --all -- --check`, warnings-denied Clippy for both packages and
  all targets, warnings-denied Rustdoc for both packages, the workspace
  dependency-direction check, and explicit backend compilation passed.
- Regenerated the workspace inventory and updated runtime/macros versions,
  package READMEs, changelog, migration notes, and the canonical pagination
  guide. This is the documentation impact of the change.
- The global documentation checker still reports four **pre-existing** expired
  `review_by: 2026-09-01` dates: the `ai-production-readiness` and
  `documentation-experience` active plans, and the investigation/plan templates.
  All seven changed Markdown files passed the same metadata/link checks when
  evaluated separately. Unrelated review dates were not silently extended.
