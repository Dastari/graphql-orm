//! Relation batches that run as several statements on PostgreSQL: every
//! statement must carry the caller's database auth context.
#![cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]

#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::async_graphql::dataloader::Loader;
use graphql_orm::graphql::loaders::{
    CompositeRelationQueryKey, RelationKey, RelationKeyPartKind, RelationLoader,
};
use graphql_orm::graphql::orm::{
    DbAuthContext, FilterExpression, PaginationRequest, PostgresBackend, SqlValue,
};
use graphql_orm::prelude::*;
use sqlx::Row;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "chunk_children",
    plural = "ChunkChildren",
    default_sort = "id ASC"
)]
pub struct ChunkChild {
    #[primary_key]
    #[sortable]
    pub id: String,
    #[graphql_orm(write = false)]
    pub part_a: i64,
    #[graphql_orm(write = false)]
    pub part_b: i64,
    #[graphql_orm(write = false)]
    pub part_c: i64,
    #[graphql_orm(write = false)]
    pub tenant: String,
}

impl BatchLoadEntity for ChunkChild {
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("id")
    }
}

schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [ChunkChild],
}

/// The relation's filter binds 31,000 values of the 32,000 one PostgreSQL
/// statement carries, which leaves room for 332 three-column keys, so 700
/// parents load as three statements.
const PARENTS: i64 = 700;
const FILTER_BINDS: usize = 31_000;

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn every_chunk_of_a_relation_batch_carries_the_auth_context()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("loader-chunks", 5).await?;
    // Each parent has one child per tenant. Visibility depends on a setting
    // that only the auth context installs: a statement run without it sees
    // `NULL` and returns no row.
    sqlx::query(
        "CREATE TABLE chunk_children (id TEXT PRIMARY KEY, part_a BIGINT NOT NULL, \
         part_b BIGINT NOT NULL, part_c BIGINT NOT NULL, tenant TEXT NOT NULL)",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunk_children (id, part_a, part_b, part_c, tenant) \
         SELECT tenant || '-' || n, n, n + 1, n + 2, tenant \
         FROM generate_series(1, $1) AS n CROSS JOIN (VALUES ('a'), ('b')) AS tenants(tenant)",
    )
    .bind(PARENTS)
    .execute(&pool)
    .await?;
    let database = graphql_orm::db::Database::<PostgresBackend>::new(pool.clone());
    let loader = RelationLoader::<ChunkChild, PostgresBackend>::new(database);

    let context = |tenant: &str| DbAuthContext {
        tenant_id: Some(tenant.to_string()),
        ..DbAuthContext::default()
    };
    let key = |n: i64, auth: Option<DbAuthContext>, paged: bool| {
        let values = vec![SqlValue::Int(n), SqlValue::Int(n + 1), SqlValue::Int(n + 2)];
        CompositeRelationQueryKey {
            relation: "chunk_children",
            parent_key: RelationKey::from_sql_values(&values),
            parent_values: values,
            fk_columns: vec!["part_a", "part_b", "part_c"],
            key_part_kinds: vec![RelationKeyPartKind::Int; 3],
            where_signature: Some("tenant".into()),
            order_signature: None,
            page_signature: paged.then(|| "first".into()),
            filter: Some(FilterExpression::trusted_fragment(
                format!(
                    "tenant = current_setting('app.tenant_id', true) AND part_a NOT IN ({})",
                    vec!["?"; FILTER_BINDS].join(", ")
                ),
                vec![SqlValue::Int(0); FILTER_BINDS],
            )),
            sorts: vec![],
            pagination: paged.then_some(PaginationRequest {
                limit: Some(5),
                offset: 0,
            }),
            auth_context: auth,
        }
    };

    for paged in [false, true] {
        for tenant in ["a", "b"] {
            let keys = (1..=PARENTS)
                .map(|n| key(n, Some(context(tenant)), paged))
                .collect::<Vec<_>>();
            let results = loader.load(&keys).await?;
            assert_eq!(results.len(), PARENTS as usize);
            // Parents 1, 400 and 700 are in three different statements.
            for (key, n) in keys.iter().zip(1..=PARENTS) {
                let page = &results[key];
                let ids = page
                    .entities
                    .iter()
                    .map(|child| child.id.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(
                    ids,
                    [format!("{tenant}-{n}")],
                    "paged: {paged}, tenant {tenant}, parent {n}"
                );
                assert_eq!(page.total_count, 1, "paged: {paged}, parent {n}");
            }
        }

        // Without a context no statement sees a tenant, in any chunk.
        let keys = (1..=PARENTS)
            .map(|n| key(n, None, paged))
            .collect::<Vec<_>>();
        let results = loader.load(&keys).await?;
        assert!(
            results
                .values()
                .all(|page| page.entities.is_empty() && page.total_count == 0),
            "paged: {paged}"
        );
    }
    Ok(())
}
