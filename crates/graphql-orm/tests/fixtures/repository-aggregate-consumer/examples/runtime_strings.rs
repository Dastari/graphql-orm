//! Schema-bound literal string pages without handwritten query SQL or GraphQL derives.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod supported {
    use graphql_orm::prelude::*;
    use repository_aggregate_consumer::Backend;

    #[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
    #[cfg_attr(
        feature = "sqlite",
        repository_entity(
            backend = "sqlite",
            table = "private_string_history",
            plural = "PrivateStringHistory"
        )
    )]
    #[cfg_attr(
        all(feature = "postgres", not(feature = "sqlite")),
        repository_entity(
            backend = "postgres",
            table = "private_string_history",
            plural = "PrivateStringHistory"
        )
    )]
    struct PrivateHistory {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        id: String,
        event: Option<String>,
        tenant: String,
    }
    fn field(name: &str, nullable: bool) -> RuntimeField {
        RuntimeField {
            id: FieldId::new(name).unwrap(),
            api_name: name.into(),
            physical_column: name.into(),
            value_kind: RuntimeValueKind::String,
            nullable,
            unique: name == "id",
            filterable: true,
            sortable: true,
            generated: false,
            default: None,
        }
    }
    pub async fn run(db: &Database<Backend>) -> Result<(), Box<dyn std::error::Error>> {
        // Setup is for this example's owned database only. Read requests issue no DDL.
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "example",
                "owned string example",
                &[PrivateHistory::metadata()],
            )
            .await?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
        for (index, event) in ["alpha", "ALPHA", "πfoo", "πfoo%", "suffix", "Ω", ""]
            .iter()
            .enumerate()
        {
            PrivateHistory::insert(
                db,
                CreatePrivateHistoryInput {
                    id: index.to_string(),
                    event: Some((*event).into()),
                    tenant: "partition-a".into(),
                },
            )
            .await?;
        }
        PrivateHistory::insert(
            db,
            CreatePrivateHistoryInput {
                id: "private".into(),
                event: Some("πfoo".into()),
                tenant: "partition-b".into(),
            },
        )
        .await?;
        PrivateHistory::insert(
            db,
            CreatePrivateHistoryInput {
                id: "null".into(),
                event: None,
                tenant: "partition-a".into(),
            },
        )
        .await?;
        let collection_id = CollectionId::new("history")?;
        let schema = RuntimeSchema {
            format_version: 1,
            collections: vec![RuntimeCollection {
                id: collection_id.clone(),
                api_type_name: "PrivateHistory".into(),
                api_plural_name: "PrivateStringHistory".into(),
                physical_table: "private_string_history".into(),
                primary_key: vec![FieldId::new("id")?],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    field("id", false),
                    field("event", true),
                    field("tenant", false),
                ],
                relations: vec![],
                indexes: vec![],
                composite_unique: vec![],
                default_order: vec![],
            }],
        }
        .validate()?;
        let collection = schema.resolve_collection(&collection_id)?;
        let event = schema.resolve_field(&collection, &FieldId::new("event")?)?;
        let tenant = schema.resolve_field(&collection, &FieldId::new("tenant")?)?;
        let id = schema.resolve_field(&collection, &FieldId::new("id")?)?;
        let limits = RuntimeQueryLimits::default();
        let projection = schema.resolve_projection(&collection, std::slice::from_ref(&event))?;
        let visibility = schema.runtime_compare(
            &collection,
            &tenant,
            RuntimeScalarOperator::Eq,
            RuntimeValue::String("partition-a".into()),
            limits,
        )?;
        for (operator, operand, expected) in [
            (RuntimeScalarOperator::StartsWith, "π", 2),
            (RuntimeScalarOperator::EndsWith, "foo", 1),
            (RuntimeScalarOperator::EndsWith, "", 7),
        ] {
            let filter = schema.runtime_compare(
                &collection,
                &event,
                operator,
                RuntimeValue::String(operand.into()),
                limits,
            )?;
            let filter =
                schema.runtime_and(&collection, vec![filter, visibility.clone()], limits)?;
            let order = schema.runtime_order(
                &collection,
                Some(vec![RuntimeOrderInput {
                    field: event.clone(),
                    direction: RuntimeOrderDirection::Asc,
                    nulls: RuntimeNullPlacement::Last,
                }]),
                limits,
            )?;
            let mut cursor = None;
            let mut seen = 0;
            loop {
                let request = schema.runtime_read_request(
                    &collection,
                    &projection,
                    Some(filter.clone()),
                    order.clone(),
                    RuntimePageRequest::first(2, cursor),
                    true,
                    limits,
                )?;
                let page = db.execute_runtime_read(&request, None).await?;
                assert_eq!(page.total_count, Some(expected));
                for edge in &page.edges {
                    assert!(edge.node.string(&event).is_ok());
                    assert_eq!(edge.node.state(&id)?, RuntimeFieldState::Unloaded);
                    assert_eq!(edge.node.state(&tenant)?, RuntimeFieldState::Unloaded);
                }
                seen += page.edges.len() as i64;
                if !page.page_info.has_next_page {
                    break;
                }
                cursor = page.page_info.end_cursor;
            }
            assert_eq!(seen, expected);
        }
        println!(
            "Literal prefix/suffix pages, SQL NULL exclusion, private projection and visibility counts verified"
        );
        Ok(())
    }
}
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use supported::run;

#[cfg(feature = "sqlite")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = graphql_orm::db::Database::<graphql_orm::prelude::SqliteBackend>::connect_sqlite(
        "sqlite::memory:",
    )
    .await?;
    run(&db).await?;
    db.pool().close().await;
    Ok(())
}
#[cfg(not(feature = "sqlite"))]
fn main() {
    println!(
        "Use the test-owned PostgreSQL runtime_strings test. MSSQL runtime reads remain unsupported."
    );
}
