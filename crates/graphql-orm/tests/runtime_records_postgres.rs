#![cfg(feature = "postgres")]
//! Owned disposable PostgreSQL conformance for runtime-row decoding.

use std::process::Command;

use graphql_orm::graphql::orm::{
    CollectionId, FieldId, PostgresBackend, RuntimeCollection, RuntimeDateTime, RuntimeField,
    RuntimeNullPlacement, RuntimeOrderDirection, RuntimeOrderInput, RuntimeOrderTerm,
    RuntimePageRequest, RuntimeProjection, RuntimeQueryLimits, RuntimeRecord,
    RuntimeRecordErrorCode, RuntimeRowDecoder, RuntimeScalarOperator, RuntimeSchema, RuntimeValue,
    RuntimeValueKind, ValidatedRuntimeSchema,
};
use graphql_orm::sqlx::Connection;

const _: () = assert!(<PostgresBackend as RuntimeRowDecoder>::RUNTIME_ROW_DECODING_SUPPORTED);

fn cid(value: &str) -> CollectionId {
    CollectionId::new(value).expect("test collection ID")
}

fn fid(value: &str) -> FieldId {
    FieldId::new(value).expect("test field ID")
}

fn field(id: &str, column: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        id: fid(id),
        api_name: id.to_string(),
        physical_column: column.to_string(),
        value_kind: kind,
        nullable,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
    }
}

fn schema() -> ValidatedRuntimeSchema {
    let id = field("customer_id", "id", RuntimeValueKind::Integer, false);
    RuntimeSchema {
        format_version: 1,
        collections: vec![RuntimeCollection {
            id: cid("customers"),
            api_type_name: "Customer".to_string(),
            api_plural_name: "Customers".to_string(),
            physical_table: "runtime_customers".to_string(),
            primary_key: vec![id.id.clone()],
            append_only: false,
            retention_purge: false,
            fields: vec![
                id.clone(),
                field("active", "active", RuntimeValueKind::Boolean, false),
                field("count", "count_value", RuntimeValueKind::Integer, false),
                field("score", "score", RuntimeValueKind::Float, false),
                field("name", "name", RuntimeValueKind::String, false),
                field("uid", "uid", RuntimeValueKind::Uuid, false),
                field("document", "document", RuntimeValueKind::Json, false),
                field("payload", "payload", RuntimeValueKind::Bytes, false),
                field(
                    "happened_at",
                    "happened_at",
                    RuntimeValueKind::DateTime,
                    false,
                ),
                field("note", "note", RuntimeValueKind::String, true),
            ],
            relations: Vec::new(),
            indexes: Vec::new(),
            composite_unique: Vec::new(),
            default_order: vec![RuntimeOrderTerm {
                field: id.id,
                direction: RuntimeOrderDirection::Asc,
            }],
        }],
    }
    .validate()
    .unwrap_or_else(|diagnostics| panic!("valid test schema: {diagnostics}"))
}

fn projection(schema: &ValidatedRuntimeSchema) -> RuntimeProjection {
    schema
        .resolve_projection_ids(
            &cid("customers"),
            &[
                fid("customer_id"),
                fid("active"),
                fid("count"),
                fid("score"),
                fid("name"),
                fid("uid"),
                fid("document"),
                fid("payload"),
                fid("happened_at"),
                fid("note"),
            ],
        )
        .expect("valid projection")
}

struct OwnedPostgres {
    name: String,
    url: String,
}

impl Drop for OwnedPostgres {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.name])
            .output();
    }
}

