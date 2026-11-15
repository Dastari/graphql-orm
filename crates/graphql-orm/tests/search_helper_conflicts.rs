//! Search helper table names that two base tables would share, or that the
//! database already uses, are refused before a migration changes anything
//! (#528).
//!
//! `search_table_name` is not injective once names are sanitized: `alias-notes`
//! and `alias_notes` map to one helper table. SQLite ignores ASCII case in
//! table names, and PostgreSQL folds the unquoted helper name to lower case,
//! so a table that differs from the helper name only in case is the same
//! table. PostgreSQL cases own a verified disposable server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use graphql_orm::prelude::*;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "alias-notes", plural = "AliasDashNotes")]
#[graphql_orm(search(index = true))]
struct AliasDashNote {
    #[primary_key]
    #[filterable(type = "string")]
    pub id: String,

    #[graphql_orm(searchable(weight = "A"))]
    #[filterable(type = "string")]
    #[sortable]
    pub title: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "alias_notes", plural = "AliasNotes")]
#[graphql_orm(search(index = true))]
struct AliasNote {
    #[primary_key]
    #[filterable(type = "string")]
    pub id: String,

    #[graphql_orm(searchable(weight = "A"))]
    #[filterable(type = "string")]
    #[sortable]
    pub title: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "MixedNotes", plural = "MixedNotes")]
#[graphql_orm(search(index = true))]
struct MixedNote {
    #[primary_key]
    #[filterable(type = "string")]
    pub id: String,

    #[graphql_orm(searchable(weight = "A"))]
    #[filterable(type = "string")]
    #[sortable]
    pub title: String,
}

/// The helper table a search index of `base` needs on this backend.
fn helper(base: &str) -> String {
    #[cfg(feature = "postgres")]
    return search_table_name(base);
    #[cfg(feature = "sqlite")]
    return sqlite_fts_table_name(base);
}

struct Fixture {
    db: Database<Backend>,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}

impl Fixture {
    async fn new(label: &str) -> Self {
        #[cfg(feature = "sqlite")]
        {
            let _ = label;
            Self {
                db: Database::<Backend>::connect_sqlite_with_options(
                    "sqlite::memory:",
                    graphql_orm::db::ConnectionOptions::default().max_connections(1),
                )
                .await
                .unwrap(),
            }
        }
        #[cfg(feature = "postgres")]
        {
            let owned = owned_postgres::OwnedPostgres::start(label).unwrap();
            let db = Database::<Backend>::connect_postgres(&owned.url)
                .await
                .unwrap();
            Self { db, owned }
        }
    }

    async fn run(&self, sql: &str) {
        Backend::fetch_rows(self.db.pool(), sql, &[]).await.unwrap();
    }

    /// Whether a table of exactly this stored name exists.
    async fn stored(&self, name: &str) -> bool {
        #[cfg(feature = "postgres")]
        let sql = "SELECT 1 AS present FROM pg_catalog.pg_class WHERE relname = $1";
        #[cfg(feature = "sqlite")]
        let sql = "SELECT 1 AS present FROM sqlite_master WHERE name = ?";
        !Backend::fetch_rows(self.db.pool(), sql, &[SqlValue::String(name.into())])
            .await
            .unwrap()
            .is_empty()
    }

    async fn finish(self) {
        self.db.pool().close().await;
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup().unwrap();
        }
    }
}

fn conflict(error: graphql_orm::Error) -> SearchHelperTableConflict {
    SearchHelperTableConflict::from_error(&error)
        .unwrap_or_else(|| panic!("expected a search helper table conflict, got {error:?}"))
        .clone()
}

