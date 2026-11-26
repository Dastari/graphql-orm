//! Host code cannot make the admission or the authority that lets a mutation
//! hook present its originating request: only the runtime write pipeline's
//! own admission issues them (#724).
use graphql_orm::graphql::orm::{
    EntityMetadata, GeneratedWriteAuthority, GeneratedWriteEntity, GeneratedWriteRequest,
};

fn metadata() -> &'static EntityMetadata {
    unimplemented!()
}

fn main() {
    let entity = GeneratedWriteEntity::new("Order", "orders", metadata);
    let _request = GeneratedWriteRequest { _admitted: () };
    let _authority = GeneratedWriteAuthority { entity };
}
