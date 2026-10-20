#![cfg(feature = "mssql")]

#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;

use async_graphql::SimpleObject;
use graphql_orm::prelude::*;
use graphql_orm::rust_decimal::Decimal;
use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;
use tokio::net::TcpStream;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    table = "dbo.GraphqlOrmMssqlCustomers",
    plural = "MssqlCustomers",
    default_sort = "[CustomerName] ASC"
)]
#[graphql_orm(projection(
    name = "MssqlCustomerMoneyProjection",
    fields = [id, credit_limit],
    private = true
))]
pub struct MssqlCustomer {
    #[primary_key]
    #[graphql_orm(db_column = "CustomerId", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i64,

    #[graphql_orm(db_column = "CustomerName", write = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub customer_name: String,

    #[graphql_orm(db_column = "CreditLimit", mssql_money, write = false)]
    pub credit_limit: Decimal,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for MssqlCustomer {
    fn batch_column() -> &'static str {
        "[CustomerId]"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get::<i64, _>("CustomerId")
            .map(|value| value.to_string())
    }
}

#[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations, SimpleObject, Clone, Debug)]
#[graphql_entity(
    table = "dbo.GraphqlOrmMssqlJobs",
    plural = "MssqlJobs",
    default_sort = "[JobId] ASC"
)]
#[graphql(complex)]
pub struct MssqlJob {
    #[primary_key]
    #[graphql_orm(db_column = "JobId", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i64,

    #[graphql_orm(db_column = "CustomerId", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub customer_id: i64,

    #[graphql_orm(db_column = "JobName", write = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub job_name: String,

    #[graphql_orm(db_column = "Amount", mssql_money, write = false)]
    pub amount: Option<Decimal>,

    #[graphql_orm(db_column = "IsClosed", write = false)]
    #[filterable(type = "boolean")]
    pub closed: bool,

    #[graphql_orm(db_column = "StartedAt", write = false)]
    #[filterable(type = "date")]
    #[sortable]
    pub started_at: Option<String>,

    #[graphql_orm(db_column = "CompletedAt", write = false)]
    #[filterable(type = "date")]
    pub completed_at: Option<String>,

    #[graphql(skip)]
    #[relation(
        target = "MssqlCustomer",
        from = "customer_id",
        to = "CustomerId",
        emit_fk = false
    )]
    pub customer: Option<MssqlCustomer>,
}

schema_roots! {
    query_custom_ops: [],
    entities: [MssqlCustomer, MssqlJob],
}

async fn exec_batch(connection_string: &str, sql: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = graphql_orm::tiberius::Config::from_ado_string(connection_string)?;
    let tcp = TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;
    let mut client = graphql_orm::tiberius::Client::connect(config, tcp.compat_write()).await?;
    client.simple_query(sql).await?.into_results().await?;
    Ok(())
}

async fn setup_database(connection_string: &str) -> Result<(), Box<dyn std::error::Error>> {
    exec_batch(
        connection_string,
        r#"
        IF OBJECT_ID(N'dbo.GraphqlOrmMssqlJobs', N'U') IS NOT NULL
            DROP TABLE dbo.GraphqlOrmMssqlJobs;
        IF OBJECT_ID(N'dbo.GraphqlOrmMssqlCustomers', N'U') IS NOT NULL
            DROP TABLE dbo.GraphqlOrmMssqlCustomers;

        CREATE TABLE dbo.GraphqlOrmMssqlCustomers (
            CustomerId BIGINT NOT NULL PRIMARY KEY,
            CustomerName NVARCHAR(100) NOT NULL,
            CreditLimit MONEY NOT NULL
        );

        CREATE TABLE dbo.GraphqlOrmMssqlJobs (
            JobId BIGINT NOT NULL PRIMARY KEY,
            CustomerId BIGINT NOT NULL,
            JobName NVARCHAR(100) NOT NULL,
            Amount MONEY NULL,
            IsClosed BIT NOT NULL,
            StartedAt DATE NULL,
            CompletedAt DATETIME2 NULL
        );

        INSERT INTO dbo.GraphqlOrmMssqlCustomers (CustomerId, CustomerName, CreditLimit)
        VALUES (10, N'Acme', 922337203685477.5807),
               (20, N'Globex', -922337203685477.5808),
               (30, N'Zeta', 900719925474.0993),
               (40, N'Zero', 0.0000);

        INSERT INTO dbo.GraphqlOrmMssqlJobs
            (JobId, CustomerId, JobName, Amount, IsClosed, StartedAt, CompletedAt)
        VALUES
            (1, 10, N'Pump repair', 0.0001, 0, '2026-01-03', NULL),
            (2, 10, N'Pump audit', -0.0001, 0, '2026-01-04', NULL),
            (3, 20, N'Valve replacement', NULL, 1, '2025-12-01', '2025-12-02T08:15:00');
        "#,
    )
    .await
}

