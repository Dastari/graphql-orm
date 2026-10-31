---
title: Full-text search on runtime collections
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-31
review_by: 2027-04-30
supersedes: []
---

# Full-text search on runtime collections

A runtime collection can declare full-text search in the IR. Runtime migrations
create its storage, every runtime write keeps its documents in the same
transaction, and the runtime GraphQL list connection gains a ranked,
policy-safe `search` argument. Storage and strategies are the same as for
static search entities: SQLite FTS5, or PostgreSQL weighted `tsvector`. SQL
Server is unsupported, as for runtime GraphQL and migrations generally.

Static entity search, its generated API and its storage are unchanged.

## Declaration

```rust
RuntimeCollection {
    // ...
    search: Some(RuntimeSearch {
        fields: vec![
            RuntimeSearchField { field: title, weight: RuntimeSearchWeight::A, json_path: None },
            RuntimeSearchField { field: details, weight: RuntimeSearchWeight::C, json_path: Some("$.summary".into()) },
        ],
        language: None,          // PostgreSQL text search configuration; "english"
        tokenizer: None,         // SQLite: Unicode61 (default), Porter or Ascii
        remove_diacritics: true, // "Maori" matches "Māori"; unicode61 only
        min_token_len: None,     // 1..=16; 2
    }),
}
```

**Rules.** Validation reports each problem as a structured diagnostic:

| Rule | Diagnostic |
| --- | --- |
| The entry names an unknown field | `UnknownFieldReference` |
| The field is not `String` or `Json` | `InvalidSearchField` |
| A JSON field has no `json_path`, or a string field has one | `InvalidSearchField` |
| The path breaks the static path rules (`$`, then `.name` or `[*]`) | `InvalidSearchField` |
| A field and path pair is repeated | `InvalidSearchField` |
| No entries, or more than 32 (`RUNTIME_MAX_SEARCH_FIELDS`) | `InvalidSearchConfiguration` |
| `language` is not a lowercase identifier of at most 63 bytes | `InvalidSearchConfiguration` |
| `remove_diacritics` with a tokenizer other than `unicode61` | `InvalidSearchConfiguration` |
| `min_token_len` is outside 1 to 16 | `InvalidSearchConfiguration` |
| The collection has a composite primary key | `InvalidSearchConfiguration` |

Search documents are keyed by one primary-key value, as for static search entities; that is why a composite primary key is refused.

**Fingerprints.**
- `search` is structural: it determines storage, so it is part of both `fingerprint()` and `structural_fingerprint()`.
- It is rendered only when present, and omitted from serialized JSON when absent. So schemas without search keep their exact bytes, fingerprints, SDL and plans.
- Explicit defaults (`english`, `unicode61`, 2) are the same structure as omitted ones.

**`RuntimeSchema::from_static_entities`** maps a static search index:
- **Mapped:** its fields, JSON paths, language, tokenizer (`unicode61`, `unicode61 remove_diacritics 2`, `porter`, `ascii`) and minimum token length.
- **Refused**, each with a precise `UnsupportedCapability` diagnostic:
  - relation-sourced search fields;
  - per-field search policies;
  - the fallback-table strategy;
  - other tokenizers.

## Storage, readiness and backfill

The migration planner emits the static search storage model, so the existing renderers, introspection and drift checks apply.
- **Adding search** is `CreateSearchIndex`.
- **Changing** fields, weights, paths, language or tokenizer is `AlterSearchIndex`: a drop and a create, which then needs a rebuild.
- **Removing search** is `DropSearchIndex`, which drops its storage.
- **Unknown language.** On PostgreSQL, planning checks every runtime search `language` against `pg_ts_config`. An unknown configuration fails with `UnknownSearchLanguage` before any DDL is planned.

**Readiness.** A runtime search index records its readiness in its `__graphql_orm_search_metadata` row, as a `readiness` member of `config_json`. Keeping it in `config_json` needs no change to that shared table on existing databases.
- The index is created `building`.
- Until it is `ready`, the GraphQL `search` argument is refused with `search_index_not_ready`, so an interrupted or deferred backfill never returns silently incomplete results.
- Static search indexes record no readiness and behave as before.

**Backfill.**

```rust
// Apply, then index existing rows before returning (the default):
db.schema().apply_runtime_migration(&plan, &schema, ApplyOptions::default(), Default::default()).await?;

// Or leave the backfill to a host job:
db.schema()
    .apply_runtime_migration(&plan, &schema, ApplyOptions::default(), RuntimeSearchApplyOptions::deferred())
    .await?;
let progress = db
    .schema()
    .backfill_runtime_search(
        &schema,
        &collection_id,
        RuntimeSearchBackfillOptions::default().with_batch_size(500).with_max_batches(100),
    )
    .await?;
// progress.processed, progress.remaining, progress.ready
```

