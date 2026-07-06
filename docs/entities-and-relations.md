# Entities And Relations

Entities are Rust structs annotated with `GraphQLEntity`. The derive macro turns the struct into a
GraphQL object plus the runtime metadata needed for queries, filters, ordering, schema models, and
migrations.

## Basic Entity

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "users", plural = "Users", default_sort = "name ASC")]
pub struct User {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "boolean")]
    pub active: bool,
}
```

## Persisted Column Names

Use `#[graphql_orm(db_column = "...")]` when a legacy column name differs from the Rust field name:

```rust
#[graphql(name = "CardNo")]
#[graphql_orm(db_column = "CardNo", write = false)]
#[filterable(type = "number")]
#[sortable]
pub card_no: i32,
```

`#[graphql(name = "...")]` controls the GraphQL field name. `db_column` controls SQL rendering.

## Primary Keys

Single primary-key entities keep the existing API:

- `PRIMARY_KEY`
- `find_by_id`
- `get`
- one GraphQL lookup argument

Composite primary keys are declared by marking more than one field:

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(table = "JimLabour", plural = "JimLabourEntries")]
pub struct JimLabourEntry {
    #[primary_key]
    #[graphql(name = "JimObjectType")]
    #[graphql_orm(db_column = "JimObjectType", write = false)]
    pub jim_object_type: i32,

    #[primary_key]
    #[graphql(name = "RefNo")]
    #[graphql_orm(db_column = "RefNo", write = false)]
    pub ref_no: i32,

    #[primary_key]
    #[graphql(name = "LineNum")]
    #[graphql_orm(db_column = "LineNum", write = false)]
    pub line_num: i16,
}
```

Composite-key lookups use one argument per key field:

```graphql
query {
  jimLabourEntry(jimObjectType: 1, refNo: 12345, lineNum: 2) {
    jimObjectType
    refNo
    lineNum
  }
}
```

Runtime metadata exposes all keys through `PRIMARY_KEYS` and `Entity::metadata().primary_keys`.
`PRIMARY_KEY` remains the first key for compatibility.

Composite-key writes are not generated yet. SQLite/Postgres read support works, and MSSQL remains
read-only.

## Full-Text Search

Mark text fields as searchable with `#[graphql_orm(searchable(...))]`. Add an entity-level
`search(...)` attribute when you need non-default language/tokenizer options:

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "articles", plural = "Articles", backend = "postgres")]
#[graphql_orm(search(index = true, language = "english"))]
pub struct Article {
    #[primary_key]
    pub id: uuid::Uuid,

    #[graphql_orm(searchable(weight = "A"))]
    #[filterable(type = "string")]
    #[sortable]
    pub title: String,

    #[graphql_orm(searchable(weight = "B"))]
    pub body: Option<String>,
}
```

Searchable fields must be `String` or `Option<String>`. Private fields are rejected. Fields with a
read policy require an explicit search policy:

```rust
#[graphql_orm(read_policy = "article.body.read")]
#[graphql_orm(searchable(weight = "B", policy = "article.body.search"))]
pub body: Option<String>,
```

Persisted JSON fields can contribute selected string paths to the same search document with
`search_json(...)`. Extraction happens in Rust from the entity value, so Postgres and SQLite keep the
same public API and continue to use their existing managed search storage:

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "records", plural = "Records", backend = "sqlite")]
#[graphql_orm(search(index = true))]
pub struct Record {
    #[primary_key]
    pub id: String,

    #[graphql_orm(searchable(weight = "A"))]
    pub display_title: String,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    #[graphql_orm(search_json(path = "$.description.summary", weight = "C"))]
    #[graphql_orm(search_json(path = "$.historical.summary", weight = "C"))]
    #[graphql_orm(search_json(path = "$.classification.primary.label", weight = "B"))]
    #[graphql_orm(search_json(path = "$.classification.keywords[*].label", weight = "C"))]
    pub content: Content,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    #[graphql_orm(search_json(path = "$[*].value", weight = "C"))]
    pub tags: Vec<Tag>,
}
```

Supported JSON search paths are intentionally portable: `$.field`, `$.nested.field`,
`$.array[*].field`, and `$[*].field`. Multiple string matches are joined with spaces. Missing paths,
nulls, non-string scalars, empty arrays, and serialization failures contribute empty text. The
`weight` option uses the same `A`/`B`/`C`/`D` values as `searchable(...)` and defaults to `D`.