impl OwnedPostgres {
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("graphql-orm-runtime-record-{suffix}");
        let password = format!("runtime_{suffix}");
        let database = format!("runtime_{suffix}");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--name",
                &name,
                "--publish",
                "127.0.0.1::5432",
                "--env",
                "POSTGRES_USER=runtime_owner",
                "--env",
                &format!("POSTGRES_PASSWORD={password}"),
                "--env",
                &format!("POSTGRES_DB={database}"),
                "postgres:16-alpine",
            ])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "failed to start disposable PostgreSQL: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let mut owned = Self {
            name,
            url: String::new(),
        };
        for _ in 0..120 {
            let ready = Command::new("docker")
                .args(["exec", &owned.name, "pg_isready", "-U", "runtime_owner"])
                .output()?;
            if ready.status.success() {
                let port_output = Command::new("docker")
                    .args(["port", &owned.name, "5432/tcp"])
                    .output()?;
                let ports = String::from_utf8(port_output.stdout)?;
                let port = ports
                    .lines()
                    .find_map(|line| line.strip_prefix("127.0.0.1:"))
                    .ok_or("Docker did not publish PostgreSQL on loopback")?;
                owned.url =
                    format!("postgres://runtime_owner:{password}@127.0.0.1:{port}/{database}");
                // `pg_isready` runs inside the container. Give Docker's
                // loopback-published proxy a short settling window as well,
                // especially while the full PostgreSQL matrix is under load.
                std::thread::sleep(std::time::Duration::from_millis(500));
                return Ok(owned);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err("disposable PostgreSQL did not become ready".into())
    }

    async fn connect(&self) -> Result<graphql_orm::sqlx::PgConnection, Box<dyn std::error::Error>> {
        for _ in 0..40 {
            match graphql_orm::sqlx::PgConnection::connect(&self.url).await {
                Ok(connection) => return Ok(connection),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(250)),
            }
        }
        Err("disposable PostgreSQL refused connections after readiness".into())
    }
}

async fn decode_postgres(
    connection: &mut graphql_orm::sqlx::PgConnection,
    projection: &RuntimeProjection,
) -> Result<RuntimeRecord, Box<dyn std::error::Error>> {
    graphql_orm::sqlx::query(
        "CREATE TEMPORARY TABLE runtime_customers (
            id BIGINT NOT NULL,
            active BOOLEAN NOT NULL,
            count_value BIGINT NOT NULL,
            score DOUBLE PRECISION NOT NULL,
            name TEXT NOT NULL,
            uid UUID NOT NULL,
            document JSONB NOT NULL,
            payload BYTEA NOT NULL,
            happened_at TIMESTAMPTZ NOT NULL,
            note TEXT NULL
        )",
    )
    .execute(&mut *connection)
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO runtime_customers VALUES (
            9223372036854775807, TRUE, -9223372036854775808, 12.5, 'Māori 🦀',
            '67e55044-10b1-426f-9247-bb680e5fe0c8',
            '{\"a\":[1,true],\"z\":\"雪\"}'::jsonb,
            decode('0001feff', 'hex'),
            '2026-07-15T12:34:56.123456789+10:00'::timestamptz,
            NULL
        )",
    )
    .execute(&mut *connection)
    .await?;
    let row = graphql_orm::sqlx::query(
        "SELECT id, active, count_value, score, name, uid, document, payload, happened_at, note,
                'ignored' AS unexpected_extra
         FROM runtime_customers",
    )
    .fetch_one(&mut *connection)
    .await?;
    Ok(projection.decode_row::<PostgresBackend>(&row)?)
}

