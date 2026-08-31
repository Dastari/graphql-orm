---
title: "GraphQL ORM workspace"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# GraphQL ORM

`graphql-orm` turns Rust entity definitions into an `async-graphql` API and
typed repository helpers. It generates filters, ordering, keyset pagination,
relations, schema metadata, and explicit migration plans while leaving the
application in control of its server, authentication, authorization, and
deployment.

Start with the [five-minute SQLite GraphQL quickstart](docs/learn/sqlite-quickstart.md).
It is a runnable Axum service with a managed SQLite schema, seed data, a
generated GraphQL query, and a smoke test.

## When it fits

Use this workspace when you want to:

- build a Rust + `async-graphql` service on SQLite or PostgreSQL, with explicit
  application-controlled schema ownership;
- expose a safe generated GraphQL read layer, or deliberately enabled
  generated DML, over an existing SQL Server schema; or
- compose independent storage, backup, AI, operation-catalog, and Federation
  router packages without making them dependencies of the core ORM.

It is not a hosted GraphQL service, an authentication provider, or a database
administration system. SQL Server compatibility constructors remain
physically read-only; generated DML requires the explicit `ExternalWritable`
connection and schema policy.
Schema construction never applies migrations; applications choose and execute
schema changes explicitly.

## Install

Packages are distributed from this repository, not crates.io. Pin the reviewed
release revision, not a moving branch or tag. The current coordinated
`graphql-orm` version is 0.29.0. Replace the placeholder below with the final
reviewed full SHA for the release:

```toml
[dependencies]
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.29.0", default-features = false, features = ["sqlite"] }
```

Choose exactly the backend support needed by each service. Cargo can unify
features in a shared dependency graph; when more than one backend is enabled,
declare the backend explicitly on each entity and `schema_roots!` block.
The [quickstart](docs/learn/sqlite-quickstart.md) separately pins its
repository snapshot because the checked-in example was added after this package
release.

## Backend and schema capability

| Backend | Reads | Generated writes | Managed migrations | Intended schema policy |
| --- | --- | --- | --- | --- |
| SQLite (`sqlite`) | Yes | Yes | Yes | `Managed` |
| PostgreSQL (`postgres`) | Yes | Yes | Yes | `Managed` or externally owned |
| Microsoft SQL Server (`mssql`) | Yes | Yes, when explicitly enabled | No | `ExternalReadOnly` or `ExternalWritable` |

Backend features compile database support. `SchemaPolicy` separately decides
whether the application owns the schema: `Managed`, `ValidateOnly`, `PlanOnly`,
`ExternalWritable`, or `ExternalReadOnly`. Read the [schema-management
reference](docs/reference/graphql-orm/schema-management.md) before connecting
to an existing database.

## Choose a package

| Package | Use it for | Start here |
| --- | --- | --- |
| `graphql-orm` | Generated GraphQL, repositories, migrations, and database runtime | [quickstart](docs/learn/sqlite-quickstart.md) |
| `graphql-orm-macros` | The derive macros re-exported by `graphql-orm` | [package README](crates/graphql-orm-macros/README.md) |
| `graphql-orm-operation-catalog` | Stable generated-operation metadata and fingerprints | [package README](crates/graphql-orm-operation-catalog/README.md) |
| `graphql-orm-storage` | Provider-neutral object storage contracts | [package docs](crates/graphql-orm-storage/docs/README.md) |
| `graphql-orm-backup` | Backup, verification, and restore orchestration | [package docs](crates/graphql-orm-backup/docs/README.md) |
| `graphql-orm-ai-tool-profiles` | Least-privilege AI tool declarations | [package README](crates/graphql-orm-ai-tool-profiles/README.md) |
| `graphql-orm-ai` | Project-neutral AI runtime and durable state | [package docs](crates/graphql-orm-ai/docs/README.md) |
| `graphql-orm-router-protocol` | Versioned project-neutral subgraph/router declarations | [package README](crates/graphql-orm-router-protocol/README.md) |
| `graphql-orm-router` | Federated GraphQL router runtime | [package README](crates/graphql-orm-router/README.md) |

The packages remain independently consumable. Adding one does not enable the
others as ORM features.

## Maturity and security boundaries

SQLite and PostgreSQL support managed schemas and generated writes. SQL Server
supports externally managed reads and deliberate `ExternalWritable` DML, but
not managed migrations or backups. The generated API supplies query limits and
policy hooks, but it does not make an application public-service ready by
itself: install authentication, authorization, disclosure policy, rate limits,
transport security, and operational controls in the host application.

The quickstart intentionally sets `auth: "none"` for a local learning server.
Do not deploy that configuration. See [authentication and
authorization](docs/architecture/authentication-and-authorization.md),
[operation assurance](docs/architecture/operation-assurance.md), and the
[GraphQL workflow](docs/development/graphql-workflow.md).

## Documentation

- [Learn](docs/learn/README.md) — begin with a working service.
- [How-to guides](docs/how-to/README.md) — choose a backend, manage schemas,
  work with entities, or operate companion packages.
- [Core ORM reference](docs/reference/graphql-orm/README.md) — runtime
  mechanics and configuration; use the [macro and attribute
  reference](docs/reference/graphql-orm/macros-and-attributes.md) for exact
  derive syntax.
- [Reference index](docs/reference/README.md) — all API and configuration
  material.
- [Concepts](docs/architecture/system-context.md) — package boundaries,
  persistence, auth, assurance, storage, and backup design.
- [Contributing and verification](docs/development/README.md) — workspace setup
  and maintainer checks.

The [documentation index](docs/README.md) is the canonical navigation point.
