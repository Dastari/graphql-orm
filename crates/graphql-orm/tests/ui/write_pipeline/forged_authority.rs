//! Host code cannot make the entity, the admitted request or the authority
//! that lets a mutation hook present its originating request: only the
//! runtime write pipeline's own admission produces them, from the entity's
//! generated configuration (#724, #921).
use graphql_orm::graphql::orm::{
    GeneratedWriteAuthority, GeneratedWriteEntity, GeneratedWriteKind, GeneratedWriteRequest,
};

fn main() {
    let _entity = GeneratedWriteEntity {
        entity_name: "Order",
        table_name: "orders",
    };
    let _request = GeneratedWriteRequest {
        kind: GeneratedWriteKind::Delete,
    };
    let _authority = GeneratedWriteAuthority {
        kind: GeneratedWriteKind::Delete,
    };
}
