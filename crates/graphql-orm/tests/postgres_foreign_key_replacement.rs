#![cfg(all(feature = "postgres", not(feature = "sqlite")))]
//! A changed delete policy replaces a foreign key under the same generated constraint name;
//! the planned statements must apply on PostgreSQL in every direction.
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::graphql::orm::*;

fn column(name: &str, primary: bool) -> ColumnModel {
    ColumnModel {
        name: name.into(),
        sql_type: "TEXT".into(),
        spatial: None,
        nullable: !primary,
        is_primary_key: primary,
        is_unique: false,
        default: None,
    }
}
fn table(name: &str, columns: Vec<ColumnModel>, foreign_keys: Vec<ForeignKeyModel>) -> TableModel {
    TableModel {
        entity_name: name.into(),
        table_name: name.into(),
        primary_key: "id".into(),
        primary_keys: vec!["id".into()],
        default_sort: "id ASC".into(),
        columns,
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys,
        search_indexes: vec![],
        append_only: false,
        retention_purge: false,
        check_constraints: vec![],
    }
}
fn model(policy: Option<DeletePolicy>) -> SchemaModel {
    SchemaModel {
        extensions: vec![],
        tables: vec![
            table("fk_replace_parents", vec![column("id", true)], vec![]),
            table(
                "fk_replace_children",
                vec![column("id", true), column("parent_id", false)],
                policy
                    .map(|policy| {
                        ForeignKeyModel::single(
                            "parent_id",
                            "fk_replace_parents",
                            "id",
                            false,
                            policy,
                        )
                    })
                    .into_iter()
                    .collect(),
            ),
        ],
    }
}

#[tokio::test]
async fn postgres_applies_every_delete_policy_replacement() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = owned_postgres::OwnedPostgres::pool("fk-replacement", 2).await?;
    let empty = SchemaModel {
        extensions: vec![],
        tables: vec![],
    };
    let mut current = model(Some(DeletePolicy::Restrict));
    for statement in build_migration_plan(DatabaseBackend::Postgres, &empty, &current).statements {
        sqlx::query(&statement).execute(&pool).await?;
    }
    for (policy, rule) in [
        (DeletePolicy::Cascade, "CASCADE"),
        (DeletePolicy::Restrict, "RESTRICT"),
        (DeletePolicy::SetNull, "SET NULL"),
        (DeletePolicy::Cascade, "CASCADE"),
        (DeletePolicy::SetNull, "SET NULL"),
        (DeletePolicy::Restrict, "RESTRICT"),
    ] {
        let target = model(Some(policy.clone()));
        let plan = build_migration_plan(DatabaseBackend::Postgres, &current, &target);
        let mut tx = pool.begin().await?;
        for statement in &plan.statements {
            sqlx::query(statement)
                .execute(&mut *tx)
                .await
                .map_err(|error| format!("{policy:?}: {statement}: {error}"))?;
        }
        tx.commit().await?;
        let rules: Vec<String> = sqlx::query_scalar(
            "SELECT r.delete_rule::text FROM information_schema.referential_constraints r
             JOIN information_schema.table_constraints t
               ON t.constraint_name = r.constraint_name AND t.constraint_schema = r.constraint_schema
             WHERE t.table_name = 'fk_replace_children'",
        )
        .fetch_all(&pool)
        .await?;
        assert_eq!(rules, [rule], "{policy:?}");
        current = target;
    }
    Ok(())
}
