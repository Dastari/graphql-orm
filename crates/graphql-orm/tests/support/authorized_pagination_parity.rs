use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};
#[cfg(feature = "mssql")]
type Backend = MssqlBackend;
#[cfg(not(feature = "mssql"))]
type Backend = PostgresBackend;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "pagination_parity",
        plural = "PaginationParity",
        schema_policy = "external_read_only",
        keyset = "bucket asc nulls last, ordinal asc, id asc"
    )
)]
#[cfg_attr(
    not(feature = "mssql"),
    graphql_entity(
        backend = "postgres",
        table = "pagination_parity",
        plural = "PaginationParity",
        schema_policy = "external_read_only",
        keyset = "bucket asc nulls last, ordinal asc, id asc"
    )
)]
struct ParityItem {
    #[primary_key]
    id: String,
    #[filterable(type = "number")]
    #[sortable]
    ordinal: i64,
    bucket: Option<i32>,
    #[filterable(type = "string")]
    owner: String,
}
struct Policy {
    complete: bool,
}
impl RowPolicy<Backend> for Policy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            if !self.complete {
                return Ok(ReadVisibility::CallbackOnly);
            }
            Ok(ReadVisibility::Complete(ReadPredicate::from_filter::<
                Backend,
                _,
            >(
                &ParityItemWhereInput {
                    owner: Some(StringFilter {
                        eq: Some("a".into()),
                        ..Default::default()
                    }),
                    ordinal: Some(IntFilter {
                        gte: Some(10),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )?))
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let row = row.downcast_ref::<ParityItem>().unwrap();
            Ok(row.owner == "a" && row.ordinal >= 20)
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<(String, usize)>>>);
impl ReadQueryObserver for Capture {
    fn on_read(&self, sql: &str, rows: usize) {
        self.0.lock().unwrap().push((sql.into(), rows));
    }
}

pub async fn verify(mut db: Database<Backend>) {
    let capture = Capture::default();
    db = db
        .with_read_query_observer(capture.clone())
        .with_authorized_scan_config(AuthorizedScanConfig::new([9; 32], 4, 7));
    let grants = ReadPredicate::from_filter::<Backend, _>(&ParityGrantWhereInput {
        subject: Some(StringFilter {
            eq: Some("reader".into()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    let related =
        ReadPredicate::related::<Backend, ParityItem, ParityGrant>(&[("owner", "owner")], grants)
            .unwrap();
    assert_eq!(
        EntityQuery::<ParityItem, Backend>::new()
            .with_read_visibility(&ReadVisibility::Complete(related))
            .unwrap()
            .count_with_auth(&db, None)
            .await
            .unwrap(),
        20
    );
    db.set_row_policy(Policy { complete: true });
    let filter = ParityItemWhereInput {
        ordinal: Some(IntFilter {
            lte: Some(30),
            ..Default::default()
        }),
        ..Default::default()
    };
    let page = ParityItem::keyset_page(
        &db,
        filter.clone(),
        KeysetPageInput {
            limit: Some(3),
            include_total_count: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.page_info.total_count, Some(11));
    assert_eq!(
        page.edges
            .iter()
            .map(|e| e.node.ordinal)
            .collect::<Vec<_>>(),
        [10, 12, 14]
    );
    let next = ParityItem::keyset_page(
        &db,
        filter,
        KeysetPageInput {
            limit: Some(3),
            after: page.page_info.end_cursor,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(next.edges[0].node.ordinal, 16);
    db.set_row_policy(Policy { complete: false });
    let mut after = None;
    let mut seen = Vec::new();
    for index in 0..30 {
        let page = ParityItem::authorized_scan(
            &db,
            ParityItemWhereInput::default(),
            AuthorizedScanInput {
                limit: Some(3),
                after,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        if index == 0 {
            assert!(page.nodes.is_empty());
            assert_eq!(page.page_info.status, ScanStatus::BudgetExhausted);
        }
        seen.extend(page.nodes.iter().map(|n| n.ordinal));
        after = page.page_info.continuation;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(seen, (20..40).step_by(2).collect::<Vec<_>>());
    for (sql, rows) in capture.0.lock().unwrap().iter() {
        if !sql.contains("COUNT") {
            assert!(sql.contains("LIMIT") || sql.contains("FETCH NEXT"), "{sql}");
            assert!(*rows <= 4);
        }
    }
}

pub fn seed_sql() -> String {
    let mut sql = "CREATE TABLE pagination_parity (id VARCHAR(40) PRIMARY KEY, ordinal BIGINT NOT NULL, bucket INTEGER, owner VARCHAR(10) NOT NULL);".to_string();
    for i in 0..40 {
        sql.push_str(&format!(
            "INSERT INTO pagination_parity VALUES ('{i:04}', {i}, {}, '{}');",
            if i < 30 { "1" } else { "NULL" },
            if i % 2 == 0 { "a" } else { "b" }
        ));
    }
    sql.push_str("CREATE TABLE pagination_grants (id VARCHAR(40) PRIMARY KEY, owner VARCHAR(10), subject VARCHAR(40)); INSERT INTO pagination_grants VALUES ('grant', 'a', 'reader');");
    sql
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "pagination_grants",
        plural = "PaginationGrants",
        schema_policy = "external_read_only"
    )
)]
#[cfg_attr(
    not(feature = "mssql"),
    graphql_entity(
        backend = "postgres",
        table = "pagination_grants",
        plural = "PaginationGrants",
        schema_policy = "external_read_only"
    )
)]
struct ParityGrant {
    #[primary_key]
    id: String,
    owner: String,
    #[filterable(type = "string")]
    #[sortable]
    subject: String,
}