#[tokio::test]
#[ignore = "creates and owns a disposable Docker PostgreSQL container"]
async fn disposable_postgres_runtime_records_match_sqlite_logically_and_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = OwnedPostgres::start()?;
    let mut connection = owned.connect().await?;
    let schema = schema();
    let projection = projection(&schema);
    let postgres_record = decode_postgres(&mut connection, &projection).await?;
    assert_eq!(postgres_record.collection_id(), &cid("customers"));

    let missing = graphql_orm::sqlx::query("SELECT 1 AS another_column")
        .fetch_one(&mut connection)
        .await?;
    assert_eq!(
        projection
            .decode_row::<PostgresBackend>(&missing)
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::MissingColumn
    );
    let wrong_type = graphql_orm::sqlx::query(
        "SELECT id::text AS id, active, count_value, score, name, uid, document, payload,
                happened_at, note FROM runtime_customers",
    )
    .fetch_one(&mut connection)
    .await?;
    assert_eq!(
        projection
            .decode_row::<PostgresBackend>(&wrong_type)
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::BackendTypeMismatch
    );

    #[cfg(feature = "sqlite")]
    {
        use graphql_orm::graphql::orm::SqliteBackend;
        let mut sqlite = graphql_orm::sqlx::SqliteConnection::connect("sqlite::memory:").await?;
        let row = graphql_orm::sqlx::query(
            "SELECT 9223372036854775807 AS id, 1 AS active,
                    -9223372036854775808 AS count_value, 12.5 AS score, 'Māori 🦀' AS name,
                    '67e55044-10b1-426f-9247-bb680e5fe0c8' AS uid,
                    '{\"a\":[1,true],\"z\":\"雪\"}' AS document, x'0001feff' AS payload,
                    '2026-07-15T12:34:56.123456789+10:00' AS happened_at, NULL AS note",
        )
        .fetch_one(&mut sqlite)
        .await?;
        let sqlite_record = projection.decode_row::<SqliteBackend>(&row)?;
        assert_eq!(postgres_record, sqlite_record);
    }

    drop(connection);
    drop(owned);
    Ok(())
}

#[tokio::test]
#[ignore = "creates and owns a disposable Docker PostgreSQL container"]
async fn disposable_postgres_runtime_query_filters_pages_and_counts()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = OwnedPostgres::start()?;
    let database = graphql_orm::db::Database::connect_postgres(&owned.url).await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE runtime_customers (
            id BIGINT PRIMARY KEY, active BOOLEAN NOT NULL, count_value BIGINT NOT NULL,
            score DOUBLE PRECISION NOT NULL, name TEXT NOT NULL, uid UUID NOT NULL,
            document JSONB NOT NULL, payload BYTEA NOT NULL,
            happened_at TIMESTAMPTZ NOT NULL, note TEXT NULL
        )",
    )
    .execute(database.pool())
    .await?;
    for id in 1_i64..=4 {
        graphql_orm::sqlx::query(
            "INSERT INTO runtime_customers VALUES (
                $1, TRUE, $1, $1::double precision, $2,
                '67e55044-10b1-426f-9247-bb680e5fe0c8', '{}'::jsonb,
                decode('ff', 'hex'), $3::timestamptz, NULL
            )",
        )
        .bind(id)
        .bind(format!("customer-{id}"))
        .bind(format!("2026-07-1{id}T00:00:00Z"))
        .execute(database.pool())
        .await?;
    }

    let schema = schema();
    let collection = schema.resolve_collection(&cid("customers"))?;
    let id = schema.resolve_field(&collection, &fid("customer_id"))?;
    let name = schema.resolve_field(&collection, &fid("name"))?;
    let happened = schema.resolve_field(&collection, &fid("happened_at"))?;
    let projection = schema.resolve_projection(&collection, &[id.clone(), name.clone()])?;
    let limits = RuntimeQueryLimits::default();
    let filter = schema.runtime_compare(
        &collection,
        &happened,
        RuntimeScalarOperator::Gte,
        RuntimeValue::DateTime(RuntimeDateTime::parse("2026-07-12T00:00:00Z")?),
        limits,
    )?;
    let order = schema.runtime_order(
        &collection,
        Some(vec![RuntimeOrderInput {
            field: name,
            direction: RuntimeOrderDirection::Desc,
            nulls: RuntimeNullPlacement::Last,
        }]),
        limits,
    )?;
    let request = schema.runtime_read_request(
        &collection,
        &projection,
        Some(filter),
        order,
        RuntimePageRequest::first(2, None),
        true,
        limits,
    )?;
    let result = database.execute_runtime_read(&request, None).await?;
    assert_eq!(result.edges.len(), 2);
    assert!(result.page_info.has_next_page);
    assert_eq!(result.total_count, Some(3));
    assert_eq!(result.edges[0].node.integer(&id)?, 4);

    drop(database);
    drop(owned);
    Ok(())
}
