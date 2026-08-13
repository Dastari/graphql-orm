#![cfg(feature = "mssql")]

use graphql_orm::prelude::*;
use std::process::Command;

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
}

struct OwnedSqlServer {
    name: String,
    owner_token: String,
    connection_string: String,
}

impl Drop for OwnedSqlServer {
    fn drop(&mut self) {
        let identity = Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{ index .Config.Labels \"graphql-orm.test-owner\" }}",
                &self.name,
            ])
            .output();
        if identity
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout).trim() == self.owner_token
            })
        {
            let _ = Command::new("docker")
                .args(["rm", "--force", "--volumes", &self.name])
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
                "mcr.microsoft.com/mssql/server:2022-latest",
            ])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "failed to start owned SQL Server: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let mut owned = Self {
            name,
            owner_token: token,
            connection_string: String::new(),
        };
        let port = Command::new("docker")
            .args(["port", &owned.name, "1433/tcp"])
            .output()?;
        let published = String::from_utf8(port.stdout)?;
        let port = published
            .lines()
            .find_map(|line| line.strip_prefix("127.0.0.1:"))
            .ok_or("owned SQL Server was not loopback-published")?;
        owned.connection_string = format!(
            "server=tcp:127.0.0.1,{port};database=tempdb;user id=sa;password={password};TrustServerCertificate=true"
        );
        for _ in 0..180 {
            if let Ok(pool) =
                graphql_orm::db::mssql::MssqlPool::connect_ado(&owned.connection_string).await
            {
                if pool.fetch_rows("SELECT 1 AS ready", &[]).await.is_ok() {
                    return Ok(owned);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err("owned SQL Server did not become ready".into())
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

/// Owns a loopback-only disposable SQL Server container and removes it after
/// testing. This never accepts an ambient application database URL.
#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn external_writable_mssql_dml_and_transactions_are_native_and_atomic()
-> Result<(), Box<dyn std::error::Error>> {
    let server = OwnedSqlServer::start().await?;
    execute_batch(
        &server.connection_string,
        r#"
        CREATE TABLE dbo.GraphqlOrmWritableRecords (
            id NVARCHAR(64) NOT NULL PRIMARY KEY,
            external_key NVARCHAR(64) NOT NULL UNIQUE,
            value BIGINT NOT NULL,
            active BIT NOT NULL,
            payload VARBINARY(MAX) NOT NULL,
            amount DECIMAL(12,2) NOT NULL
        );
        "#,
    )
    .await?;

    let database =
        Database::<MssqlBackend>::connect_ado_external_writable(&server.connection_string).await?;
    let first = WritableRecord::insert(
        &database,
        CreateWritableRecordInput {
            external_key: "first".to_string(),
            value: 1,
            active: true,
            payload: vec![1, 2, 3],
            amount: rust_decimal::Decimal::new(1234, 2),
        },
    )
    .await?;
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
            external_key: "first".to_string(),
            value: 3,
            active: false,
            payload: vec![4, 5],
            amount: rust_decimal::Decimal::new(500, 2),
        },
    )
    .await?;
    assert_eq!(upsert.action, ChangeAction::Updated);
    assert_eq!(upsert.entity.value, 3);

    let rollback = database
        .transaction(TransactionMode::Default, |context| {
            Box::pin(async move {
                context
                    .insert::<WritableRecord>(CreateWritableRecordInput {
                        external_key: "rolled-back".to_string(),
                        value: 4,
                        active: true,
                        payload: Vec::new(),
                        amount: rust_decimal::Decimal::ZERO,
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

    assert!(WritableRecord::delete_by_id(&database, &first.id).await?);
    Ok(())
}
