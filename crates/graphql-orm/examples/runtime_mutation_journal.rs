//! Owned disposable runtime mutation + repository journal example.
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
mod example {
    use graphql_orm::futures::future::BoxFuture;
    use graphql_orm::{graphql::orm::*, prelude::*};
    use std::sync::Arc;
    #[cfg(feature = "sqlite")]
    type Backend = SqliteBackend;
    #[cfg(feature = "postgres")]
    type Backend = PostgresBackend;
    #[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
    #[repository_entity(table = "example_journal", plural = "ExampleJournal")]
    struct Journal {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        id: String,
        action: String,
    }
    #[derive(GraphQLSchemaEntity, Clone, serde::Serialize, serde::Deserialize)]
    #[graphql_entity(table = "example_records", plural = "ExampleRecords")]
    struct Record {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        id: i64,
        #[filterable(type = "string")]
        label: String,
    }
    // Explicit public authority for this isolated example. A real host validates its
    // pinned identity, policy/public revision, input/CAS fields and return fields here.
    struct PublicAuthority(RuntimeProjection);
    impl RuntimeWriteAuthority<Backend> for PublicAuthority {
        fn authorize_intent<'a>(
            &'a self,
            _: RuntimeWriteIntent<'a>,
            _: &'a mut MutationContext<'_, Backend>,
        ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
            Box::pin(async move { RuntimeWriteGrant::new(self.0.clone(), None) })
        }
        fn authorize_preimage<'a>(
            &'a self,
            _: RuntimePreimageCheck<'a>,
            _: &'a mut MutationContext<'_, Backend>,
        ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
            Box::pin(async { Ok(()) })
        }
        fn authorize_result<'a>(
            &'a self,
            check: RuntimeResultCheck<'a>,
            _: &'a mut MutationContext<'_, Backend>,
        ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
            Box::pin(async move { Ok(RuntimeReturnGrant::new(check.intent.returning().cloned())) })
        }
    }
    pub async fn run(db: Database<Backend>) -> Result<(), Box<dyn std::error::Error>> {
        let schema =
            Arc::new(RuntimeSchema::from_static_entities(&[Record::metadata()])?.validate()?);
        let target = schema
            .physical_schema::<Backend>(Default::default())?
            .with_static_entities(&[Journal::metadata()])?;
        let ownership = ManagedTableSet::new(["example_records".into(), "example_journal".into()])?;
        let plan = db
            .schema()
            .plan_owned_migration(
                "example",
                "isolated example",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await?;
        db.schema()
            .apply_owned_migration(&plan, ApplyOptions::default())
            .await?;
        let environment = Arc::new(
            db.schema()
                .runtime_mutation_environment(schema.clone(), &target, &ownership)
                .await?,
        );
        let collection = schema.resolve_collection(&schema.schema().collections[0].id)?;
        let id = schema.resolve_field(&collection, &FieldId::new("example_records.id")?)?;
        let label = schema.resolve_field(&collection, &FieldId::new("example_records.label")?)?;
        let projection = schema.resolve_projection(&collection, &[id.clone(), label.clone()])?;
        let expected = schema.runtime_compare(
            &collection,
            &label,
            RuntimeScalarOperator::Eq,
            RuntimeValue::String("draft".into()),
            Default::default(),
        )?;
        let requests = [
            schema.runtime_create_request(
                &collection,
                &[
                    (id.clone(), RuntimeValue::Integer(7)),
                    (label.clone(), RuntimeValue::String("draft".into())),
                ],
                Some(projection.clone()),
                Default::default(),
            )?,
            schema.runtime_update_request(
                schema.runtime_key(&collection, &[(id.clone(), RuntimeValue::Integer(7))])?,
                &[(label, RuntimeValue::String("published".into()))],
                Some(expected),
                Some(projection.clone()),
                Default::default(),
            )?,
            schema.runtime_delete_request(
                schema.runtime_key(&collection, &[(id, RuntimeValue::Integer(7))])?,
                None,
                Some(projection.clone()),
                Default::default(),
            )?,
        ];
        for (index, request) in requests.into_iter().enumerate() {
            let environment = environment.clone();
            let authority = PublicAuthority(projection.clone());
            let committed = db
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let pending = tx
                            .mutate_runtime(&environment, &request, &authority)
                            .await?;
                        tx.insert::<Journal>(CreateJournalInput {
                            id: index.to_string(),
                            action: format!("{:?}", pending.action()),
                        })
                        .await?;
                        Ok(pending)
                    })
                })
                .await?;
            println!("Committed {:?}", committed.action());
        }
        assert_eq!(Journal::count_all(&db).await?, 3);
        db.pool().close().await;
        Ok(())
    }
}
#[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
#[path = "../tests/support/owned_postgres.rs"]
mod owned_postgres;
#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
    example::run(graphql_orm::db::Database::connect_sqlite("sqlite::memory:").await?).await?;
    #[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
    {
        let mut owned = owned_postgres::OwnedPostgres::start("runtime-journal-example")?;
        example::run(graphql_orm::db::Database::connect_postgres(&owned.url).await?).await?;
        owned.cleanup()?;
    }
    Ok(())
}
