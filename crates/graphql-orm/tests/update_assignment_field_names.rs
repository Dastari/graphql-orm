// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! An entity may name updatable fields `db` and `ctx`. The generated update
//! paths share one assignment helper whose purity check looks for free-standing
//! `db` and `ctx` identifiers; `input.db` and `input.ctx` must not trip it.

use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(table = "named_connections", plural = "NamedConnections")]
struct NamedConnection {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    db: String,
    ctx: Option<String>,
    #[graphql_orm(write_policy = "connection.note.write")]
    note: String,
}

async fn setup() -> graphql_orm::Result<Database<SqliteBackend>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "named-connections-init",
            "fields named db and ctx",
            &[NamedConnection::metadata()],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Ok(database)
}

#[tokio::test]
async fn fields_named_db_and_ctx_compile_and_update() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let created = NamedConnection::insert(
        &database,
        CreateNamedConnectionInput {
            db: "primary".to_string(),
            ctx: None,
            note: "first".to_string(),
        },
    )
    .await?;
    assert_eq!(created.db, "primary");
    assert_eq!(created.ctx, None);

    let updated = NamedConnection::update_by_id(
        &database,
        &created.id,
        UpdateNamedConnectionInput {
            db: Some("replica".to_string()),
            ctx: Some(Some("read-only".to_string())),
            note: None,
        },
    )
    .await?
    .expect("the row exists");
    assert_eq!(updated.db, "replica");
    assert_eq!(updated.ctx.as_deref(), Some("read-only"));
    assert_eq!(updated.note, "first");

    let cleared = NamedConnection::update_by_id(
        &database,
        &created.id,
        UpdateNamedConnectionInput {
            db: None,
            ctx: Some(None),
            note: None,
        },
    )
    .await?
    .expect("the row exists");
    assert_eq!(cleared.db, "replica");
    assert_eq!(cleared.ctx, None);

    let stored = NamedConnection::find_by_id(&database, &created.id)
        .await?
        .expect("the row exists");
    assert_eq!(stored, cleared);
    Ok(())
}
