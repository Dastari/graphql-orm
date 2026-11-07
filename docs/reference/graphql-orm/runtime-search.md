---
title: Full-text search on runtime collections
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-04
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
        remove_diacritics: true, // "Maori" matches "Māori" on every backend; unicode61 or porter
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
| `remove_diacritics` with the `ascii` tokenizer | `InvalidSearchConfiguration` |
| `min_token_len` is outside 1 to 16 | `InvalidSearchConfiguration` |
| The collection has a composite primary key | `InvalidSearchConfiguration` |

Search documents are keyed by one primary-key value, as for static search entities; that is why a composite primary key is refused.

**Fingerprints.**
- `search` is structural: it determines storage, so it is part of both `fingerprint()` and `structural_fingerprint()`.
- It is rendered only when present, and omitted from serialized JSON when absent. So schemas without search keep their exact bytes, fingerprints, SDL and plans.
- Explicit defaults (`english`, `unicode61`, 2) are the same structure as omitted ones.

**`RuntimeSchema::from_static_entities`** maps a static search index:
- **Mapped:** its fields, JSON paths, language, tokenizer (`unicode61`, `unicode61 remove_diacritics 2`, `porter`, `porter unicode61 remove_diacritics 2`, `ascii`) and minimum token length.
- **Refused**, each with a precise `UnsupportedCapability` diagnostic:
  - relation-sourced search fields;
  - per-field search policies;
  - the fallback-table strategy;
  - other tokenizers.

## Storage, readiness and backfill

The migration planner emits the static search storage model, so the existing renderers, introspection and drift checks apply.
- **Adding search** is `CreateSearchIndex`.
- **Changing** fields, weights, paths, language, tokenizer, `remove_diacritics` or `min_token_len` is `AlterSearchIndex`: a drop and a create. The recreated index starts `building` and needs a rebuild, which `apply_runtime_migration` runs unless it is deferred.
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
  - On PostgreSQL the batch locks its rows with `FOR UPDATE` while it writes their documents. A concurrent writer of one of those rows waits for the batch instead of racing it. Runtime writes are serializable, so the writer then usually fails as retryable (`TransactionError::Retryable`) with nothing written, and the host's ordinary retry succeeds. SQLite serialises writers.
  - Writes during a backfill maintain their own documents, so no document is left stale either way.
- **Failures.** A batch that fails partway through rolls back as a whole: its documents are discarded, the resume point does not advance, and the index stays `building`.
- **Resuming.** Progress is recorded after every batch. A backfill stopped by a crash, a cancellation or `max_batches` resumes where it stopped, and the index stays `building` until a call finishes it.
- **Bounding.** `max_batches` bounds one call, so a host can run a large backfill as its own fenced job.
- **Stale schemas are refused.** Every batch first compares the index configuration stored with the index against the supplied schema's declaration for the collection, in the batch's own transaction. The comparison covers the fields, weights, JSON paths and their columns, the language, the tokenizer (which carries `remove_diacritics`) and the minimum token length. If they differ, the call returns `RuntimeMigrationError::Diagnostics` with one `SearchConfigurationMismatch` diagnostic per differing part, and writes nothing: no document, readiness or resume point. Each diagnostic names the collection. Its `subject` is `fields`, `language`, `tokenizer`, `min_token_len` or `index` (table, type, key or storage strategy).
  - This catches a host job that still holds a schema from before the index was dropped and recreated for another declaration. It also applies to a `ready` index, which then stays `ready`.
  - Backfill with the schema the index was last migrated to. A backfill with the current schema completes and resumes exactly as before.
  - On PostgreSQL each batch also locks the index's metadata row (`FOR UPDATE`) before the comparison. A migration that recreates the index while a batch runs therefore waits for the batch, or PostgreSQL ends one of the two with a deadlock error and it writes nothing. The next batch's comparison sees the new configuration. A batch never writes back a configuration that a migration replaced. SQLite serialises writers.
  - The apply-time backfill of `apply_runtime_migration` uses the same fence. Passing it a schema other than the one the plan was built from therefore fails with this diagnostic after the plan is applied, and the index stays `building`.
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
  - The semantic operation catalog declares the same optional root argument and description as SDL, so a runtime-schema host can use `finish_with_semantic_catalog` with searchable collections. Regenerate stored catalogs after adopting 0.95.0.
  - Runtime collection reads and writes support SQLite and PostgreSQL. MSSQL runtime row decoding is unsupported and runtime GraphQL composition refuses it with `unsupported_backend`; the native MSSQL entity lane does not enable runtime collections.
