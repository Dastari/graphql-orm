#![cfg(feature = "mssql")]

use graphql_orm::prelude::*;
use std::process::Command;
use std::sync::Arc;

const SQL_SERVER_IMAGE: &str = "mcr.microsoft.com/mssql/server@sha256:ba4c8329f48fb8f02e1416be6a930ebfd71268caee78aa985f3af4315e457c89";

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.GraphqlOrmWritableRecords",
    plural = "WritableRecords",
    schema_policy = "external_writable",
    upsert = "external_key"
)]
struct WritableRecord {
    #[primary_key]
    id: String,
    #[unique]
    #[filterable(type = "string")]
    external_key: String,
    value: i64,
    active: bool,
    payload: Vec<u8>,
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    amount: rust_decimal::Decimal,
    #[graphql_orm(version, default = "0")]
    #[filterable(type = "number")]
    version: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.GraphqlOrmAggregateRows",
    plural = "AggregateRows",
    schema_policy = "external_writable",
    aggregate = true,
    auth = "none"
)]
struct AggregateRow {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    team: Option<String>,
    units: i64,
    hours: f64,
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    amount: rust_decimal::Decimal,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.GraphqlOrmCompositeRecords",
    plural = "CompositeRecords",
    schema_policy = "external_writable",
    repository_mutations = true,
    unique_composite = "tenant_id,record_key",
    upsert = "tenant_id,record_key"
)]
struct CompositeRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    tenant_id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    record_key: String,
    #[filterable(type = "string")]
    value: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.GraphqlOrmScalarRecords",
    plural = "ScalarRecords",
    schema_policy = "external_writable"
)]
struct ScalarRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    optional_text: Option<String>,
    document: serde_json::Value,
    business_date: String,
    business_time: String,
    occurred_at: String,
}

#[derive(Clone)]
struct AllowEntityPolicy;

impl EntityPolicy<MssqlBackend> for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<MssqlBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

struct OwnedSqlServer {
    name: String,
    container_id: String,
    owner_token: String,
    connection_string: String,
    cleaned: bool,
}

impl Drop for OwnedSqlServer {
    fn drop(&mut self) {
        if !self.cleaned && self.has_exact_owned_identity() {
            let _ = Command::new("docker")
                .args(["rm", "--force", "--volumes", &self.container_id])
                .output();
        }
    }
}

impl OwnedSqlServer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
        let name = format!("graphql-orm-mssql-writes-{token}");
        let password = format!("Gom_{token}A9!");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--name",
                &name,
                "--label",
                &format!("graphql-orm.test-owner={token}"),
                "--publish",
                "127.0.0.1::1433",
                "--env",
                "ACCEPT_EULA=Y",
                "--env",
                &format!("MSSQL_SA_PASSWORD={password}"),
                SQL_SERVER_IMAGE,
            ])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "failed to start owned SQL Server: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let container_id = String::from_utf8(output.stdout)?.trim().to_owned();
        if container_id.is_empty() {
            return Err("docker did not return the owned SQL Server container ID".into());
        }
        let mut owned = Self {
            name,
            container_id,
            owner_token: token,
            connection_string: String::new(),
            cleaned: false,
        };
        let port = Command::new("docker")
            .args(["port", &owned.name, "1433/tcp"])
            .output()?;
        let published = String::from_utf8(port.stdout)?;
        let port = published
            .lines()
            .find_map(|line| line.strip_prefix("127.0.0.1:"))
            .ok_or("owned SQL Server was not loopback-published")?;
        let master_connection = format!(
            "server=tcp:127.0.0.1,{port};database=master;user id=sa;password={password};TrustServerCertificate=true"
        );
        for _ in 0..180 {
            if let Ok(pool) =
                graphql_orm::db::mssql::MssqlPool::connect_ado(&master_connection).await
            {
                if pool.fetch_rows("SELECT 1 AS ready", &[]).await.is_ok() {
                    let database_name = format!("graphql_orm_{}", owned.owner_token);
                    execute_batch(
                        &master_connection,
                        &format!("CREATE DATABASE [{database_name}]"),
                    )
                    .await?;
                    owned.connection_string = format!(
                        "server=tcp:127.0.0.1,{port};database={database_name};user id=sa;password={password};TrustServerCertificate=true"
                    );
                    return Ok(owned);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err("owned SQL Server did not become ready".into())
    }

    fn has_exact_owned_identity(&self) -> bool {
        let identity = Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{.Id}} {{ index .Config.Labels \"graphql-orm.test-owner\" }}",
                &self.container_id,
            ])
            .output();
        identity
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout).trim()
                    == format!("{} {}", self.container_id, self.owner_token)
            })
    }

    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.has_exact_owned_identity() {
            return Err("refusing to clean up a SQL Server without exact owned identity".into());
        }
        let removed = Command::new("docker")
            .args(["rm", "--force", "--volumes", &self.container_id])
            .output()?;
        if !removed.status.success() {
            return Err(format!(
                "failed to remove owned SQL Server: {}",
                String::from_utf8_lossy(&removed.stderr)
            )
            .into());
        }
        let absent = Command::new("docker")
            .args(["inspect", &self.container_id])
            .output()?;
        if absent.status.success() {
            return Err("owned SQL Server remains after cleanup".into());
        }
        self.cleaned = true;
        Ok(())
    }
}

