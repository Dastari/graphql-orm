//! Search helper tables of long base table names (#528).
//!
//! PostgreSQL keeps 63 bytes of an identifier. `__graphql_orm_search_<table>`
//! is longer than that for a base table name above 42 bytes, so two such
//! tables that agree in their first 42 bytes used to share one truncated
//! helper name: the second search index could not be created. The helper
//! name is now bounded and ends in a digest of the whole base table name.
//! PostgreSQL cases own a verified disposable server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use futures::future::BoxFuture;
use graphql_orm::{graphql::errors::OrmPublicError, graphql::orm::*, prelude::*};
use std::sync::Arc;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

/// Two 60-byte table names that differ only in their last byte.
fn table(last: char) -> String {
    format!("long_search_{}_{last}", "x".repeat(46))
}

#[test]
fn helper_names_are_bounded_and_distinct() {
    // Names that fit are unchanged, up to the longest one that does.
    assert_eq!(
        search_table_name("articles"),
        "__graphql_orm_search_articles"
    );
    let fits = "t".repeat(42);
    assert_eq!(
        search_table_name(&fits),
        format!("__graphql_orm_search_{fits}")
    );
    assert_eq!(search_table_name(&fits).len(), 63);
    // One byte more is bounded, keeps its recognisable start, and is stable.
    let long = "t".repeat(43);
    let bounded = search_table_name(&long);
    assert_eq!(bounded.len(), 63);
    assert!(bounded.starts_with(&format!("__graphql_orm_search_{}_", "t".repeat(25))));
    assert_eq!(bounded, search_table_name(&long));
    assert_eq!(
        bounded,
        "__graphql_orm_search_ttttttttttttttttttttttttt_80383f974f22964f"
    );
    // Tables that agree in every byte PostgreSQL would keep stay distinct,
    // and so do names that sanitize to the same text.
    let (a, b) = (
        search_table_name(&table('a')),
        search_table_name(&table('b')),
    );
    assert!(a.len() <= 63 && b.len() <= 63, "{a} {b}");
    assert_ne!(a, b);
    assert_eq!(a[..46], b[..46]);
    assert_ne!(
        search_table_name(&format!("{}-x", "t".repeat(50))),
        search_table_name(&format!("{}_x", "t".repeat(50)))
    );
    assert!(search_table_name(&"é".repeat(40)).len() <= 63);
}

fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}
fn field(id: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        id: fid(id),
        api_name: id.into(),
        physical_column: id.into(),
        value_kind: kind,
        nullable: false,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        description: None,
        classification: None,
        export: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}
fn collection(name: &str, last: char) -> RuntimeCollection {
    let (key, title) = (format!("{name}_id"), format!("{name}_title"));
    RuntimeCollection {
        node_identity: None,
        live_subscriptions: None,
        id: CollectionId::new(name).unwrap(),
        api_type_name: format!("Long{name}"),
        api_plural_name: format!("Long{name}s"),
        physical_table: table(last),
        primary_key: vec![fid(&key)],
        append_only: false,
        retention_purge: false,
        fields: vec![
            field(&key, RuntimeValueKind::Integer),
            field(&title, RuntimeValueKind::String),
        ],
        relations: vec![],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
        description: None,
        classification: None,
        label_fields: vec![],
        search: Some(RuntimeSearch {
            fields: vec![RuntimeSearchField {
                field: fid(&title),
                weight: RuntimeSearchWeight::A,
                json_path: None,
            }],
            language: None,
            tokenizer: None,
            remove_diacritics: false,
            min_token_len: None,
        }),
    }
}

struct Allow(RuntimeProjection);
impl RuntimeWriteAuthority<Backend> for Allow {
    fn authorize_intent<'a>(
        &'a self,
        _check: RuntimeWriteIntent<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move { RuntimeWriteGrant::new(self.0.clone(), None) })
    }
    fn authorize_preimage<'a>(
        &'a self,
        _check: RuntimePreimageCheck<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async { Ok(()) })
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        Box::pin(async move { Ok(RuntimeReturnGrant::new(check.intent.returning().cloned())) })
    }
}

