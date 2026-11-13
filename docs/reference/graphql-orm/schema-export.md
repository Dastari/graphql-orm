---
title: Dedicated schema export builds
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-06
review_by: 2027-02-01
supersedes: []
---

# Dedicated schema export builds

`graphql-orm-macros` 0.92.0 offers `schema-export-only`, off by default.
`graphql-orm` neither enables nor forwards it. In a dedicated export package,
add a direct dependency on the same macro package with `default-features = false`
and retain the runtime backend selection. Forward the export feature explicitly:

```toml
[features]
export = ["graphql-orm-macros/schema-export-only"]
```

Build the export executable with `cargo run --features export`. Build the schema
normally and call `Schema::sdl()`; no resolver or database call is needed.

Cargo features are additive and unified for a package instance. If any crate
enables this feature, every consumer of that same macro package instance receives
export stubs. Use a dedicated export build invocation, never a production server
build. Check `cargo tree -e features -i graphql-orm-macros` to detect accidental
activation. Disabling it returns the original generated tokens before any parsing
or reprinting by the export pass.

The pass processes only `GraphQLOperations`, `GraphQLRelations`, and the operations
portion of `RepositoryEntity`. It replaces async functions and functions returning
Future or Stream types (including boxed forms). Calling a synchronous stub panics
immediately; an async stub panics when polled. It preserves signatures, attributes,
visibility, generics, fields, input defaults, policies, metadata, synchronous
constructors, and schema/DataLoader registration. `GraphQLEntity` and
`schema_roots!` are untouched. This mode grants no runtime authority.

Opaque `impl Stream<Item = T>` and `impl Future<Output = T>` retain unreachable
concrete witnesses, including inside `Result`. Other opaque wrappers fail with
`schema-export-only: unsupported opaque return wrapper` rather than inventing a
public wrapper type. Associated Item/Output types must be concrete. Additional
bounds still undergo ordinary Rust type checking.

Run `scripts/check-schema-export.sh` to compile panic/witness and diagnostic probes
and compare full-stack fixture SDL bytes with the feature off/on for SQLite and
PostgreSQL. This script is an owned CI lane and does not connect to a database.

Run `scripts/compare-generated-schema.sh <base-ref>` before merging a change to
what the derives emit when it must not alter the schema. It compiles the
full-stack fixture against the base revision and against the working tree, and
compares the SDL, the operation catalogue and the semantic catalogue byte for
byte, for SQLite and PostgreSQL. It needs no database and is not a CI lane.