/// Two entities of one target whose base tables sanitize to the same name
/// are refused when the target is planned, and nothing is created.
#[tokio::test]
async fn entities_whose_bases_sanitize_to_one_helper_name_are_refused() {
    let f = Fixture::new("search-helper-alias").await;
    assert_eq!(helper("alias-notes"), helper("alias_notes"));
    let both = [AliasDashNote::metadata(), AliasNote::metadata()];
    let refused =
        f.db.schema()
            .plan_migration_to_entities("init", "aliases", &both)
            .await
            .unwrap_err();
    assert_eq!(
        conflict(refused),
        SearchHelperTableConflict {
            helper_table: helper("alias_notes"),
            table: "alias_notes".into(),
            other_table: Some("alias-notes".into()),
        }
    );
    for absent in ["alias-notes", "alias_notes", &helper("alias_notes")] {
        assert!(!f.stored(absent).await, "{absent}");
    }

    // One of them alone is installed; the other is then refused whether it
    // is planned alone or with the first, because the helper table belongs
    // to the first base table.
    let first = [AliasDashNote::metadata()];
    let plan =
        f.db.schema()
            .plan_migration_to_entities("init", "aliases", &first)
            .await
            .unwrap();
    f.db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    assert!(f.stored(&helper("alias-notes")).await);
    let together =
        f.db.schema()
            .plan_migration_to_entities("second", "aliases", &both)
            .await
            .unwrap_err();
    assert_eq!(
        conflict(together).other_table.as_deref(),
        Some("alias-notes")
    );
    let alone =
        f.db.schema()
            .plan_migration_to_entities("second", "aliases", &[AliasNote::metadata()])
            .await
            .unwrap_err();
    assert_eq!(
        conflict(alone),
        SearchHelperTableConflict {
            helper_table: helper("alias_notes"),
            table: "alias_notes".into(),
            other_table: None,
        }
    );
    assert!(!f.stored("alias_notes").await);
    // The installed entity still plans clean.
    let again =
        f.db.schema()
            .plan_migration_to_entities("again", "aliases", &first)
            .await
            .unwrap();
    assert!(again.steps.is_empty(), "{:?}", again.steps);
    f.finish().await;
}

/// A table whose name differs from the helper name only in case is the same
/// table to the database: SQLite ignores ASCII case, and PostgreSQL folds the
/// unquoted helper name to lower case. It is refused at plan time and again
/// at apply time, before any statement runs.
#[tokio::test]
async fn a_case_variant_of_the_helper_name_is_refused_before_any_ddl() {
    let f = Fixture::new("search-helper-case").await;
    let entities = [MixedNote::metadata()];
    let expected = helper("MixedNotes");
    // SQLite: the upper-case form of the helper name. PostgreSQL: the
    // lower-case form, which is what the unquoted mixed-case name becomes.
    #[cfg(feature = "sqlite")]
    let variant = expected.to_ascii_uppercase();
    #[cfg(feature = "postgres")]
    let variant = expected.to_ascii_lowercase();
    assert_ne!(variant, expected);
    let plan = || async {
        f.db.schema()
            .plan_migration_to_entities("init", "mixed", &entities)
            .await
    };
    // Planned while the name is free; taken before it is applied.
    let planned = plan().await.unwrap();
    f.run(&format!("CREATE TABLE \"{variant}\" (kept BIGINT)"))
        .await;
    let wanted = SearchHelperTableConflict {
        helper_table: expected.clone(),
        table: "MixedNotes".into(),
        other_table: None,
    };
    let at_apply =
        f.db.schema()
            .apply_migration(&planned, ApplyOptions::default())
            .await
            .unwrap_err();
    assert_eq!(conflict(at_apply), wanted);
    assert_eq!(conflict(plan().await.unwrap_err()), wanted);
    // Nothing ran: the base table does not exist and the squatter is intact.
    assert!(!f.stored("MixedNotes").await);
    assert!(f.stored(&variant).await);
    // Once the name is free the same plan applies and plans clean again.
    // SQLite only: on PostgreSQL a mixed-case base table cannot be applied at
    // all, for a reason that has nothing to do with search (#716).
    f.run(&format!("DROP TABLE \"{variant}\"")).await;
    #[cfg(feature = "sqlite")]
    {
        f.db.schema()
            .apply_migration(&planned, ApplyOptions::default())
            .await
            .unwrap();
        assert!(f.stored("MixedNotes").await);
        assert!(plan().await.unwrap().steps.is_empty());
    }
    #[cfg(feature = "postgres")]
    assert!(plan().await.is_ok());
    f.finish().await;
}
