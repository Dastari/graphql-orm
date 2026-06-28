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

## PostGIS Spatial Fields

Spatial fields are currently implemented for PostgreSQL/PostGIS only. A spatial field persists as a
PostGIS `geometry(<type>, <srid>)` column and is exposed through Rust and GraphQL as a GeoJSON
geometry object.

```rust
#[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326, index = true))]
#[filterable(type = "spatial")]
pub location: serde_json::Value,
```

Supported geometry types are `Geometry`, `Point`, `LineString`, `Polygon`, `MultiPoint`,
`MultiLineString`, `MultiPolygon`, and `GeometryCollection`. The only supported spatial kind is
`geometry`; the default SRID is `4326`. Spatial Rust fields must be `serde_json::Value` or
`Option<serde_json::Value>`.

Generated queries project spatial columns as GeoJSON using `ST_AsGeoJSON`. Generated writes bind the
GeoJSON value and wrap it with `ST_SetSRID(ST_GeomFromGeoJSON(...), srid)`. Invalid GeoJSON or invalid
geometry is reported by PostGIS.

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
`IS NULL` or `IS NOT NULL` without binding a geometry. `disjoint` renders as
`NOT ST_Intersects(...)` so Postgres can still use spatial-index-friendly planning.

When `index = true`, migrations create a GiST spatial index:

```sql
CREATE INDEX idx_places_location_spatial ON places USING GIST (location)
```

Managed migrations enable PostGIS with `CREATE EXTENSION IF NOT EXISTS postgis` when any entity uses a
spatial column. Managed migrations do not use `CREATE INDEX CONCURRENTLY`; plan a separate operational
migration if a production table needs a concurrent index build.
