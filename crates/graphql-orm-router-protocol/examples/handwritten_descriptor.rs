use graphql_orm_router_protocol::{
    AuthorizationRequirement, CapabilitySet, Fingerprint, OperationDescriptor, RootOperationType,
    SubgraphDescriptorBuilder,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sdl = "type Query { health: String! }";
    let descriptor = SubgraphDescriptorBuilder::new(
        "health-service",
        "health",
        "https://health.example/graphql",
        "https://health.example/schema.graphql",
        Fingerprint::sha256(sdl),
    )?
    .capabilities(CapabilitySet {
        authorization_metadata: true,
        schema_fingerprints: true,
        ..CapabilitySet::default()
    })
    .require_semantic("authorizationMetadata")
    .operation(OperationDescriptor {
        root_type: RootOperationType::Query,
        field_name: "health".to_owned(),
        arguments: Vec::new(),
        authorization: AuthorizationRequirement::Public,
    })
    .build()?;

    // Return this JSON from GET `/.well-known/graphql-router` in the host's
    // existing HTTP framework. The protocol crate owns no server dependency.
    println!("{}", serde_json::to_string_pretty(&descriptor)?);
    Ok(())
}
