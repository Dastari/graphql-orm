#![cfg(feature = "mssql")]

#[path = "support/federation_sdl.rs"]
mod federation_sdl;

use federation_sdl::ParsedFederationSchema;
use graphql_orm::async_graphql::{EmptyMutation, EmptySubscription, SDLExportOptions, Schema};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.FederationLegacyDevices",
    plural = "LegacyDevices",
    default_sort = "[DeviceId] ASC"
)]
struct LegacyDevice {
    #[primary_key]
    #[graphql_orm(db_column = "DeviceId", write = false)]
    #[sortable]
    pub id: i64,

    #[graphql_orm(db_column = "DeviceName", write = false)]
    #[filterable(type = "string")]
    pub name: String,
}

schema_roots! {
    backend: "mssql",
    query_custom_ops: [],
    entities: [LegacyDevice],
}

#[test]
fn read_only_mssql_shape_exports_only_a_reachable_query_operation() {
    let schema = Schema::build(QueryRoot::default(), EmptyMutation, EmptySubscription).finish();
    let sdl = schema.sdl_with_options(SDLExportOptions::new().federation());
    let parsed = ParsedFederationSchema::parse(&sdl);

    assert_eq!(parsed.query, "Query");
    assert!(parsed.query_fields().contains("legacyDevices"));
    assert_eq!(parsed.mutation, None);
    assert_eq!(parsed.subscription, None);
    assert!(!parsed.objects.contains_key("QueryRoot"));
    assert!(!sdl.contains("SubscriptionRoot"));
}