`search_json(...)` is only accepted on persisted JSON fields. Private fields are rejected. JSON
fields with a read policy require an explicit search policy:

```rust
#[graphql_orm(read_policy = "record.content.read")]
#[graphql_orm(json)]
#[graphql_orm(search_json(
    path = "$.description.summary",
    weight = "C",
    policy = "record.content.search"
))]
pub content: Content,
```

Generated GraphQL adds a per-entity search resolver:

```graphql
query {
  articlesSearch(
    search: { query: "melbourne park", mode: WEB }
    where: { published: { eq: true } }
    page: { limit: 20 }
  ) {
    edges {
      score
      node { id title }
    }
    pageInfo { totalCount }
  }
}
```

Generated Rust helpers use the same backend-neutral input:

```rust
let hits = Article::search_db(&database, SearchInput {
    query: "melbourne park".to_string(),
    mode: Some(SearchMode::Web),
    min_score: None,
})
.filter(ArticleWhereInput::default())
.limit(20)
.fetch_all()
.await?;
```

Search modes are `Plain`, `Phrase`, `Web`, and `Prefix`. Results order by relevance descending, then
the entity default sort where native SQL can apply it.

Postgres and SQLite FTS5 search push the match expression, score, count, limit, and offset into SQL
when the native structures exist. Authenticated PostgreSQL requests that include `DbAuthContext`
still use native search inside the same transaction-local auth context used by normal resolvers, so
database RLS policies and indexed search compose. SQLite can fall back to the deterministic Rust
search scorer when FTS structures are unavailable and fallback is enabled.

## Relations

Relation fields should generally be skipped from `SimpleObject` and exposed by `GraphQLRelations`:

```rust
#[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations, SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct Post {
    #[primary_key]
    pub id: String,

    pub author_id: String,

    #[graphql(skip)]
    #[relation(target = "User", from = "author_id", to = "id")]
    pub author: Option<User>,
}
```

Single-column relation syntax remains:

```rust
#[graphql(skip)]
#[relation(target = "Post", from = "id", to = "author_id", multiple)]
pub posts: Vec<Post>,
```

Related fields can be copied into the parent search document:

```rust
#[graphql(skip)]
#[relation(target = "City", from = "city_id", to = "id")]
#[graphql_orm(search_relation(fields = ["name", "country"], weight = "C"))]
pub city: Option<Box<City>>;
```

The first implementation records relation search metadata and keeps local searchable fields current
through generated writes. For relation-heavy documents, run an explicit search rebuild after large
related-data changes until deeper propagation is enabled.

## Composite Relation Keys

Composite relation keys use array syntax. The `from` entries are Rust fields on the source entity;
the `to` entries are target database columns:

```rust
#[graphql(skip, name = "Details")]
#[relation(
    target = "JimCardFileDetail",
    from = ["card_no", "cont_no"],
    to = ["CardNo", "ContNo"],
    multiple,
    emit_fk = false
)]
pub details: Vec<JimCardFileDetail>,
```

The macro validates:

- `from` and `to` have the same arity
- every source field exists on the source entity
- arity 1 keeps the existing single-column behavior

Target columns are metadata literals used for generated SQL. Invalid target names are caught by the
database when the generated query runs.

## Nested Relation Batching

Selected relation fields are loaded in batches by relation layer. A query shaped like
`JimCardFiles -> Contacts -> Details` performs:

1. one parent query for card files
2. one relation query for all selected contacts
3. one relation query for all selected details

This applies to single-key and composite-key relations. Relation-level `where`, `orderBy`, and
`page` arguments use the DataLoader batching path for supported scalar key parts.

Generated GraphQL relation resolvers are not one query per parent row. For each relation field,
the macro registers an async-graphql `DataLoader<RelationLoader<T, B>>` in `schema_builder`. Each
parent resolver submits a `CompositeRelationQueryKey`; async-graphql coalesces keys observed during
the same resolution wave, and the runtime groups compatible keys by relation, foreign-key columns,
filter signature, sort signature, page signature, and auth context. Belongs-to relations use the
same grouped loader with a single-row result per parent key.

