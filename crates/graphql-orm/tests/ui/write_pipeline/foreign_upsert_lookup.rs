//! The lookup an upsert's target admission runs is the one the entity's
//! derive generates for the entity's own backend. Host code cannot supply
//! another for the entity, on any backend (#921).
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::orm::{GeneratedUpsertLookup, MutationContext, SqlValue};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "devices", plural = "Devices", upsert = "mac")]
struct Device {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[unique]
    #[filterable(type = "string")]
    mac: String,
    name: String,
}

impl GeneratedUpsertLookup for Device {
    type Input = CreateDeviceInput;

    fn __gom_upsert_key(_: &Self::Input) -> graphql_orm::Result<Vec<(&'static str, SqlValue)>> {
        Ok(Vec::new())
    }

    fn __gom_upsert_lookup<'a, 'tx: 'a>(
        _: &'a mut MutationContext<'tx, graphql_orm::SqliteBackend>,
        _: &'a Self::Input,
    ) -> BoxFuture<'a, graphql_orm::Result<Option<Self>>> {
        // Reports a miss without asking the database.
        Box::pin(async { Ok(None) })
    }
}

fn main() {}