#[tokio::test]
async fn long_tables_sharing_a_prefix_keep_separate_search_indexes() {
    #[cfg(feature = "sqlite")]
    let db = Database::<Backend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    #[cfg(feature = "postgres")]
    let owned = owned_postgres::OwnedPostgres::start("runtime-search-long-tables").unwrap();
    #[cfg(feature = "postgres")]
    let db = Database::<Backend>::connect_postgres(&owned.url)
        .await
        .unwrap();
    let schema = Arc::new(
        RuntimeSchema {
            repository_only: false,
            format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
            collections: vec![collection("alpha", 'a'), collection("beta", 'b')],
        }
        .validate()
        .unwrap(),
    );
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new([table('a'), table('b')]).unwrap();
    let plan = db
        .schema()
        .plan_owned_migration(
            "init",
            "long search tables",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    // Before the fix PostgreSQL refused the second index: both helper tables
    // truncated to one name.
    db.schema()
        .apply_runtime_migration(&plan, &schema, ApplyOptions::default(), Default::default())
        .await
        .unwrap();
    let environment = Arc::new(
        db.schema()
            .runtime_mutation_environment(schema.clone(), &target, &ownership)
            .await
            .unwrap(),
    );
    // Each helper table exists under exactly the name the ORM computes.
    #[cfg(feature = "postgres")]
    for last in ['a', 'b'] {
        let name = search_table_name(&table(last));
        let found: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_catalog.pg_class WHERE relname = $1 AND relkind = 'r'",
        )
        .bind(&name)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(found, 1, "{name}");
    }
    // A row in each collection, found only through its own index.
    for (name, title) in [("alpha", "harbour bridge"), ("beta", "mountain tunnel")] {
        let collection = schema
            .resolve_collection(&CollectionId::new(name).unwrap())
            .unwrap();
        let field = |suffix: &str| {
            schema
                .resolve_field(&collection, &fid(&format!("{name}_{suffix}")))
                .unwrap()
        };
        let projection = schema
            .resolve_projection(&collection, &[field("id"), field("title")])
            .unwrap();
        let request = schema
            .runtime_create_request(
                &collection,
                &[
                    (field("id"), RuntimeValue::Integer(1)),
                    (field("title"), RuntimeValue::String(title.into())),
                ],
                Some(projection.clone()),
                Default::default(),
            )
            .unwrap();
        let (environment, authority) = (environment.clone(), Allow(projection));
        db.transaction(TransactionMode::StateMachine, move |tx| {
            Box::pin(async move {
                tx.mutate_runtime(&environment, &request, &authority)
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .unwrap();
    }
    for (name, own, other) in [("alpha", "bridge", "tunnel"), ("beta", "tunnel", "bridge")] {
        let collection = schema
            .resolve_collection(&CollectionId::new(name).unwrap())
            .unwrap();
        let id = schema
            .resolve_field(&collection, &fid(&format!("{name}_id")))
            .unwrap();
        let projection = schema.resolve_projection(&collection, &[id]).unwrap();
        for (query, hits) in [(own, 1), (other, 0)] {
            let search = schema
                .runtime_search_request(
                    &collection,
                    graphql_orm::graphql::filters::SearchInput {
                        query: query.into(),
                        mode: None,
                        min_score: None,
                    },
                    true,
                    RuntimeSearchLimits::default(),
                )
                .unwrap();
            let request = schema
                .runtime_read_request(
                    &collection,
                    &projection,
                    None,
                    schema
                        .runtime_order(&collection, None, Default::default())
                        .unwrap(),
                    RuntimePageRequest::first(10, None),
                    true,
                    Default::default(),
                )
                .unwrap()
                .with_search(search)
                .unwrap();
            let found = db.execute_runtime_read(&request, None).await.unwrap();
            assert_eq!(found.edges.len(), hits, "{name}: {query}");
        }
    }
    db.pool().close().await;
    #[cfg(feature = "postgres")]
    {
        let mut owned = owned;
        owned.cleanup().unwrap();
    }
}
