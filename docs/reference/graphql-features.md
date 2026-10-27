---
title: "GraphQL feature selection"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-10-28
review_by: 2027-02-01
supersedes: []
---

# GraphQL feature selection

The workspace's async-graphql dependency disables defaults and supplies no
named features. Each package adds only its required capabilities:

- ORM: `dataloader`, `uuid`, `decimal`; `runtime-graphql` explicitly adds
  `dynamic-schema`. Default SQLite does not enable runtime GraphQL.
- AI: `uuid` and base static objects/inputs/errors/Json. Its ORM dependency
  contributes ORM's required facilities. Attachments do not need `tempfile`.
- Tool-profiles: base errors and parser; no UUID scalar contract or feature.
- Macros: no direct runtime dependency. Generated references go through
  `graphql_orm::async_graphql`. Standalone derive consumers explicitly select
  their loader/scalar facilities without upstream defaults.

No package contributes async-graphql `default`, `email-validator`, `graphiql`,
`playground` or `tempfile`. Ordinary upstream base APIs remain available.
[Migration instructions](../../MIGRATION.md#0780-to-0790-explicit-graphql-features)
name the intentional optional reexport/default break, `UploadValue.content`
byte-versus-file representation and explicit application opt-ins. UUID/Decimal
SDL and serialization, static/runtime operations, relation batching,
federation and authorization are preservation requirements; snapshots must
not change to accommodate a regression. No schema/data migration is needed.

## Independent consumer checks

The [locked host fixture](../../crates/graphql-orm/tests/fixtures/graphql-features/Cargo.toml)
is outside the root workspace and has no dev dependency that can mask feature
selection. Minimal and additive hosts share one lock and canonical reviewed
Auth 0.24 full revision. With `router-auth`, a host-owned
`Arc<agql_auth::AccessTokenValidator>` and `Arc<dyn agql_auth::ScopeMatch>`
compile through both public router constructors. Source assertions require
one Auth identity across direct host, ORM, AI and router.

```bash
python3 scripts/check-graphql-features.py --output .handoff/graphql-features
cargo test --manifest-path crates/graphql-orm/tests/fixtures/graphql-features/Cargo.toml --locked --no-default-features --features orm,sqlite,ai,profiles,auth,runtime,router,router-auth
cargo test --manifest-path crates/graphql-orm/tests/fixtures/graphql-features/Cargo.toml --locked --no-default-features --features orm,sqlite,ai,profiles,auth,runtime,router,router-auth,additive,probe-ui,probe-tempfile --all-targets
```

The graph checker covers no-backend/default SQLite, PostgreSQL, MSSQL,
`auth-agql`, explicit runtime/descriptor and AI provider lanes, profiles and
Auth alone, plus combined and additive hosts. It checks required features,
forbidden default/UI/email/tempfile features, and absence of MPL email
packages in normal/build trees. Profiles and Auth alone must not acquire UUID,
loaders, Decimal or dynamic schema. Provider/TLS flags retain their existing
selection; this is not a TLS or dependency-family upgrade.

Two negative compile probes must fail without `additive` and pass with it:

```bash
cargo check --manifest-path crates/graphql-orm/tests/fixtures/graphql-features/Cargo.toml --locked --features probe-ui --example ui
cargo check --manifest-path crates/graphql-orm/tests/fixtures/graphql-features/Cargo.toml --locked --features probe-tempfile --example upload-file
```

A build with no ORM backend is also intentionally refused; its feature graph
remains inspectable. Select one supported backend for compilation.

The first fails because `GraphiQLSource` is gated off; the second because an
`std::fs::File` cannot initialize byte-backed upload content. With `additive`
(`graphiql,tempfile`) both compile. This host deliberately adds no email/MPL
edge. Hosts opting into `email-validator` separately must review its MPL-2.0
dependencies. Feature graph and compile evidence is bounded to the selected
locked lanes, not a broad compatibility claim for arbitrary host graphs.
