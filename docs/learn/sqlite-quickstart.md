---
title: SQLite GraphQL quickstart
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# SQLite GraphQL quickstart

In a few minutes, run a local GraphQL service that derives its schema and
repository helpers from one Rust struct. The canonical source is
[`crates/graphql-orm/examples/sqlite_quickstart.rs`](../../crates/graphql-orm/examples/sqlite_quickstart.rs);
the commands below execute and test that exact file.

## Run the example

Clone the release and run its smoke test before starting the server:

```bash
git clone https://github.com/Dastari/graphql-orm.git
cd graphql-orm
git checkout graphql-orm-v0.21.0
cargo test -p graphql-orm --example sqlite_quickstart
cargo run -p graphql-orm --example sqlite_quickstart
```

The server listens only on `127.0.0.1:3000` and writes `quickstart.db` in the
current directory. In a second terminal, issue a GraphQL request:

```bash
curl http://127.0.0.1:3000/graphql \
  --header 'content-type: application/json' \
  --data '{"query":"{ tasks { edges { node { title completed } } pageInfo { totalCount } } }"}'
```

It returns two seeded tasks, including `Learn graphql-orm` and `Run the
quickstart`, with `totalCount: 2`.

## What the example does

The [`Task` entity](../../crates/graphql-orm/examples/sqlite_quickstart.rs)
uses `GraphQLEntity` and `GraphQLOperations`. `schema_roots!` creates the
generated query and mutation roots. At startup, the example:

1. connects a SQLite `Database` with `SchemaPolicy::Managed`;
2. explicitly plans and applies the initial `tasks` migration;
3. inserts two rows with generated `Task::insert_many` helpers;
4. builds the generated async-graphql schema; and
5. serves `POST /graphql` and `GET /health` through Axum.

The embedded smoke test builds an in-memory database and executes the same
generated `tasks` query. Keep application examples close to this shape so the
test and the published guidance cannot silently drift apart.

## Use it in a separate application

Copy the canonical source into your application and give it these direct
dependencies. The ORM revision below is the exact revision for
`graphql-orm` 0.21.0; review and update it deliberately when upgrading.

```toml
[dependencies]
async-graphql = "7"
axum = "0.8.9"
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "fac98d99e64c841a34d2d0096cdf928c3f9a7c6f", version = "0.21.0", default-features = false, features = ["sqlite"] }
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["macros", "net", "rt-multi-thread"] }
```

The source uses no application-specific names or framework adapter beyond
Axum. You can replace the HTTP layer while keeping the explicit connection,
migration, seed, and `schema_builder` steps.

## Security note

The example deliberately declares `auth: "none"` at both the entity and root
level, so a newcomer can make a local request without credentials. That is an
explicit public-demo choice, not a production recommendation. Before binding a
real service beyond a local interface, provide application-owned
authentication, authorization and row/field policy, request limits, transport
security, error handling, observability, and an appropriate migration process.
Read [authentication and authorization](../architecture/authentication-and-authorization.md)
and [operation assurance](../architecture/operation-assurance.md) before
exposing mutations.

## Next steps

- [Model entities and relations](../reference/graphql-orm/entities-and-relations.md)
- [Plan and apply schema changes](../reference/graphql-orm/schema-management.md)
- [Choose a backend](../reference/graphql-orm/backends.md)
- [Find a task-oriented guide](../how-to/README.md)
