#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "sqlx_boundary_items",
    plural = "SqlxBoundaryItems",
    default_sort = "code ASC",
    upsert = "code"
)]
struct SqlxBoundaryItem {
    #[primary_key]
    pub id: graphql_orm::uuid::Uuid,

    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    pub code: String,

    #[filterable(type = "string")]
    pub name: String,
}

fn input(code: &str, name: &str) -> CreateSqlxBoundaryItemInput {
    CreateSqlxBoundaryItemInput {
        code: code.to_string(),
        name: name.to_string(),
    }
}

#[tokio::test]
async fn app_boundary_can_connect_migrate_and_write_without_sqlx_names() -> graphql_orm::Result<()>
{
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await?
        .with_schema_policy(SchemaPolicy::Managed);

    let entities = [SqlxBoundaryItem::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities("sqlx-boundary-init", "init", &entities)
        .await?;
    database
        .schema()
        .apply_migration(
            &plan,
            ApplyOptions {
                additive_only: true,
                ..Default::default()
            },
        )
        .await?;

    let inserted = SqlxBoundaryItem::insert_many(
        &database,
        [input("alpha", "Alpha One"), input("beta", "Beta One")],
    )
    .await?;
    assert_eq!(inserted.len(), 2);
    assert_eq!(SqlxBoundaryItem::count_all(&database).await?, 2);

    let alpha = SqlxBoundaryItem::find_many(
        &database,
        SqlxBoundaryItemWhereInput {
            code: Some(StringFilter {
                eq: Some("alpha".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(alpha.len(), 1);
    assert_eq!(alpha[0].name, "Alpha One");

    let outcomes = SqlxBoundaryItem::upsert_many(
        &database,
        [input("alpha", "Alpha Updated"), input("gamma", "Gamma One")],
    )
    .await?;
    assert_eq!(outcomes.len(), 2);
    assert_eq!(SqlxBoundaryItem::count_all(&database).await?, 3);
    assert_eq!(
        SqlxBoundaryItem::count(
            &database,
            SqlxBoundaryItemWhereInput {
                name: Some(StringFilter {
                    contains: Some("One".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await?,
        2
    );

    let replaced = SqlxBoundaryItem::replace_all(&database, [input("delta", "Delta One")]).await?;
    assert_eq!(replaced.len(), 1);
    assert_eq!(SqlxBoundaryItem::count_all(&database).await?, 1);

    let deleted = SqlxBoundaryItem::delete_all(&database).await?;
    assert_eq!(deleted, 1);
    assert_eq!(SqlxBoundaryItem::count_all(&database).await?, 0);

    Ok(())
}

#[tokio::test]
async fn managed_table_planning_can_ignore_unmanaged_live_tables() -> graphql_orm::Result<()> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let mut current = SchemaModel::from_entities(&[SqlxBoundaryItem::metadata()]);
    let mut external_table = current.tables[0].clone();
    external_table.table_name = "external_table".to_string();
    external_table.entity_name = "ExternalTable".to_string();
    external_table.indexes.clear();
    external_table.composite_unique_indexes.clear();
    external_table.foreign_keys.clear();
    external_table.search_indexes.clear();
    current.tables.push(external_table);
    let target = SchemaModel::from_entities(&[SqlxBoundaryItem::metadata()]);

    let strict = database
        .schema()
        .plan_migration("strict", "strict", &current, &target)?;
    assert!(strict.steps.iter().any(|step| {
        matches!(
            step.step,
            MigrationStep::DropTable { ref table_name } if table_name == "external_table"
        )
    }));
    let strict_apply_error = database
        .schema()
        .apply_migration(
            &strict,
            ApplyOptions {
                additive_only: true,
                ..Default::default()
            },
        )
        .await
        .expect_err("additive_only should reject non-additive drift plans");
    assert!(strict_apply_error.to_string().contains("non-additive"));

    let managed_only = database.schema().plan_migration_with_options(
        "managed-only",
        "managed only",
        &current,
        &target,
        PlanOptions::managed_tables_only(),
    )?;
    assert!(!managed_only.steps.iter().any(|step| {
        matches!(
            step.step,
            MigrationStep::DropTable { ref table_name } if table_name == "external_table"
        )
    }));

    Ok(())
}

#[tokio::test]
async fn public_transaction_boundary_needs_no_sqlx_names() -> graphql_orm::Result<()> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "transaction-boundary-init",
            "init",
            &[SqlxBoundaryItem::metadata()],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    let inserted = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let item = tx
                    .insert::<SqlxBoundaryItem>(input("atomic", "Atomic"))
                    .await?;
                let visible = tx.find_by_id::<SqlxBoundaryItem>(&item.id).await?;
                assert!(visible.is_some());
                Ok(item)
            })
        })
        .await
        .expect("transaction commits");
    assert_eq!(inserted.code, "atomic");

    let rejected = database
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert::<SqlxBoundaryItem>(input("rolled-back", "Nope"))
                    .await?;
                Err::<(), _>(OrmPublicError::with_message(
                    OrmErrorCode::Conflict,
                    "state changed",
                ))
            })
        })
        .await
        .expect_err("callback error rolls back");
    assert_eq!(rejected.public_error().code, OrmErrorCode::Conflict);
    assert_eq!(SqlxBoundaryItem::count_all(&database).await?, 1);
    Ok(())
}
