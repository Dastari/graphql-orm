#![cfg(feature = "sqlite")]
//! A planned unique index over rows that already repeat its key is refused
//! with a typed error before any migration statement runs.

use graphql_orm::graphql::orm::{ApplyOptions, Entity, MigrationStep, UniqueIndexDuplicates};
use graphql_orm::prelude::{Database, SqliteBackend};

mod before {
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(backend = "sqlite", table = "uniq_notes", plural = "UniqNotesBefore")]
    pub struct NoteBefore {
        #[primary_key]
        #[sortable]
        pub id: String,
        pub owner: String,
        pub slug: Option<String>,
    }
}
mod after {
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "uniq_notes",
        plural = "UniqNotesAfter",
        unique_index = "owner, slug"
    )]
    pub struct NoteAfter {
        #[primary_key]
        #[sortable]
        pub id: String,
        pub owner: String,
        pub slug: Option<String>,
    }
}
mod added_column {
    use graphql_orm::prelude::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "uniq_notes",
        plural = "UniqNotesAdded",
        unique_index = "code"
    )]
    pub struct NoteAdded {
        #[primary_key]
        #[sortable]
        pub id: String,
        pub owner: String,
        pub slug: Option<String>,
        pub code: Option<String>,
    }
}

async fn store(rows: &[(&str, &str, Option<&str>)]) -> Database<SqliteBackend> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open");
    let plan = database
        .schema()
        .plan_migration_to_entities("v1", "notes", &[before::NoteBefore::metadata()])
        .await
        .expect("v1 should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("v1 should apply");
    for (id, owner, slug) in rows {
        sqlx::query("INSERT INTO uniq_notes (id, owner, slug) VALUES (?, ?, ?)")
            .bind(id)
            .bind(owner)
            .bind(slug)
            .execute(database.pool())
            .await
            .expect("row should insert");
    }
    database
}

async fn unique_index_count(database: &Database<SqliteBackend>) -> i64 {
    sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND tbl_name = 'uniq_notes' \
         AND sql LIKE 'CREATE UNIQUE INDEX%'",
    )
    .fetch_one(database.pool())
    .await
    .expect("index catalogue should read")
    .0
}

#[tokio::test]
async fn repeated_keys_refuse_the_plan_before_any_statement_and_name_only_the_index() {
    let database = store(&[
        ("1", "owner-a", Some("first")),
        ("2", "owner-a", Some("first")),
        ("3", "owner-b", Some("first")),
    ])
    .await;
    let plan = database
        .schema()
        .plan_migration_to_entities("v2", "unique slug", &[after::NoteAfter::metadata()])
        .await
        .expect("v2 should plan");
    assert!(plan.steps.iter().any(|step| matches!(
        &step.step,
        MigrationStep::CreateIndex { index, .. } if index.is_unique
    )));
    let mut mismatched = plan.clone();
    mismatched.backend = "postgres";
    let mismatch = database
        .schema()
        .preflight_unique_indexes(&mismatched)
        .await
        .expect_err("preflight must refuse a plan for another backend");
    assert!(mismatch.to_string().contains("preflight uses sqlite"));

    let preflight_error = database
        .schema()
        .preflight_unique_indexes(&plan)
        .await
        .expect_err("preflight must refuse repeated keys");
    assert!(UniqueIndexDuplicates::from_error(&preflight_error).is_some());

    let refused = database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect_err("repeated keys must refuse the plan");
    let duplicates = UniqueIndexDuplicates::from_error(&refused)
        .unwrap_or_else(|| panic!("refusal should be typed: {refused:?}"));
    assert_eq!(duplicates.table, "uniq_notes");
    assert_eq!(duplicates.columns, ["owner", "slug"]);
    let message = duplicates.to_string();
    assert!(message.contains("uniq_notes") && message.contains(&duplicates.index));
    // No row value is disclosed.
    assert!(!message.contains("owner-a") && !message.contains("first"));

    assert_eq!(unique_index_count(&database).await, 0);
    assert_eq!(
        database.schema().current_version().await.expect("version"),
        Some("v1".to_owned())
    );

    sqlx::query("DELETE FROM uniq_notes WHERE id = '2'")
        .execute(database.pool())
        .await
        .expect("duplicate should delete");
    database
        .schema()
        .preflight_unique_indexes(&plan)
        .await
        .expect("clean preflight should succeed");
    assert_eq!(unique_index_count(&database).await, 0);
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("a clean table should take the unique index");
    assert_eq!(unique_index_count(&database).await, 1);
}

#[tokio::test]
async fn null_keys_and_columns_the_plan_adds_are_not_reported() {
    // SQLite treats NULL keys as distinct, so repeated NULL slugs are valid.
    let database = store(&[
        ("1", "owner-a", None),
        ("2", "owner-a", None),
        ("3", "owner-a", Some("only")),
    ])
    .await;
    let plan = database
        .schema()
        .plan_migration_to_entities("v2", "unique slug", &[after::NoteAfter::metadata()])
        .await
        .expect("v2 should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("NULL keys do not repeat");
    assert_eq!(unique_index_count(&database).await, 1);

    // A unique index over a column the same plan adds cannot be read first.
    let database = store(&[("1", "owner-a", None), ("2", "owner-a", None)]).await;
    let plan = database
        .schema()
        .plan_migration_to_entities("v2", "unique code", &[added_column::NoteAdded::metadata()])
        .await
        .expect("added-column plan should build");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("an added nullable column has no repeated key");
    assert_eq!(unique_index_count(&database).await, 1);
}
