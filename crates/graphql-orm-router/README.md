---
title: graphql-orm-router
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-10-15
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router

A project-neutral Federation router for statically configured and explicitly
registered GraphQL subgraphs. It composes a complete candidate graph before
atomically publishing it; a bad refresh keeps the last-known-good executable
graph active.

It is not an ORM, subgraph authorization service, identity issuer, credential
store, persistence layer, or application policy engine. Router authorization
can deny early but never grants authority over subgraph resolver guards or data
policy.

## Install

This unpublished package is Git-only:

```toml
[dependencies]
graphql-orm-router = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.6.0" }
```

Version 0.6 makes `AuthenticationProvider::current_time` fallible. The agql
adapter uses its validator's exact injected clock, including subsecond precision,
for subscription admission and expiry scheduling. Clock failures reject a new
connection as service unavailable; they never select a fallback clock. Custom
provider migration is described in [MIGRATION.md](MIGRATION.md).

Enable `auth-agql` only when adapting a separately configured
`agql-auth::AccessTokenValidator`. The router never accepts private keys,
issues tokens, refreshes sessions, or decrypts credentials.
Upgrade deliberately by reviewing the router [migration guide](MIGRATION.md),
then validate the full graph with `--check` before deploying the new pin.

## Build prerequisites

The pinned Hive Router 0.2.19 dependency generates Laboratory HTML during its
default build. In a fresh Cargo output directory it runs
`npm install --include=dev` using its supplied `package.json` and
`package-lock.json`, then embeds the prebuilt Laboratory assets. Node.js and npm
must be available to the build; Hive declares npm 11.16.0 in `packageManager`.

Cargo's offline mode does not constrain npm network access or lifecycle scripts.
For controlled offline builds, prepare the locked npm dependencies in the cache,
configure npm itself for offline operation with scripts, audit and update checks
disabled, and verify that its lockfile remains unchanged. The upstream install
command alone does not enforce those restrictions.

Hive's separate `graphiql` feature selects static GraphiQL instead, skipping
Laboratory generation and ignoring Laboratory seed configuration.

## Minimal programmatic start

For trusted local development, anonymous mode is explicit. Production must
install an authentication provider and should use the strict file format.
The canonical source is [`examples/programmatic_router.rs`](examples/programmatic_router.rs):

```sh
cargo run -p graphql-orm-router --example programmatic_router
```

```rust,no_run
use graphql_orm_router::{RouterConfig, StaticSubgraph};

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let config = RouterConfig::new("127.0.0.1:4000".parse()?)
    .allow_anonymous_development(true)
    .with_subgraph(StaticSubgraph::new(
        "inventory", "http://127.0.0.1:8080/graphql", "http://127.0.0.1:8080/schema.graphql",
    ));
graphql_orm_router::run(config)?;
# Ok(())
# }
```

The router fetches and composes every source before opening the listener.
`prepare()` is available to inspect the process-local graph identity and
composition warnings before serving.

## Configuration, limits, and features

| Surface | Default or boundary |
| --- | --- |
| Authentication | Fail-closed; optional signed role expansion requires `auth-agql`. |
| Scope matching | Exact by default; hierarchical matching and exact-only super-scope policy require `auth-agql` and explicit file or programmatic configuration. |
| Public path | `/graphql`; `/health` and `/readiness` are also exposed. |
| Subgraphs | At least one static source; file configuration requires strict JSON. |
| Dynamic registration | Disabled unless authenticated administration and exact network policy are configured. |
| Subscriptions | Disabled by default; require authentication and a declared owner. |
| Prometheus | Disabled by default; deployment network policy protects its listener. |

The [configuration reference](docs/configuration.md) is the complete field,
secret-loading, default, and hard-limit source. Never put secret header values
in JSON: map header names to environment-variable names. `--check` performs
the same bounded startup validation without binding a listener:

```text
graphql-orm-router --config /etc/graphql-orm/router.json --check
graphql-orm-router --config /etc/graphql-orm/router.json
```

Quiet subscriptions release their upstream resources when the final public
consumer completes or disconnects, without requiring another upstream event.
A fresh connection authenticates again before opening a successor subscription.

Sensitive headers, arbitrary endpoint overrides, proxy bypass, private network
access, and stale JWKS cache use explicit deny-by-default policy. Static source
configuration is deployment-owned; dynamic destinations are additionally
subject to DNS, host, port, CIDR, redirect, and peer validation.

Authenticated subscriptions forward the ordinary variables object once. A
separate private authorization extension contains only a bounded projection of
small scalar variables; client-supplied values for that reserved extension are
always replaced. Large data variables are not duplicated across the private
transport, and a variable-dependent authorization decision fails closed if its
value cannot fit the projection or remaining frame budget.

## Errors and operations

Public errors are router-owned and sanitized: status does not reveal SDL,
endpoints, headers, tokens, keys, or private variables. Readiness requires an
active graph. On reload, in-flight requests remain pinned to their selected
graph; retired subscriptions end with the documented reload signal and must
reconnect with jittered backoff. Do not automatically replay an uncertain
mutation.

## Further reading

- [Documentation index](docs/README.md)
- [Configuration](docs/configuration.md) and [operations](docs/operations.md)
- [Schema evolution](docs/schema-evolution.md) and [threat model](docs/threat-model.md)
- [Migration guide](MIGRATION.md) and [changelog](CHANGELOG.md)
