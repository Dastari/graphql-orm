---
title: graphql-orm-router
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-12
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
graphql-orm-router = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.5.0" }
```

Enable `auth-agql` only when adapting a separately configured
`agql-auth::AccessTokenValidator`. The router never accepts private keys,
issues tokens, refreshes sessions, or decrypts credentials.
Upgrade deliberately by reviewing the router [migration guide](MIGRATION.md),
then validate the full graph with `--check` before deploying the new pin.

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

Sensitive headers, arbitrary endpoint overrides, proxy bypass, private network
access, and stale JWKS cache use explicit deny-by-default policy. Static source
configuration is deployment-owned; dynamic destinations are additionally
subject to DNS, host, port, CIDR, redirect, and peer validation.

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