Paged relation connections are pushed into SQL with `ROW_NUMBER() OVER (PARTITION BY ...)` and a
grouped count on SQLite, Postgres, and MSSQL-capable relation reads. This avoids loading every child
row for every selected parent when a nested field asks for the first page of a large relation. When a
relation has no page argument, the loader keeps the simpler batched query and groups the returned
rows in memory.

Nullable key semantics are explicit: if any source key part is `NULL`, that parent relation is not
loaded and resolves to `None` or an empty connection. SQL `NULL = NULL` matching is not inferred.

## Physical Foreign Keys

Relation metadata and physical foreign-key emission are separate. By default, relations emit
migration metadata for physical foreign keys where migrations are supported. Disable physical FK
emission for external schemas:

```rust
#[graphql(skip)]
#[relation(target = "Customer", from = "customer_id", to = "CustomerId", emit_fk = false)]
pub customer: Option<Customer>,
```

## Recursive Relations

For self-referential entities, use boxed singular links and keep relation fields skipped:

```rust
#[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations, SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct Place {
    #[primary_key]
    pub id: String,

    pub parent_id: Option<String>,

    #[graphql(skip)]
    #[relation(target = "Place", from = "parent_id", to = "id")]
    pub parent_place: Option<Box<Place>>,
}
```

## JSON And UUID Fields

UUID fields are supported across generated filters, metadata, CRUD, and migrations:

```rust
#[filterable(type = "uuid")]
pub stored_file_id: Option<graphql_orm::uuid::Uuid>,
```

Typed structured fields can be persisted as JSON with `#[graphql_orm(json)]`:

```rust
#[graphql_orm(json)]
#[filterable(type = "json")]
pub metadata: serde_json::Value,
```

## Spatial Fields

Spatial fields are exposed through Rust and GraphQL as GeoJSON geometry objects. Postgres stores
them as native PostGIS `geometry(<type>, <srid>)` columns. SQLite stores the same GeoJSON in `TEXT`
columns and evaluates spatial predicates in Rust.

```rust
#[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326, index = true))]
#[filterable(type = "spatial")]
pub location: serde_json::Value,
```

Supported geometry types are `Geometry`, `Point`, `LineString`, `Polygon`, `MultiPoint`,
`MultiLineString`, `MultiPolygon`, and `GeometryCollection`. The only supported spatial kind is
`geometry`; the default SRID is `4326`. Spatial Rust fields must be `serde_json::Value` or
`Option<serde_json::Value>`.

On Postgres, generated queries project spatial columns as GeoJSON using `ST_AsGeoJSON`. Generated
writes bind the GeoJSON value and wrap it with `ST_SetSRID(ST_GeomFromGeoJSON(...), srid)`. Invalid
GeoJSON or invalid geometry is reported by PostGIS.

On SQLite, generated migrations use `TEXT`, writes validate and store canonical GeoJSON, and spatial
predicates run in memory after rows are loaded. SQL-safe predicates in the same `where` input are
still pushed into SQL before the exact spatial check runs. This gives projects a portable spatial
field API without requiring a SQLite extension, but it is not spatial-indexed and should not be
treated as an efficient large-table geospatial query engine.

Spatial filters use `SpatialFilter`:

```graphql
places(where: {
  location: {
    contains: { type: "Point", coordinates: [144.96, -37.81] }
  }
})
```

The supported predicates are `equals`, `disjoint`, `intersects`, `touches`, `crosses`, `within`,
`contains`, and `overlaps`. Multiple predicates on one field are combined with `AND`. `isNull` emits
`IS NULL` or `IS NOT NULL` without binding a geometry. On Postgres, `disjoint` renders as
`NOT ST_Intersects(...)` so the planner can still use spatial-index-friendly planning. SQLite uses
the same predicate names with planar GeoJSON topology checks in Rust.

When `index = true` on Postgres, migrations create a GiST spatial index:

```sql
CREATE INDEX idx_places_location_spatial ON places USING GIST (location)
```

Managed migrations enable PostGIS with `CREATE EXTENSION IF NOT EXISTS postgis` when any entity uses a
spatial column. Managed migrations do not use `CREATE INDEX CONCURRENTLY`; plan a separate operational
migration if a production table needs a concurrent index build.

On SQLite, `index = true` is accepted for cross-backend schema portability but no spatial index is
created. Future SQLite indexing options are documented in [Backend Features](backends.md).
