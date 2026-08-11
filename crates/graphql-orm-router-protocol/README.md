---
title: graphql-orm-router-protocol
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router-protocol

`graphql-orm-router-protocol` defines versioned, serializable declarations a
GraphQL subgraph can advertise to a compatible router. It is usable by
`graphql-orm`-generated and hand-written Federation services alike.

The crate intentionally contains no router runtime, HTTP server, Federation
engine, ORM, database, authentication implementation, or application types.

## Boundary

Endpoint strings are service advertisements only. This crate does not parse
URLs, perform DNS or network I/O, resolve redirects, retain credentials, or
apply deployment overrides. A router must bind an advertised endpoint to its
own network, SSRF, credential, and registration policy before connecting.

The descriptor includes a stable subgraph identity, GraphQL and SDL endpoint
advertisements, capabilities, root operation and argument declarations,
authorization metadata, and schema, authorization, and combined fingerprints.
`SubgraphOnly` authorization explicitly marks policy that the router cannot
represent; a router allow never replaces the subgraph's authoritative guard.
The authorization fingerprint canonically covers each root field's
authorization metadata and the argument declarations referenced by scope
templates, including argument type and requiredness. Other argument drift is
detected by the schema and combined fingerprints without changing the
authorization fingerprint.

Optional `DescriptorExtension` values carry project-neutral, extension-owned
JSON payloads. The protocol bounds and canonicalizes each payload, validates a
positive version and lower-case identity, and binds it into the combined
fingerprint without interpreting it. Consumers of a named extension must
reject unsupported or incomplete inner versions; an extension never changes
router authorization semantics by itself.

## Compatibility

V1 uses `protocolVersion: { "major": 1, "minor": 0 }`. Readers accept later
minor versions in the same major and ignore unknown additive JSON fields.
Producers must place semantics a reader must understand in `requiredSemantics`.
An unknown required semantic and any different major fail with stable
`ProtocolErrorKind` categories.

## Framework-neutral host route

```rust
use graphql_orm_router_protocol::{
    CapabilitySet, Fingerprint, SubgraphDescriptorBuilder,
};

// A host framework can return these bytes from a GET route at
// `/.well-known/graphql-router` with `application/json` content type.
fn router_descriptor_json(schema_sdl: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let descriptor = SubgraphDescriptorBuilder::new(
        "inventory-service",
        "inventory",
        "https://inventory.example/graphql",
        "https://inventory.example/schema.graphql",
        Fingerprint::sha256(schema_sdl),
    )?
    .capabilities(CapabilitySet {
        schema_fingerprints: true,
        ..CapabilitySet::default()
    })
    .build()?;

    Ok(serde_json::to_vec(&descriptor)?)
}
# let _ = router_descriptor_json("type Query { inventory: Int! }")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

The service's chosen HTTP framework owns routing and response construction;
the protocol package intentionally does not. Operation declarations can be
added with `SubgraphDescriptorBuilder::operation`, and the builder canonicalizes
them and calculates authorization and combined fingerprints before validation.
Optional extensions are added with `SubgraphDescriptorBuilder::extension`.

See the golden generated-style and hand-written descriptors under
[`tests/fixtures`](tests/fixtures).

See the [migration guide](MIGRATION.md) for protocol-major/minor compatibility
and first adoption.

## Verification

```sh
cargo test --manifest-path crates/graphql-orm-router-protocol/Cargo.toml
cargo clippy --manifest-path crates/graphql-orm-router-protocol/Cargo.toml --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings -D missing_docs" cargo doc --manifest-path crates/graphql-orm-router-protocol/Cargo.toml --no-deps
```