- **Batches.** Each batch is one transaction of `batch_size` rows, 500 by default, ordered by primary key.
  - On PostgreSQL the batch locks its rows with `FOR UPDATE` while it writes their documents, so a concurrent writer either waits for the batch or wrote its own document first. SQLite serialises writers.
  - Writes during a backfill maintain their own documents, so the result is consistent either way.
- **Resuming.** Progress is recorded after every batch. A backfill stopped by a crash, a cancellation or `max_batches` resumes where it stopped, and the index stays `building` until a call finishes it.
- **Fencing.** `max_batches` bounds one call, so a host can run a large backfill as its own fenced job.
- **Generic apply.** `apply_owned_migration` alone leaves new search storage `building`, which fails safe: search stays refused until a backfill runs.

## Writes

Every runtime create, update and delete maintains the row's document in the same transaction as the row. This includes cascade and set-null dependents, which go through the same write path.
- A failed document write fails the mutation, and the host's transaction runner rolls back the row write.
- No triggers are used, matching the static strategy.
- Rows written outside the ORM need a backfill or a rebuild.

## Queries

```graphql
query {
  records(search: { query: "harbour bridge", mode: PLAIN, minScore: 0.1 }, first: 20) {
    totalCount
    edges { score cursor node { id title } }
  }
}
```

- **The argument.** A collection with search gets an optional `search: RuntimeSearchInput` argument on its root list connection.
  - `RuntimeSearchInput { query: String!, mode: RuntimeSearchMode, minScore: Float }`, with `RuntimeSearchMode` `PLAIN`, `PHRASE`, `WEB` or `PREFIX`. Modes and `minScore` behave as for static entities.
  - These two types are registered only when a collection declares search.
  - Relation lists have no `search` argument.
- **Score.** Each `Edge` of a searchable collection has a nullable `score: Float`, which is null without `search`.
  - The input members and `score` follow the configured `field-case-*` feature, like the static `SearchInput`. For example `Query`, `MinScore` and `Score` under Pascal case.
- **Evaluation order.** The host's grant predicate, `where`, `minScore` and the match are one SQL condition. Policies and filters therefore apply before ranking and counting, and `totalCount` counts only matching rows the caller may see.
- **Ordering.**
  - Without `orderBy`, results are ordered by relevance (highest first), then by the collection's order and primary key.
  - With `orderBy`, the requested order applies, and relevance only filters through `minScore`.
- **Cursors.** A cursor binds a digest of the search (query, mode, minimum score and ranking). A cursor from another search, or from a read without one, is refused with `cursor_invalid`. Relevance-ordered cursors carry the score as their leading keyset value.
- **Bounds.** Queries are bounded by `RuntimeSearchLimits`: 256 bytes and 32 whitespace-separated tokens by default, set with `RuntimeGraphqlOptions::with_search_limits`. A query over the bounds is refused with `resource_limit`. A query with no searchable token is refused with `invalid_search`.
- **Rust API.** The same reads are available without GraphQL: build a `RuntimeSearchRequest` with `ValidatedRuntimeSchema::runtime_search_request`, add it with `RuntimeReadRequest::with_search`, and each `RuntimeEdge` then carries `score: Option<RuntimeScore>`.

## Authorization

Searching reads every searchable field.
- **Reported to the host.** A request with `search` reports each searchable field to the host's `RuntimeReadAuthority` as a filter operation named `"search"` in `RuntimeReadCheck::filter_operations`.
- **Denial.** A host that denies any of them refuses the request with its ordinary denial, before any read. An unreadable field therefore can neither match nor rank a row.
- **Allowlists.** Hosts whose authority allowlists operator names deny `"search"` until they opt in.

## Backend differences

| | SQLite | PostgreSQL |
| --- | --- | --- |
| Storage | FTS5 table `__graphql_orm_fts_<table>` | `__graphql_orm_search_<table>` with weighted `tsvector` and GIN |
| Score | `bm25` with weights 1.0/0.7/0.4/0.1, negated so higher is better | `ts_rank_cd` |
| `remove_diacritics` | the `unicode61 remove_diacritics 2` tokenizer | documents and queries folded by the ORM (canonical decomposition, combining marks removed); no `unaccent` extension needed |
| Language | ignored | text search configuration, checked at plan time |

- **Scores aren't comparable across backends.** Ordering is promised only within one backend.
- **SQLite's `bm25`** depends on corpus statistics, so relevance-ordered pages can shift if documents change between page requests.
- **`PHRASE` and `PREFIX` syntax** follows each engine.

## Not included

Vector or semantic search, trigram fuzzy matching, cross-collection search (#213), highlighting and snippets.
