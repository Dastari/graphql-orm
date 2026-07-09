# Postgres Test Coverage

Postgres is the primary compatibility target for generated schema management.
Run the Postgres tests against a disposable Postgres or PostGIS database, not
against application data.

For PostGIS-compatible coverage, start a throwaway local database:

```sh
docker run -d --name graphql-orm-postgis-test \
  -e POSTGRES_USER=graphql_orm \
  -e POSTGRES_PASSWORD=graphql_orm \
  -e POSTGRES_DB=graphql_orm_test \
  -p 55433:5432 \
  postgis/postgis:17-3.5
```

That database exposes:

```sh
postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test
```

From the repository root, point `TEST_DATABASE_URL` at the throwaway database
and run Postgres tests serially when sharing one database across integration
tests:

```sh
TEST_DATABASE_URL=postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test \
  cargo test -p graphql-orm --no-default-features --features postgres -- --test-threads=1
```

The Postgres suite covers:

- generated indexes for filterable columns used by where-inputs
- generated indexes for relation lookup columns used by relation resolvers
- active-schema introspection via `current_schema()` rather than a hard-coded
  `public` schema
- PostgreSQL type mappings for UUID, JSONB, TIMESTAMPTZ/date fields, and epoch
  timestamp integers
- transactional migration application and rollback on failed migrations

SQLite remains supported and should be run separately:

```sh
cargo test -p graphql-orm --no-default-features --features sqlite
```
