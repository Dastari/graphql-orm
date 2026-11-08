use graphql_orm::prelude::*;
#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Target {
    #[primary_key]
    #[sortable]
    id: i32,
    #[graphql_orm(private)]
    secret: i32,
    #[graphql_orm(db_column = "tier_code")]
    tier: i32,
}
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    tier: i32,
    #[relation(
        target = "Target",
        from = "id",
        to = "ID",
        source_condition(field = "tier", equals = 7),
        target_condition(column = "TIER_CODE", equals = 7),
        emit_fk = false
    )]
    public: Option<Target>,
    #[graphql_orm(read = false)]
    #[relation(
        target = "Target",
        from = "id",
        to = "secret",
        target_condition(column = "secret", equals = 7),
        emit_fk = false
    )]
    hidden: Option<Target>,
    #[relation(target = "Manual", from = "id", to = "id", emit_fk = false)]
    custom: Option<Manual>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Manual;
impl graphql_orm::graphql::orm::GeneratedRelationReadability for Manual {
    const COLUMNS: &'static [(&'static str, bool)] = &[("id", true)];
}
fn main() {}