async fn execute_batch(connection_string: &str, sql: &str) -> graphql_orm::Result<()> {
    use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;
    let config = graphql_orm::tiberius::Config::from_ado_string(connection_string)
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let tcp = tokio::net::TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;
    let mut client = graphql_orm::tiberius::Client::connect(config, tcp.compat_write())
        .await
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    client
        .simple_query(sql)
        .await
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?
        .into_results()
        .await
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    Ok(())
}

fn writable_input(external_key: &str, value: i64) -> CreateWritableRecordInput {
    CreateWritableRecordInput {
        external_key: external_key.to_owned(),
        value,
        active: true,
        payload: vec![1, 2, 3],
        amount: rust_decimal::Decimal::new(1234, 2),
    }
}

fn writable_database(
    connection_string: &str,
    maximum_connections: usize,
) -> Result<Database<MssqlBackend>, Box<dyn std::error::Error>> {
    let config = graphql_orm::tiberius::Config::from_ado_string(connection_string)?;
    let pool = graphql_orm::db::mssql::MssqlPool::with_max_connections_external_writable(
        config,
        maximum_connections,
    );
    let mut database = Database::<MssqlBackend>::builder(pool)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    database.set_entity_policy(AllowEntityPolicy);
    Ok(database)
}

/// Owns a loopback-only disposable SQL Server container and removes it after
/// testing. This never accepts an ambient application database URL.
#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn external_writable_mssql_dml_and_transactions_are_native_and_atomic()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(
        &server.connection_string,
        r#"
        CREATE TABLE dbo.GraphqlOrmWritableRecords (
            id NVARCHAR(64) NOT NULL PRIMARY KEY,
            external_key NVARCHAR(64) NOT NULL UNIQUE,
            value BIGINT NOT NULL,
            active BIT NOT NULL,
            payload VARBINARY(MAX) NOT NULL,
            amount DECIMAL(12,2) NOT NULL,
            version BIGINT NOT NULL DEFAULT (0)
        );
        CREATE TABLE dbo.GraphqlOrmCompositeRecords (
            tenant_id NVARCHAR(64) NOT NULL,
            record_key NVARCHAR(64) NOT NULL,
            value NVARCHAR(128) NOT NULL,
            CONSTRAINT PK_GraphqlOrmCompositeRecords PRIMARY KEY (tenant_id, record_key)
        );
        CREATE TABLE dbo.GraphqlOrmAggregateRows (
            id NVARCHAR(64) NOT NULL PRIMARY KEY,
            team NVARCHAR(64) NULL,
            units BIGINT NOT NULL,
            hours FLOAT NOT NULL,
            amount DECIMAL(12,2) NOT NULL
        );
        CREATE TABLE dbo.GraphqlOrmScalarRecords (
            id UNIQUEIDENTIFIER NOT NULL PRIMARY KEY,
            optional_text NVARCHAR(128) NULL,
            document NVARCHAR(MAX) NOT NULL,
            business_date DATE NOT NULL,
            business_time TIME(3) NOT NULL,
            occurred_at DATETIME2(3) NOT NULL
        );
        "#,
    )
    .await?;

    let read_only = Database::<MssqlBackend>::connect_ado(&server.connection_string).await?;
    assert_eq!(
        read_only.pool().access_mode(),
        graphql_orm::db::mssql::MssqlAccessMode::ReadOnly
    );
    assert!(
        WritableRecord::insert(&read_only, writable_input("read-only-denied", 9))
            .await
            .is_err()
    );

    let database = writable_database(&server.connection_string, 8)?;
    let scalars = ScalarRecord::insert(
        &database,
        CreateScalarRecordInput {
            optional_text: None,
            document: serde_json::json!({"enabled": true, "count": 2}),
            business_date: "2026-08-13".to_owned(),
            business_time: "14:15:16.125".to_owned(),
            occurred_at: "2026-08-13T14:15:16.125".to_owned(),
        },
    )
    .await?;
    assert_ne!(scalars.id, graphql_orm::uuid::Uuid::nil());
    assert_eq!(scalars.optional_text, None);
    assert_eq!(scalars.document["count"], 2);
    assert_eq!(scalars.business_date, "2026-08-13");
    assert!(scalars.business_time.starts_with("14:15:16.125"));
    assert!(scalars.occurred_at.starts_with("2026-08-13 14:15:16.125"));
    let first = WritableRecord::insert(&database, writable_input("first", 1)).await?;
    assert!(!first.id.is_empty());
    assert_eq!(first.version, 0);
    assert_eq!(first.amount, rust_decimal::Decimal::new(1234, 2));

    let updated = WritableRecord::update_by_id(
        &database,
        &first.id,
        UpdateWritableRecordInput {
            value: Some(2),
            ..Default::default()
        },
    )
    .await?
    .expect("inserted row should update");
    assert_eq!(updated.value, 2);

    let upsert = WritableRecord::upsert(
        &database,
        CreateWritableRecordInput {
            active: false,
            payload: vec![4, 5],
            amount: rust_decimal::Decimal::new(500, 2),
            ..writable_input("first", 3)
        },
    )
    .await?;
    assert_eq!(upsert.action, ChangeAction::Updated);
    assert_eq!(upsert.entity.value, 3);

    let cas = WritableRecord::compare_and_swap(
        &database,
        &first.id,
        0,
        WritableRecordWhereInput {
            external_key: Some(StringFilter {
                eq: Some("first".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateWritableRecordInput {
            value: Some(4),
            ..Default::default()
        },
    )
    .await?;
    let ConditionalUpdateOutcome::Updated(cas) = cas else {
        panic!("matching MSSQL compare-and-swap should update");
    };
    assert_eq!(cas.version, 1);
    assert_eq!(cas.value, 4);
    assert!(matches!(
        WritableRecord::compare_and_swap(
            &database,
            &first.id,
            0,
            WritableRecordWhereInput::default(),
            UpdateWritableRecordInput {
                value: Some(5),
                ..Default::default()
            },
        )
        .await?,
        ConditionalUpdateOutcome::Conflict
    ));

    let bulk = WritableRecord::insert_many(
        &database,
        [
            writable_input("bounded-a", 10),
            writable_input("bounded-b", 20),
        ],
    )
    .await?;
    assert_eq!(bulk.len(), 2);
    let bounded_filter = || WritableRecordWhereInput {
        external_key: Some(StringFilter {
            contains: Some("bounded-".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        WritableRecord::update_where_bounded(
            &database,
            bounded_filter(),
            UpdateWritableRecordInput {
                value: Some(30),
                ..Default::default()
            },
            MutationLimit::new(1)?,
        )
        .await?,
        BoundedMutationOutcome::LimitExceeded { maximum: 1 }
    );
    assert_eq!(
        WritableRecord::update_where_bounded(
            &database,
            bounded_filter(),
            UpdateWritableRecordInput {
                value: Some(30),
                ..Default::default()
            },
            MutationLimit::new(2)?,
        )
        .await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );
    assert_eq!(
        WritableRecord::delete_where_bounded(&database, bounded_filter(), MutationLimit::new(2)?,)
            .await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );

    let bulk_upsert = WritableRecord::upsert_many(
        &database,
        [
            writable_input("bulk-upsert-a", 1),
            writable_input("bulk-upsert-b", 2),
        ],
    )
    .await?;
    assert!(
        bulk_upsert
            .iter()
            .all(|outcome| outcome.action == ChangeAction::Created)
    );
    let bulk_upsert = WritableRecord::upsert_many(
        &database,
        [
            writable_input("bulk-upsert-a", 11),
            writable_input("bulk-upsert-b", 12),
        ],
    )
    .await?;
    assert!(
        bulk_upsert
            .iter()
            .all(|outcome| outcome.action == ChangeAction::Updated)
    );

    let composite_input = |value: &str| CreateCompositeRecordInput {
        tenant_id: "tenant-a".to_owned(),
        record_key: "record-a".to_owned(),
        value: value.to_owned(),
    };
    assert!(matches!(
        CompositeRecord::insert_if_absent(&database, composite_input("one")).await?,
        InsertIfAbsentOutcome::Inserted(_)
    ));
    assert!(matches!(
        CompositeRecord::insert_if_absent(&database, composite_input("two")).await?,
        InsertIfAbsentOutcome::AlreadyPresent(_)
    ));
    let composite = CompositeRecord::upsert(&database, composite_input("updated")).await?;
    assert_eq!(composite.action, ChangeAction::Updated);
    let composite_key = CompositeRecordKey {
        tenant_id: "tenant-a".to_owned(),
        record_key: "record-a".to_owned(),
    };
    assert_eq!(
        CompositeRecord::find_by_key(&database, &composite_key)
            .await?
            .expect("composite row should remain")
            .value,
        "updated"
    );
    let composite = CompositeRecord::update_by_key(
        &database,
        &composite_key,
        UpdateCompositeRecordInput {
            value: Some("updated-by-key".to_owned()),
        },
    )
    .await?
    .expect("composite key update should find the row");
    assert_eq!(composite.value, "updated-by-key");
    CompositeRecord::insert(
        &database,
        CreateCompositeRecordInput {
            tenant_id: "tenant-a".to_owned(),
            record_key: "record-b".to_owned(),
            value: "second".to_owned(),
        },
    )
    .await?;
    let tenant_filter = || CompositeRecordWhereInput {
        tenant_id: Some(StringFilter {
            eq: Some("tenant-a".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        CompositeRecord::update_where_bounded(
            &database,
            tenant_filter(),
            UpdateCompositeRecordInput {
                value: Some("bounded-composite".to_owned()),
            },
            MutationLimit::new(1)?,
        )
        .await?,
        BoundedMutationOutcome::LimitExceeded { maximum: 1 }
    );
    assert_eq!(
        CompositeRecord::update_where_bounded(
            &database,
            tenant_filter(),
            UpdateCompositeRecordInput {
                value: Some("bounded-composite".to_owned()),
            },
            MutationLimit::new(2)?,
        )
        .await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );
    assert_eq!(
        CompositeRecord::delete_where_bounded(&database, tenant_filter(), MutationLimit::new(2)?,)
            .await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );
    assert!(
        CompositeRecord::find_by_key(&database, &composite_key)
            .await?
            .is_none()
    );

    let rollback = database
        .transaction(TransactionMode::Default, |context| {
            Box::pin(async move {
                context
                    .insert::<WritableRecord>(CreateWritableRecordInput {
                        payload: Vec::new(),
                        amount: rust_decimal::Decimal::ZERO,
                        ..writable_input("rolled-back", 4)
                    })
                    .await?;
                Err::<(), _>(graphql_orm::graphql::errors::OrmPublicError::new(
                    graphql_orm::graphql::errors::OrmErrorCode::Conflict,
                ))
            })
        })
        .await;
    assert!(rollback.is_err());
    assert!(
        WritableRecord::query(&database)
            .filter(WritableRecordWhereInput {
                external_key: Some(StringFilter {
                    eq: Some("rolled-back".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_optional_one()
            .await?
            .is_none()
    );

    let duplicate_failure = database
        .transaction(TransactionMode::Default, |context| {
            Box::pin(async move {
                context
                    .insert::<WritableRecord>(writable_input("duplicate", 1))
                    .await?;
                context
                    .insert::<WritableRecord>(writable_input("duplicate", 2))
                    .await?;
                Ok::<(), graphql_orm::graphql::errors::OrmPublicError>(())
            })
        })
        .await;
    assert!(duplicate_failure.is_err());
    assert!(
        WritableRecord::query(&database)
            .filter(WritableRecordWhereInput {
                external_key: Some(StringFilter {
                    eq: Some("duplicate".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_optional_one()
            .await?
            .is_none()
    );

    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let left_db = database.clone();
    let left_barrier = barrier.clone();
    let left = tokio::spawn(async move {
        left_barrier.wait().await;
        WritableRecord::upsert(&left_db, writable_input("concurrent", 41)).await
    });
    let right_db = database.clone();
    let right_barrier = barrier.clone();
    let right = tokio::spawn(async move {
        right_barrier.wait().await;
        WritableRecord::upsert(&right_db, writable_input("concurrent", 42)).await
    });
    barrier.wait().await;
    let left = left.await??;
    let right = right.await??;
    assert!(matches!(
        (left.action, right.action),
        (ChangeAction::Created, ChangeAction::Updated)
            | (ChangeAction::Updated, ChangeAction::Created)
    ));
    assert_eq!(
        WritableRecord::query(&database)
            .filter(WritableRecordWhereInput {
                external_key: Some(StringFilter {
                    eq: Some("concurrent".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_all()
            .await?
            .len(),
        1
    );

    execute_batch(
        &server.connection_string,
        r#"
        INSERT INTO dbo.GraphqlOrmAggregateRows (id, team, units, hours, amount) VALUES
            ('a', 'alpha', 2, 0.5, 1.25),
            ('b', 'alpha', 3, 1.5, 2.50),
            ('c', NULL, 4, 2.0, 3.75);
        "#,
    )
    .await?;
    let aggregates = AggregateRow::aggregate(&database)
        .group_by(AggregateRowAggregateField::Team)?
        .count_rows()?
        .min(AggregateRowAggregateField::Units)?
        .max(AggregateRowAggregateField::Units)?
        .sum(AggregateRowAggregateField::Units)?
        .sum(AggregateRowAggregateField::Hours)?
        .sum(AggregateRowAggregateField::Amount)?
        .group_limit(10)?
        .fetch()
        .await?;
    assert_eq!(aggregates.len(), 2);
    assert_eq!(aggregates[0].groups[0].value, AggregateValue::Null);
    assert_eq!(aggregates[0].metrics[0].value, AggregateValue::Count(1));
    assert_eq!(aggregates[0].metrics[1].value, AggregateValue::Integral(4));
    assert_eq!(aggregates[0].metrics[2].value, AggregateValue::Integral(4));
    assert_eq!(aggregates[0].metrics[3].value, AggregateValue::Integral(4));
    assert_eq!(
        aggregates[0].metrics[4].value,
        AggregateValue::Floating(2.0)
    );
    assert_eq!(
        aggregates[0].metrics[5].value,
        AggregateValue::Decimal(rust_decimal::Decimal::new(375, 2))
    );
    assert_eq!(aggregates[1].metrics[0].value, AggregateValue::Count(2));
    assert_eq!(aggregates[1].metrics[1].value, AggregateValue::Integral(2));
    assert_eq!(aggregates[1].metrics[2].value, AggregateValue::Integral(3));
    assert_eq!(aggregates[1].metrics[3].value, AggregateValue::Integral(5));
    assert_eq!(
        aggregates[1].metrics[4].value,
        AggregateValue::Floating(2.0)
    );
    assert_eq!(
        aggregates[1].metrics[5].value,
        AggregateValue::Decimal(rust_decimal::Decimal::new(375, 2))
    );
    let first_group = AggregateRow::aggregate(&database)
        .group_by(AggregateRowAggregateField::Team)?
        .count_rows()?
        .group_limit(1)?
        .fetch()
        .await?;
    assert_eq!(first_group.len(), 1);
    assert_eq!(first_group[0].groups[0].value, AggregateValue::Null);

    execute_batch(
        &server.connection_string,
        r#"
        WITH source_rows AS (
            SELECT 1 AS row_no
            UNION ALL
            SELECT row_no + 1 FROM source_rows WHERE row_no < 125
        )
        INSERT INTO dbo.GraphqlOrmAggregateRows (id, team, units, hours, amount)
        SELECT CONCAT('extra-', row_no), 'alpha', 0, 0.0, 0.00
        FROM source_rows
        OPTION (MAXRECURSION 125);
        "#,
    )
    .await?;
    let beyond_page_ceiling = AggregateRow::aggregate(&database)
        .group_by(AggregateRowAggregateField::Team)?
        .count_rows()?
        .sum(AggregateRowAggregateField::Units)?
        .group_limit(10)?
        .fetch()
        .await?;
    assert_eq!(
        beyond_page_ceiling[1].metrics[0].value,
        AggregateValue::Count(127)
    );
    assert_eq!(
        beyond_page_ceiling[1].metrics[1].value,
        AggregateValue::Integral(5)
    );

    let poison_database = writable_database(&server.connection_string, 1)?;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let poison_task_database = poison_database.clone();
    let poisoned = tokio::spawn(async move {
        poison_task_database
            .transaction(TransactionMode::Default, |context| {
                Box::pin(async move {
                    context
                        .insert::<WritableRecord>(writable_input("cancelled", 99))
                        .await?;
                    let _ = started_tx.send(());
                    context.execute("WAITFOR DELAY '00:00:30'", &[]).await?;
                    Ok::<(), graphql_orm::graphql::errors::OrmPublicError>(())
                })
            })
            .await
    });
    started_rx.await?;
    poisoned.abort();
    let _ = poisoned.await;
    let cancelled_row = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        WritableRecord::query(&poison_database)
            .filter(WritableRecordWhereInput {
                external_key: Some(StringFilter {
                    eq: Some("cancelled".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_optional_one(),
    )
    .await??;
    assert!(cancelled_row.is_none());

    assert!(WritableRecord::delete_by_id(&database, &first.id).await?);
    server.cleanup()?;
    Ok(())
}
