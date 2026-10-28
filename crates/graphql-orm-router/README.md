---
title: graphql-orm-router
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-10-22
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router

The current workspace selects reviewed merged Auth 0.24.0 at full revision
`1f4b4bfb6d052d04e6c80b80cb7aa3d0c2ebaacd`. This is provisional Git source,
not a claim of a published Auth release. Direct Auth consumers must select the
same revision and refresh their locks to one source; see the
[workspace migration](../../MIGRATION.md#0780-to-0790-explicit-graphql-features).

Version `0.7.0` adopts that Auth identity across its public constructor inputs.
Hosts must rebuild with the identical Auth selector; see
[MIGRATION.md](MIGRATION.md). Routing, TLS/provider behavior and the Datadog
constraint are unchanged.

Version `0.6.7` keeps preparation-test listener addresses owned across schema
and protocol fetches. Invalid candidates still require their exact typed admission
errors before binding; a valid-candidate control prepares while the address is
occupied. It also constrains Datadog OpenTelemetry to `=0.5.2` with default
features disabled: pinned Hive `0.2.19` exchanges OpenTelemetry SDK `0.32` types,
whereas Datadog `0.5.3` selects SDK `0.33`. This constraint applies to fresh
consumers without relying on the workspace lockfile. Public features and behavior
are unchanged; no executable is distributed.

Version `0.6.6` records the documentation-only Auth `v0.23.0` selection
note added since the immutable `0.6.5` package tag. Rust code, routing and
authorization are unchanged. No router executable is distributed by this change.

Version `0.6.5` retains test-child startup errors in hosted CI diagnostics.
Production routing and authorization are unchanged; this source update does
not distribute or authorize a new router executable.

Version `0.6.4` selects the published TLS certificate-distrust patches in the
workspace lockfile. Rebuild with that lock and refresh downstream locks to retain
`rustls-webpki` 0.103.16 and `webpki-roots` / `webpki-root-certs` 1.0.10.
No SDL or stored-data migration is needed; see [MIGRATION.md](MIGRATION.md).

Version `0.6.3` raises the finite default signed role-catalogue HTTP response
bound to 2 MiB. This includes both the plaintext catalogue and its complete JWS;
explicit lower host limits still apply. Adoption also requires the coordinated
Auth signature-envelope correction. See [MIGRATION.md](MIGRATION.md).

Version `0.6.2` refreshes the immutable package identity for the maintained
workspace dependency guidance. The Rust implementation is unchanged; consuming
applications must still review and qualify the coordinated companion revision.

Version 0.6.1 selects published Auth `v0.21.3` for `auth-agql`, retaining the
same reviewed commit. Direct Auth dependencies must use that tag too; refresh
the lockfile and verify one Auth source. Existing clock and authorization
contracts remain unchanged. See [MIGRATION.md](MIGRATION.md).

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
graphql-orm-router = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.7.0" }
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

The in-tree federation wire tests forward spawned router-child stderr to the
test runner, preserving startup diagnostics when a child exits early.

`bash scripts/check-router-fresh-consumer.sh` builds rustdoc from a new standalone
consumer with no input lockfile, then verifies Datadog `0.5.2` and a single SDK
`0.32` dependency family. Its fresh manifest, resolved lock and graph metadata
remain available at the printed fixture path. Set `TMPDIR` for fixture placement
and `ROUTER_CONSUMER_CARGO` when a serialized Cargo runner is required.

The semver gate retains historical Rust source, package version and feature
definitions. `scripts/prepare-router-semver-baseline.py` adds only the same
Datadog constraint to a historical baseline pinned to Hive `=0.2.19`, which
otherwise admits the incompatible `0.5.3` patch. Both CI and the local semver
runner report this dependency-only adjustment explicitly and still compare the
historical default-feature API. Neither path copies a workspace lockfile into
consumer resolution or changes third-party source.

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
