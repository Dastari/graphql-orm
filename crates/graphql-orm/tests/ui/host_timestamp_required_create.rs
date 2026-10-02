use graphql_orm::prelude::*;
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "required_clock", plural = "RequiredClocks")]
struct RequiredClock {
    #[primary_key]
    id: String,
    #[graphql_orm(timestamp = "host", default = "0")]
    created_at: i64,
}
fn main() {
    let _ = CreateRequiredClockInput {};
}