- **Score.** Each `Edge` of a searchable collection has a nullable `score: Float`, which is null without `search`. A score is computed from the row's own document only; see [ranking isolation](#ranking-isolation).
  - The input members and `score` follow the configured `field-case-*` feature, like the static `SearchInput`. For example `Query`, `MinScore` and `Score` under Pascal case.
- **Evaluation order.** The host's grant predicate, `where`, `minScore` and the match are one SQL condition. Policies and filters therefore apply before ranking and counting, and `totalCount` counts only matching rows the caller may see.
- **Ordering.**
  - Without `orderBy`, results are ordered by relevance (highest first), then by the collection's order and primary key.
  - With `orderBy`, the requested order applies, and relevance only filters through `minScore`.
- **Cursors.** A cursor binds a digest of the search (query, mode, minimum score and ranking). A cursor from another search, or from a read without one, is refused with `cursor_invalid`. Relevance-ordered cursors carry the score as their leading keyset value, so their digest also names the scoring function. A relevance-ordered cursor issued before 0.91.0 is refused with `cursor_invalid`; the client restarts from the first page. Cursors of searches with an explicit `orderBy` keep their digest.
- **Bounds.** Queries are bounded by `RuntimeSearchLimits`: 256 bytes and 32 whitespace-separated tokens by default, set with `RuntimeGraphqlOptions::with_search_limits`. A query over the bounds is refused with `resource_limit`. A query with no searchable token is refused with `invalid_search`.
- **Rust API.** The same reads are available without GraphQL: build a `RuntimeSearchRequest` with `ValidatedRuntimeSchema::runtime_search_request`, add it with `RuntimeReadRequest::with_search`, and each `RuntimeEdge` then carries `score: Option<RuntimeScore>`.

## Authorization

Searching reads every searchable field.
- **Reported to the host.** A request with `search` reports each searchable field to the host's `RuntimeReadAuthority` as a filter operation named `"search"` in `RuntimeReadCheck::filter_operations`.
- **Denial.** A host that denies any of them refuses the request with its ordinary denial, before any read. An unreadable field therefore can neither match nor rank a row.
- **Allowlists.** Hosts whose authority allowlists operator names deny `"search"` until they opt in.
- **The search itself.** `RuntimeReadCheck::search` is `Some(RuntimeReadSearchCheck)` when the layer carries `search`, and `None` otherwise. It has the requested `mode` (`Plain` when none is given), whether `minScore` is supplied (`min_score`), and whether results are ordered by relevance (`ranked`, true without `orderBy`). Hosts do not need to infer search from `filter_operations`. Only the root list's `Select` check carries it.
- **Withholding modes.** A host that offers only some modes denies the others in `authorize`. Denial is the ordinary `authorization_denied`, before any read. For example, a host can withhold `WEB`, which SQLite evaluates like `PLAIN`:

  ```rust
  if check.search.is_some_and(|search| search.mode == SearchMode::Web) {
      return Err(RuntimeGraphqlError::denied());
  }
  ```

### Ranking isolation

The host's grant predicate narrows which rows match. On both backends a row's score is also computed from that row's document alone, with no statistics over other rows. So for rows the grant hides, adding or removing them leaves unchanged:

- a visible row's score;
- the order of the visible rows;
- which rows `minScore` keeps, and `totalCount`.

A caller can therefore not count hidden rows that contain a term from the scores of visible ones.

