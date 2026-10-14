use graphql_orm::graphql::auth::{AuthAssurance, AuthSubject};
use graphql_orm::graphql::loaders::{
    CompositeRelationQueryKey, FederationEntityKey, RelationKey, RelationKeyPartKind,
    RelationQueryKey,
};
use graphql_orm::graphql::orm::{DbAuthContext, SqlValue};
use std::collections::HashSet;

fn context(scopes: &[&str]) -> DbAuthContext {
    DbAuthContext::from_subject(
        &AuthSubject::builder("principal")
            .scopes(scopes.iter().map(|value| (*value).into()).collect())
            .build(),
    )
}

fn relation(auth: DbAuthContext) -> RelationQueryKey {
    RelationQueryKey {
        relation: "Parent::children",
        parent_key: "parent".into(),
        parent_value: SqlValue::String("parent".into()),
        fk_column: "parent_id",
        where_signature: None,
        order_signature: None,
        page_signature: None,
        filter: None,
        sorts: vec![],
        pagination: None,
        auth_context: Some(auth),
    }
}

fn composite(auth: DbAuthContext) -> CompositeRelationQueryKey {
    CompositeRelationQueryKey {
        relation: "Parent::children",
        parent_key: RelationKey::single("parent".into()),
        parent_values: vec![SqlValue::String("parent".into())],
        fk_columns: vec!["parent_id"],
        key_part_kinds: vec![RelationKeyPartKind::String],
        where_signature: None,
        order_signature: None,
        page_signature: None,
        filter: None,
        sorts: vec![],
        pagination: None,
        auth_context: Some(auth),
    }
}

#[test]
fn delimiter_collision_cannot_merge_principals_or_loader_identities() {
    let left = context(&["read,write"]);
    let right = context(&["read", "write"]);
    assert_ne!(left.partition_key(), right.partition_key());
    assert_ne!(left.canonical_key(), right.canonical_key());
    assert_eq!(
        HashSet::from([relation(left.clone()), relation(right.clone())]).len(),
        2
    );
    let mut string_key = relation(left.clone());
    string_key.parent_key = "1".into();
    string_key.parent_value = SqlValue::String("1".into());
    let mut integer_key = string_key.clone();
    integer_key.parent_value = SqlValue::Int(1);
    assert_ne!(string_key, integer_key);

    assert_eq!(
        HashSet::from([composite(left.clone()), composite(right.clone())]).len(),
        2
    );
    let federation = |auth| FederationEntityKey {
        key: "Child::id",
        columns: vec!["id"],
        key_part_kinds: vec![RelationKeyPartKind::String],
        values: vec![SqlValue::String("parent".into())],
        representation: RelationKey::single("parent".into()),
        auth_context: Some(auth),
    };
    assert_eq!(
        HashSet::from([federation(left), federation(right)]).len(),
        2
    );
}

#[test]
fn subject_fields_and_optional_values_remain_structurally_distinct() {
    // Exhaust a small separator alphabet across field/list boundaries; includes
    // empty strings, Unicode and syntax formerly interpreted as separators.
    let atoms = ["", "|", ",", "id=", "tenant=", "α", "\0", "read,write"];
    let mut subjects = HashSet::new();
    let mut digests = HashSet::new();
    for id in atoms {
        for tenant in atoms {
            for scope in atoms {
                let subject = AuthSubject::builder(id)
                    .tenant_id(tenant)
                    .scopes(vec![scope.into()])
                    .build();
                assert!(subjects.insert(subject.partition_key()));
                assert!(digests.insert(subject.canonical_key()));
            }
        }
    }
    let base = AuthSubject::new("principal");
    let mut variants = vec![base.clone()];
    macro_rules! variation {
        ($field:ident, $value:expr) => {{
            let mut subject = base.clone();
            subject.$field = $value;
            variants.push(subject);
        }};
    }
    variation!(user_id, Some(String::new()));
    variation!(tenant_id, Some(String::new()));
    variation!(organization_id, Some(String::new()));
    variation!(token_id, Some(String::new()));
    variation!(session_id, Some(String::new()));
    variation!(actor_id, Some(String::new()));
    variation!(correlation_id, Some(String::new()));
    variation!(roles, vec![String::new()]);
    variation!(scopes, vec![String::new()]);
    variation!(claims, Some(serde_json::Value::Null));
    variation!(claims, Some(serde_json::json!({"tenant": "other"})));
    variation!(
        assurance,
        Some(AuthAssurance {
            authenticated_at: 1,
            methods: vec!["pwd".into()],
            acr: None,
            context: None,
            mfa_satisfied: false
        })
    );
    assert_eq!(
        variants
            .iter()
            .map(AuthSubject::partition_key)
            .collect::<HashSet<_>>()
            .len(),
        variants.len()
    );
    assert_eq!(
        variants
            .iter()
            .map(AuthSubject::canonical_key)
            .collect::<HashSet<_>>()
            .len(),
        variants.len()
    );
    let contexts = variants
        .iter()
        .map(DbAuthContext::from_subject)
        .collect::<Vec<_>>();
    assert_eq!(
        contexts
            .iter()
            .map(DbAuthContext::partition_key)
            .collect::<HashSet<_>>()
            .len(),
        contexts.len()
    );
    let mut changed_policy = contexts[0].clone();
    changed_policy.policy_version = Some(String::new());
    assert_ne!(changed_policy.partition_key(), contexts[0].partition_key());
}

