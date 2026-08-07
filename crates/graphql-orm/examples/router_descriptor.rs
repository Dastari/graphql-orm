#[cfg(all(feature = "sqlite", feature = "router-protocol"))]
use graphql_orm::prelude::*;

#[cfg(all(feature = "sqlite", feature = "router-protocol"))]
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "router_example_products",
    plural = "RouterExampleProducts"
)]
struct RouterExampleProduct {
    #[primary_key]
    #[filterable(type = "uuid")]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    name: String,
}

#[cfg(all(feature = "sqlite", feature = "router-protocol"))]
schema_roots! {
    auth: "none",
    backend: "sqlite",
    entities: [RouterExampleProduct],
}

#[cfg(all(feature = "sqlite", feature = "router-protocol"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm_router_protocol::{CapabilitySet, Fingerprint, SubgraphDescriptorBuilder};

    // The finished host schema is authoritative; this compact SDL stands in
    // for `schema.sdl()` in a complete async-graphql service.
    let finished_sdl = "type Query { routerExampleProduct(id: UUID!): RouterExampleProduct }";
    let mut builder = SubgraphDescriptorBuilder::new(
        "products-service",
        "products",
        "https://products.example/graphql",
        "https://products.example/schema.graphql",
        Fingerprint::sha256(finished_sdl),
    )?
    .capabilities(CapabilitySet {
        authorization_metadata: true,
        schema_fingerprints: true,
        ..CapabilitySet::default()
    })
    .require_semantic("authorizationMetadata");

    for operation in graphql_orm_operation_catalog().router_protocol_operations()? {
        builder = builder.operation(operation);
    }
    let descriptor = builder.build()?;
    println!("{}", serde_json::to_string_pretty(&descriptor)?);
    Ok(())
}

#[cfg(not(all(feature = "sqlite", feature = "router-protocol")))]
fn main() {}