- **PostgreSQL** uses `ts_rank_cd` over the row's weighted `tsvector`, without normalization. This is unchanged.
- **SQLite** scores the weighted number of matched phrase instances in the row's document:
  `score = 1.0·nA + 0.7·nB + 0.4·nC + 0.1·nD`.
  Here `nX` is the number of matched phrase instances in the row's weight-X text. FTS5's built-in `highlight()` auxiliary function counts them per document, so the ORM needs no custom SQLite function or connection setup. Overlapping instances count once; for `PHRASE`, each whole phrase occurrence counts once. Before 0.91.0, SQLite used `bm25`, whose row count, average document length and per-term document frequency cover every row in the FTS table, including rows the grant hides.

**Parity.** Both scores grow with each occurrence and with the weight of the field it occurs in, so a match in A ranks above B, C and D, and a row matching in more fields ranks higher. The weighted-ranking fixtures order the same on both backends:

- a single-term query matching in A, B or C;
- a row matching in A and C;
- a two-term query with both terms in A, versus one term in B and the other in C.

The backends differ in two ways:

- **Weights.** PostgreSQL's default weights are A 1.0, B 0.4, C 0.2 and D 0.1. SQLite keeps 1.0, 0.7, 0.4 and 0.1. So repeated matches in a lower weight overtake one higher-weight match sooner on SQLite: four C matches score 1.6 against one A match's 1.0, while PostgreSQL scores 0.8 against 1.0.
- **Proximity.** `ts_rank_cd` rewards query terms that occur close together (cover density); the SQLite score does not.

Scores are not comparable across backends, and `minScore` thresholds are backend-specific.

## Backend differences

| | SQLite | PostgreSQL |
| --- | --- | --- |
| Storage | FTS5 table `__graphql_orm_fts_<table>` | `__graphql_orm_search_<table>` with weighted `tsvector` and GIN |
| Score | weighted matched-phrase count from the row's own document, weights 1.0/0.7/0.4/0.1 (A to D) | `ts_rank_cd` over the row's own document |
| `remove_diacritics` | the `unicode61 remove_diacritics 2` tokenizer, or `porter unicode61 remove_diacritics 2` with `porter`. Without it, the default `unicode61` tokenizer still folds most Latin diacritics (its `remove_diacritics 1` mode), so `maori` matches `Māori` either way | documents and queries folded by the ORM (canonical decomposition, combining marks removed); no `unaccent` extension needed. Without it, accents are kept |
| Language | ignored | text search configuration, checked at plan time |

- **`remove_diacritics: true`** behaves the same on both backends: matching ignores accents.
- **`remove_diacritics: false`** is only guaranteed to keep accents on PostgreSQL. On SQLite it still matches most accented and plain forms, as the table shows. This is a known backend difference.
- **Scores aren't comparable across backends.** Ordering is promised only within one backend; see [ranking isolation](#ranking-isolation).
- **Scores depend only on each row's own document** on both backends. A relevance-ordered page shifts only when a row's own document changes between page requests.
- **`WEB`** is evaluated like `PLAIN` on SQLite. Hosts that do not want to offer it can [withhold it](#authorization).
- **`PHRASE` and `PREFIX` syntax** follows each engine.

## Backup and restore

graphql-orm-backup snapshots only the static entities the host passes to it.
- **Not in its snapshots:** runtime collection tables and every search table: `__graphql_orm_search_metadata`, `__graphql_orm_fts_<table>` and `__graphql_orm_search_<table>`. A graphql-orm-backup restore therefore never writes a runtime index or its readiness.
- **Whole-database copies** taken from one consistent snapshot (`pg_dump`, the SQLite backup API) contain rows, documents and readiness from the same instant, so a restored index is `ready` only with documents that match its rows.
- **Restoring runtime rows any other way**, for example a row-level import into a database whose index is already `ready`, does not update documents or reset readiness. Until [#280](https://github.com/Dastari/graphql-orm/issues/280) adds a reset, rebuild the index explicitly: migrate the collection without search, then with search again, so the index is recreated `building` and backfilled.

## Not included

Vector or semantic search, trigram fuzzy matching, cross-collection search (#213), highlighting and snippets.