#[test]
fn set_order_and_json_object_order_preserve_equivalent_partitions() {
    let mut left = AuthSubject::builder("principal")
        .roles(vec!["b".into(), "a".into()])
        .scopes(vec!["write".into(), "read".into()])
        .claims(serde_json::from_str(r#"{"z":{"b":1,"a":2},"a":true}"#).unwrap())
        .build();
    let mut right = left.clone();
    right.roles.reverse();
    right.roles.push("a".into());
    right.scopes.reverse();
    right.scopes.push("read".into());
    right.claims = Some(serde_json::from_str(r#"{"a":true,"z":{"a":2,"b":1}}"#).unwrap());
    assert_eq!(left.partition_key(), right.partition_key());
    assert_eq!(left.canonical_key(), right.canonical_key());
    assert_eq!(
        DbAuthContext::from_subject(&left).partition_key(),
        DbAuthContext::from_subject(&right).partition_key()
    );
    assert!(left.canonical_key().starts_with("auth-v2:"));
    left.claims = Some(serde_json::json!({"secret-marker": "private-claim-body"}));
    assert!(!format!("{:?}", left.partition_key()).contains("private-claim-body"));
    assert!(!left.canonical_key().contains("private-claim-body"));
}

#[test]
fn sql_relation_keys_preserve_binary_type_null_and_component_boundaries() {
    let uuid = graphql_orm::uuid::Uuid::parse_str("abcdef12-3456-789a-bcde-0123456789ab").unwrap();
    assert_eq!(
        RelationKey::from_sql_values(&[SqlValue::Uuid(uuid)]),
        RelationKey::from_projected_parts(
            vec![uuid.to_string().to_uppercase()],
            &[RelationKeyPartKind::Uuid]
        )
    );

    let values = vec![
        SqlValue::Bytes(vec![0xff]),
        SqlValue::Bytes(vec![0xfe]),
        SqlValue::Bytes(vec![]),
        SqlValue::String(String::new()),
        SqlValue::String("bin:ff".into()),
        SqlValue::String("1".into()),
        SqlValue::Int(1),
        SqlValue::Float(1.0),
        SqlValue::Bool(true),
        SqlValue::Null,
        SqlValue::StringNull,
        SqlValue::BytesNull,
        SqlValue::IntNull,
        SqlValue::FloatNull,
        SqlValue::BoolNull,
        SqlValue::UuidNull,
        SqlValue::JsonNull,
        SqlValue::Json(serde_json::Value::Null),
    ];
    let keys = values
        .iter()
        .map(|value| RelationKey::from_sql_values(std::slice::from_ref(value)))
        .collect::<HashSet<_>>();
    assert_eq!(keys.len(), values.len());
    assert_ne!(
        RelationKey::from_sql_values(&[
            SqlValue::String("a,b".into()),
            SqlValue::String("c".into())
        ]),
        RelationKey::from_sql_values(&[
            SqlValue::String("a".into()),
            SqlValue::String("b,c".into())
        ])
    );
    assert_eq!(
        RelationKey::from_sql_values(&[SqlValue::String("id".into()), SqlValue::Int(7)]),
        RelationKey::from_projected_parts(
            vec!["id".into(), "7".into()],
            &[RelationKeyPartKind::String, RelationKeyPartKind::Int]
        )
    );
}

#[cfg(feature = "sqlite")]
mod sqlite_batch {
    use super::*;
    use graphql_orm::prelude::*;
    use graphql_orm::sqlx::Row;

    #[derive(GraphQLEntity, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[graphql_entity(table = "partition_children", plural = "PartitionChildren")]
    struct Child {
        #[primary_key]
        #[filterable(type = "string")]
        #[sortable]
        id: String,
        parent_id: String,
    }

    impl BatchLoadEntity for Child {
        fn batch_column() -> &'static str {
            "parent_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            row.try_get("parent_id")
        }
    }

    #[tokio::test]
    async fn same_tick_dataloader_requests_preserve_both_auth_partitions() {
        let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        graphql_orm::sqlx::query(
            "CREATE TABLE partition_children (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        graphql_orm::sqlx::query("INSERT INTO partition_children VALUES ('child', 'parent')")
            .execute(&pool)
            .await
            .unwrap();
        let loader = async_graphql::dataloader::DataLoader::new(
            RelationLoader::<Child>::new(graphql_orm::db::Database::new(pool.clone())),
            tokio::spawn,
        );
        let keys = [
            composite(context(&["read,write"])),
            composite(context(&["read", "write"])),
        ];
        let result = loader.load_many(keys.clone()).await.unwrap();
        assert_eq!(
            result.len(),
            2,
            "batch deduplication must retain both principals"
        );
        for key in keys {
            assert_eq!(result[&key].entities[0].id, "child");
        }
        pool.close().await;
    }
}

#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(feature = "postgres")]
mod postgres_batch {
    use super::*;
    use graphql_orm::prelude::*;
    use graphql_orm::sqlx::Row;

    #[derive(GraphQLEntity, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[graphql_entity(
        backend = "postgres",
        table = "partition_children",
        plural = "PartitionChildren"
    )]
    struct Child {
        #[primary_key]
        #[filterable(type = "string")]
        #[sortable]
        id: String,
        parent_id: String,
    }

    impl BatchLoadEntity for Child {
        fn batch_column() -> &'static str {
            "parent_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            row.try_get("parent_id")
        }
    }

    #[tokio::test]
    async fn same_tick_batches_do_not_reuse_another_principals_rls_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut owned = super::owned_postgres::OwnedPostgres::start("auth-partitions")?;
        let owner = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&owned.url)
            .await?;
        for sql in [
            "CREATE ROLE auth_partition_reader NOLOGIN NOSUPERUSER NOBYPASSRLS",
            "CREATE TABLE partition_children (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL)",
            "INSERT INTO partition_children VALUES ('private-child', 'parent')",
            "ALTER TABLE partition_children ENABLE ROW LEVEL SECURITY",
            "ALTER TABLE partition_children FORCE ROW LEVEL SECURITY",
            "CREATE POLICY scoped_read ON partition_children FOR SELECT USING (COALESCE(NULLIF(current_setting('app.scopes', true), ''), '[]')::jsonb ? 'write')",
            "GRANT USAGE ON SCHEMA public TO auth_partition_reader",
            "GRANT SELECT ON partition_children TO auth_partition_reader",
        ] {
            graphql_orm::sqlx::query(sql).execute(&owner).await?;
        }
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    graphql_orm::sqlx::query("SET ROLE auth_partition_reader")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&owned.url)
            .await?;
        let loader = async_graphql::dataloader::DataLoader::new(
            RelationLoader::<Child>::new(graphql_orm::db::Database::new(pool.clone())),
            tokio::spawn,
        );
        let denied = composite(context(&["read,write"]));
        let allowed = composite(context(&["read", "write"]));
        let result = loader.load_many([denied.clone(), allowed.clone()]).await?;
        assert_eq!(result.len(), 2);
        assert!(
            result[&denied].entities.is_empty(),
            "a comma-containing scope does not authorize write"
        );
        assert_eq!(result[&denied].total_count, 0);
        assert_eq!(result[&allowed].entities[0].id, "private-child");
        assert_eq!(result[&allowed].total_count, 1);
        pool.close().await;
        owner.close().await;
        owned.cleanup()?;
        Ok(())
    }
}