#[tokio::test]
#[ignore = "requires test-owned SQL Server container"]
async fn mssql_generated_queries_read_existing_tables() -> Result<(), Box<dyn std::error::Error>> {
    let owned = owned_mssql::OwnedSqlServer::start().await?;
    let connection_string = owned.connection_string.clone();

    setup_database(&connection_string).await?;

    let pool = graphql_orm::db::mssql::MssqlPool::connect_ado(&connection_string).await?;
    let database = graphql_orm::db::Database::new(pool);
    let projected = MssqlCustomerMoneyProjection::query(&database)
        .limit(4)
        .fetch_all()
        .await?;
    assert_eq!(projected.len(), 4);
    let projected = projected
        .into_iter()
        .map(|customer| (customer.id, customer.credit_limit.to_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(projected[&10], "922337203685477.5807");
    assert_eq!(projected[&20], "-922337203685477.5808");
    assert_eq!(projected[&30], "900719925474.0993");
    assert_eq!(projected[&40], "0.0000");
    let schema = schema_builder(database)
        .data("test-user".to_string())
        .finish();

    let list_response = schema
        .execute(
            r#"
            query {
                mssqlJobs(
                    where: {
                        jobName: { contains: "Pump" }
                        customerId: { eq: 10 }
                        startedAt: { gte: "2026-01-01" }
                        completedAt: { isNull: true }
                    }
                    orderBy: [{ jobName: ASC }]
                    page: { limit: 1, offset: 0 }
                ) {
                    edges {
                        node {
                            id
                            jobName
                            amount
                            closed
                            startedAt
                            customer {
                                customerName
                                creditLimit
                            }
                        }
                    }
                    pageInfo {
                        totalCount
                        hasNextPage
                        hasPreviousPage
                    }
                }
            }
            "#,
        )
        .await;
    assert!(
        list_response.errors.is_empty(),
        "{:?}",
        list_response.errors
    );
    let list_json = list_response.data.into_json()?;
    assert_eq!(list_json["mssqlJobs"]["pageInfo"]["totalCount"], 2);
    assert_eq!(list_json["mssqlJobs"]["pageInfo"]["hasNextPage"], true);
    assert_eq!(list_json["mssqlJobs"]["pageInfo"]["hasPreviousPage"], false);
    assert_eq!(
        list_json["mssqlJobs"]["edges"][0]["node"]["jobName"],
        "Pump audit"
    );
    assert_eq!(
        list_json["mssqlJobs"]["edges"][0]["node"]["customer"]["customerName"],
        "Acme"
    );
    assert_eq!(
        list_json["mssqlJobs"]["edges"][0]["node"]["amount"],
        "-0.0001"
    );
    assert_eq!(
        list_json["mssqlJobs"]["edges"][0]["node"]["customer"]["creditLimit"],
        "922337203685477.5807"
    );

    let single_response = schema
        .execute(
            r#"
            query {
                mssqlJob(id: 3) {
                    jobName
                    amount
                    closed
                    completedAt
                    customer {
                        customerName
                        creditLimit
                    }
                }
            }
            "#,
        )
        .await;
    assert!(
        single_response.errors.is_empty(),
        "{:?}",
        single_response.errors
    );
    let single_json = single_response.data.into_json()?;
    assert_eq!(single_json["mssqlJob"]["jobName"], "Valve replacement");
    assert_eq!(single_json["mssqlJob"]["closed"], true);
    assert!(single_json["mssqlJob"]["amount"].is_null());
    assert_eq!(
        single_json["mssqlJob"]["customer"]["creditLimit"],
        "-922337203685477.5808"
    );
    assert_eq!(
        single_json["mssqlJob"]["customer"]["customerName"],
        "Globex"
    );

    let positive = schema.execute("{ mssqlJob(id: 1) { amount } }").await;
    assert!(positive.errors.is_empty(), "{:?}", positive.errors);
    assert_eq!(positive.data.into_json()?["mssqlJob"]["amount"], "0.0001");

    Ok(())
}
