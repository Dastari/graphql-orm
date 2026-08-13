---
title: graphql-orm-router-protocol
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router-protocol

Versioned, serializable declarations that a GraphQL subgraph advertises to a
compatible router. It works with generated and hand-written Federation
services alike.

It is deliberately data-only: no HTTP server, router runtime, Federation
engine, ORM, database, URL parsing, DNS/network I/O, credential, deployment
override, or application type belongs here. Endpoint strings are inert
advertisements; a router owns SSRF, DNS, credential, and network policy.

## Install

This unpublished package is Git-only:

```toml
[dependencies]
graphql-orm-router-protocol = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.2.1" }
```

## Minimal descriptor route

Build deterministic bytes and let the host framework serve them at
`/.well-known/graphql-router` with `application/json`:

The canonical runnable source is
[`examples/handwritten_descriptor.rs`](examples/handwritten_descriptor.rs):

```sh
cargo run -p graphql-orm-router-protocol --example handwritten_descriptor
```

```rust
use graphql_orm_router_protocol::{CapabilitySet, Fingerprint, SubgraphDescriptorBuilder};

fn descriptor(schema_sdl: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let descriptor = SubgraphDescriptorBuilder::new(
        "inventory-service", "inventory", "https://inventory.example/graphql",
        "https://inventory.example/schema.graphql", Fingerprint::sha256(schema_sdl),
    )?.capabilities(CapabilitySet { schema_fingerprints: true, ..CapabilitySet::default() })
      .build()?;
    Ok(serde_json::to_vec(&descriptor)?)
}
# let _ = descriptor("type Query { inventory: Int! }")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Add root operation declarations with `operation` and project-neutral extension
payloads with `extension`; the builder canonicalizes and fingerprints output.

## Compatibility and authority boundary

V1 uses `protocolVersion: { "major": 1, "minor": 0 }`. Readers accept later
minor versions in the same major and ignore unknown additive JSON fields.
Semantics that a reader must understand belong in `requiredSemantics`; an
unknown required semantic or different major fails with stable
`ProtocolErrorKind` values.

`SubgraphOnly` remains available for authorization a router cannot represent.
A router allow is advisory defense in depth and never replaces the subgraph's
authoritative guard. Optional extensions are bounded and fingerprinted but do
not change authorization semantics by themselves.

## Reference and verification

Public model types are field-documented in source. `UnrepresentablePolicy` and
`UnrepresentablePolicyCode` are the protocol's policy-declaration reference in
[`src/model.rs`](src/model.rs); they declare information a router must leave to
the subgraph, not a router-side authorization grant. Golden generated-style and
hand-written descriptors live in [`tests/fixtures`](tests/fixtures). See the
[migration guide](MIGRATION.md) and [changelog](CHANGELOG.md) for protocol
compatibility.

```sh
cargo test --manifest-path crates/graphql-orm-router-protocol/Cargo.toml
cargo clippy --manifest-path crates/graphql-orm-router-protocol/Cargo.toml --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings -D missing_docs" cargo doc --manifest-path crates/graphql-orm-router-protocol/Cargo.toml --no-deps
```
