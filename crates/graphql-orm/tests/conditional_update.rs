#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "versioned_jobs", plural = "VersionedJobs")]
#[graphql_orm(search(index = true, tokenizer = "unicode61"))]
struct VersionedJob {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    status: String,
    #[graphql_orm(searchable(weight = "A"))]
    payload: String,
    #[graphql_orm(version, default = "0")]
    #[filterable(type = "number")]
    #[sortable]
    version: i64,
}

#[derive(Clone, Default)]
struct CasHook(Arc<Mutex<Vec<MutationPhase>>>);

impl MutationHook for CasHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _mutation: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.0.lock().expect("hook lock").push(event.phase.clone());
            Ok(())
        })
    }
}

fn create(status: &str, payload: &str) -> CreateVersionedJobInput {
    CreateVersionedJobInput {
        status: status.to_string(),
        payload: payload.to_string(),
    }
}

fn update(payload: &str) -> UpdateVersionedJobInput {
    UpdateVersionedJobInput {
        status: None,
        payload: Some(payload.to_string()),
    }
}

fn expected_status(status: &str) -> VersionedJobWhereInput {
    VersionedJobWhereInput {
        status: Some(StringFilter {
            eq: Some(status.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn setup() -> graphql_orm::Result<Database<SqliteBackend>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = database
        .schema()
        .plan_migration_to_entities("cas-init", "CAS test", &[VersionedJob::metadata()])
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Ok(database)
}

#[tokio::test]
async fn typed_status_and_version_cas_distinguishes_all_outcomes() -> graphql_orm::Result<()> {
    let mut database = setup().await?;
    let hook = CasHook::default();
    database.set_mutation_hook(hook.clone());
    let mut events = database
        .ensure_event_sender::<VersionedJobChangedEvent>()
        .subscribe();
    let job = VersionedJob::insert(&database, create("pending", "one")).await?;
    let _ = events.recv().await.expect("create event");
    hook.0.lock().expect("hook lock").clear();
    assert_eq!(job.version, 0);

    let updated = VersionedJob::compare_and_swap(
        &database,
        &job.id,
        0,
        expected_status("pending"),
        update("two"),
    )
    .await?;
    let ConditionalUpdateOutcome::Updated(updated) = updated else {
        panic!("matching CAS should update");
    };
    assert_eq!(updated.version, 1);
    assert_eq!(updated.payload, "two");
    assert_eq!(
        *hook.0.lock().expect("hook lock"),
        vec![MutationPhase::Before, MutationPhase::After]
    );
    assert_eq!(
        events
            .recv()
            .await
            .expect("CAS event")
            .entity
            .unwrap()
            .version,
        1
    );
    let hits = VersionedJob::search(
        database.pool(),
        SearchInput {
            query: "two".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .fetch_all()
    .await?;
    assert_eq!(hits.len(), 1);
    hook.0.lock().expect("hook lock").clear();

    assert!(matches!(
        VersionedJob::compare_and_swap(
            &database,
            &job.id,
            0,
            expected_status("pending"),
            update("stale"),
        )
        .await?,
        ConditionalUpdateOutcome::Conflict
    ));
    assert!(hook.0.lock().expect("hook lock").is_empty());
    assert!(events.try_recv().is_err());
    assert!(matches!(
        VersionedJob::compare_and_swap(
            &database,
            &job.id,
            1,
            expected_status("running"),
            update("wrong-state"),
        )
        .await?,
        ConditionalUpdateOutcome::Conflict
    ));
    assert!(matches!(
        VersionedJob::compare_and_swap(
            &database,
            &graphql_orm::uuid::Uuid::new_v4(),
            0,
            expected_status("pending"),
            update("missing"),
        )
        .await?,
        ConditionalUpdateOutcome::NotFound
    ));
    Ok(())
}

#[tokio::test]
async fn concurrent_cas_has_exactly_one_winner_and_works_in_mutation_context()
-> graphql_orm::Result<()> {
    let database = setup().await?;
    let job = VersionedJob::insert(&database, create("pending", "one")).await?;
    let left_db = database.clone();
    let right_db = database.clone();
    let id = job.id;
    let left = tokio::spawn(async move {
        VersionedJob::compare_and_swap(&left_db, &id, 0, expected_status("pending"), update("left"))
            .await
    });
    let right = tokio::spawn(async move {
        VersionedJob::compare_and_swap(
            &right_db,
            &job.id,
            0,
            expected_status("pending"),
            update("right"),
        )
        .await
    });
    let outcomes = [
        left.await.expect("left task")?,
        right.await.expect("right task")?,
    ];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ConditionalUpdateOutcome::Updated(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ConditionalUpdateOutcome::Conflict))
            .count(),
        1
    );

    let current = VersionedJob::find_by_id(&database, &id)
        .await?
        .expect("job exists");
    let in_transaction = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.compare_and_swap::<VersionedJob>(
                    &id,
                    current.version,
                    expected_status("pending"),
                    update("transaction"),
                )
                .await
                .map_err(Into::into)
            })
        })
        .await
        .expect("transaction CAS commits");
    assert!(matches!(
        in_transaction,
        ConditionalUpdateOutcome::Updated(_)
    ));
    Ok(())
}
