#![cfg(feature = "sqlite")]
//! `#[graphql_orm(version = "managed")]` advances on every generated write.
//!
//! A bare `#[graphql_orm(version)]` stays compare-and-swap only; the last test
//! pins that existing behaviour.

use graphql_orm::prelude::*;

fn resolver_name(camel: &'static str, pascal: &'static str) -> &'static str {
    if cfg!(feature = "resolver-case-pascal") {
        pascal
    } else {
        camel
    }
}

fn argument_name(camel: &'static str, pascal: &'static str) -> &'static str {
    if cfg!(feature = "argument-case-pascal") {
        pascal
    } else {
        camel
    }
}

fn field_name(camel: &'static str, pascal: &'static str) -> &'static str {
    if cfg!(feature = "field-case-pascal") {
        pascal
    } else {
        camel
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "managed_tickets",
    plural = "ManagedTickets",
    upsert = "reference",
    auth = "none"
)]
struct ManagedTicket {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    #[unique]
    reference: String,
    #[filterable(type = "string")]
    #[sortable]
    status: String,
    #[graphql_orm(version = "managed", default = "0")]
    #[filterable(type = "number")]
    #[sortable]
    row_version: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "cas_only_jobs", plural = "CasOnlyJobs", auth = "none")]
struct CasOnlyJob {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    status: String,
    #[graphql_orm(version, default = "0")]
    version: i64,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    table = "managed_slots",
    plural = "ManagedSlots",
    repository_mutations = true,
    unique_composite = "site,name"
)]
struct ManagedSlot {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    site: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    name: String,
    #[filterable(type = "string")]
    state: String,
    #[graphql_orm(version = "managed", default = "0")]
    #[filterable(type = "number")]
    revision: i64,
}

struct AllowEntityPolicy;

impl EntityPolicy for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

schema_roots! {
    query_custom_ops: [],
    entities: [ManagedTicket, CasOnlyJob],
}

async fn setup() -> graphql_orm::Result<Database<SqliteBackend>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "managed-version-init",
            "managed version test",
            &[
                ManagedTicket::metadata(),
                CasOnlyJob::metadata(),
                ManagedSlot::metadata(),
            ],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Ok(database)
}

fn create(reference: &str) -> CreateManagedTicketInput {
    CreateManagedTicketInput {
        reference: reference.to_string(),
        status: "open".to_string(),
    }
}

fn status(value: &str) -> UpdateManagedTicketInput {
    UpdateManagedTicketInput {
        status: Some(value.to_string()),
        ..Default::default()
    }
}

async fn stored_version(database: &Database<SqliteBackend>, id: &graphql_orm::uuid::Uuid) -> i64 {
    ManagedTicket::find_by_id(database, id)
        .await
        .expect("read ticket")
        .expect("ticket exists")
        .row_version
}

#[tokio::test]
async fn insert_starts_at_default_and_repository_updates_advance() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let ticket = ManagedTicket::insert(&database, create("T-1")).await?;
    assert_eq!(ticket.row_version, 0);

    let updated = ManagedTicket::update_by_id(&database, &ticket.id, status("assigned"))
        .await?
        .expect("ticket updated");
    assert_eq!(updated.row_version, 1);

    // An update that sets no field still advances: the row was written.
    let touched =
        ManagedTicket::update_by_id(&database, &ticket.id, UpdateManagedTicketInput::default())
            .await?
            .expect("ticket touched");
    assert_eq!(touched.row_version, 2);
    Ok(())
}

#[tokio::test]
async fn update_where_advances_every_matched_row_once() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let first = ManagedTicket::insert(&database, create("T-1")).await?;
    let second = ManagedTicket::insert(&database, create("T-2")).await?;
    let untouched = ManagedTicket::insert(&database, create("T-3")).await?;
    ManagedTicket::update_by_id(&database, &untouched.id, status("closed")).await?;

    let affected = ManagedTicket::update_where(
        &database,
        ManagedTicketWhereInput {
            status: Some(StringFilter {
                eq: Some("open".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
        status("triaged"),
    )
    .await?;
    assert_eq!(affected, 2);
    assert_eq!(stored_version(&database, &first.id).await, 1);
    assert_eq!(stored_version(&database, &second.id).await, 1);
    assert_eq!(stored_version(&database, &untouched.id).await, 1);
    Ok(())
}

#[tokio::test]
async fn transaction_writes_advance_per_statement_and_rollback_keeps_the_old_version()
-> graphql_orm::Result<()> {
    let database = setup().await?;
    let ticket = ManagedTicket::insert(&database, create("T-1")).await?;

    let id = ticket.id;
    database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.update_by_id::<ManagedTicket>(&id, status("assigned"))
                    .await?;
                tx.update_by_id::<ManagedTicket>(&id, status("in_progress"))
                    .await?;
                Ok::<_, OrmPublicError>(())
            })
        })
        .await
        .expect("transaction commits");
    assert_eq!(stored_version(&database, &ticket.id).await, 2);

    let failed = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.update_by_id::<ManagedTicket>(&id, status("closed"))
                    .await?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(stored_version(&database, &ticket.id).await, 2);
    Ok(())
}

#[tokio::test]
async fn upsert_update_branch_advances_and_insert_branch_starts_at_default()
-> graphql_orm::Result<()> {
    let database = setup().await?;
    let inserted = ManagedTicket::upsert(&database, create("T-1"))
        .await?
        .entity;
    assert_eq!(inserted.row_version, 0);
    let updated = ManagedTicket::upsert(
        &database,
        CreateManagedTicketInput {
            reference: "T-1".to_string(),
            status: "assigned".to_string(),
        },
    )
    .await?
    .entity;
    assert_eq!(updated.id, inserted.id);
    assert_eq!(updated.status, "assigned");
    assert_eq!(updated.row_version, 1);
    Ok(())
}

#[tokio::test]
async fn changed_events_carry_the_post_commit_version() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let mut events = database
        .ensure_event_sender::<ManagedTicketChangedEvent>()
        .subscribe();
    let ticket = ManagedTicket::insert(&database, create("T-1")).await?;
    assert_eq!(
        events
            .recv()
            .await
            .expect("create event")
            .entity
            .unwrap()
            .row_version,
        0
    );
    ManagedTicket::update_by_id(&database, &ticket.id, status("assigned")).await?;
    assert_eq!(
        events
            .recv()
            .await
            .expect("update event")
            .entity
            .unwrap()
            .row_version,
        1
    );
    Ok(())
}

#[tokio::test]
async fn advancing_past_i64_max_is_refused() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let ticket = ManagedTicket::insert(&database, create("T-1")).await?;
    sqlx::query("UPDATE managed_tickets SET row_version = ? WHERE id = ?")
        .bind(i64::MAX)
        .bind(ticket.id.to_string())
        .execute(database.pool())
        .await?;
    assert!(
        ManagedTicket::update_by_id(&database, &ticket.id, status("assigned"))
            .await
            .is_err()
    );
    let stored = ManagedTicket::find_by_id(&database, &ticket.id)
        .await?
        .expect("ticket exists");
    assert_eq!(stored.row_version, i64::MAX);
    assert_eq!(stored.status, "open");
    Ok(())
}

async fn graphql(database: &Database<SqliteBackend>, query: String) -> async_graphql::Response {
    schema_builder(database.clone())
        .finish()
        .execute(query)
        .await
}

#[tokio::test]
async fn graphql_update_and_delete_honour_expected_version() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let ticket = ManagedTicket::insert(&database, create("T-1")).await?;
    let update = resolver_name("updateManagedTicket", "UpdateManagedTicket");
    let delete = resolver_name("deleteManagedTicket", "DeleteManagedTicket");
    let id = argument_name("id", "Id");
    let input = argument_name("input", "Input");
    let expected = argument_name("expectedVersion", "ExpectedVersion");
    let status_field = field_name("status", "Status");
    let success = field_name("success", "Success");

    let stale = graphql(
        &database,
        format!(
            r#"mutation {{ {update}({id}: "{}", {input}: {{ {status_field}: "stale" }}, {expected}: 5) {{ {success} }} }}"#,
            ticket.id
        ),
    )
    .await;
    assert_eq!(stale.errors.len(), 1, "{stale:?}");
    assert_eq!(
        stale.errors[0]
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string)
            .as_deref(),
        Some("\"CONFLICT\"")
    );
    let unchanged = ManagedTicket::find_by_id(&database, &ticket.id)
        .await?
        .expect("ticket exists");
    assert_eq!(unchanged.status, "open");
    assert_eq!(unchanged.row_version, 0);

    let current = graphql(
        &database,
        format!(
            r#"mutation {{ {update}({id}: "{}", {input}: {{ {status_field}: "assigned" }}, {expected}: 0) {{ {success} }} }}"#,
            ticket.id
        ),
    )
    .await;
    assert!(current.errors.is_empty(), "{current:?}");
    assert_eq!(stored_version(&database, &ticket.id).await, 1);

    // Without ExpectedVersion the update applies and still advances.
    let unconditional = graphql(
        &database,
        format!(
            r#"mutation {{ {update}({id}: "{}", {input}: {{ {status_field}: "in_progress" }}) {{ {success} }} }}"#,
            ticket.id
        ),
    )
    .await;
    assert!(unconditional.errors.is_empty(), "{unconditional:?}");
    assert_eq!(stored_version(&database, &ticket.id).await, 2);

    let stale_delete = graphql(
        &database,
        format!(
            r#"mutation {{ {delete}({id}: "{}", {expected}: 1) {{ {success} }} }}"#,
            ticket.id
        ),
    )
    .await;
    assert_eq!(stale_delete.errors.len(), 1, "{stale_delete:?}");
    assert!(
        ManagedTicket::find_by_id(&database, &ticket.id)
            .await?
            .is_some()
    );

    let deleted = graphql(
        &database,
        format!(
            r#"mutation {{ {delete}({id}: "{}", {expected}: 2) {{ {success} }} }}"#,
            ticket.id
        ),
    )
    .await;
    assert!(deleted.errors.is_empty(), "{deleted:?}");
    assert!(
        ManagedTicket::find_by_id(&database, &ticket.id)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn cas_only_version_is_unchanged_by_ordinary_writes() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let job = CasOnlyJob::insert(
        &database,
        CreateCasOnlyJobInput {
            status: "pending".to_string(),
        },
    )
    .await?;
    let updated = CasOnlyJob::update_by_id(
        &database,
        &job.id,
        UpdateCasOnlyJobInput {
            status: Some("running".to_string()),
        },
    )
    .await?
    .expect("job updated");
    assert_eq!(updated.version, 0);

    let sdl = schema_builder(database.clone()).finish().sdl();
    assert!(
        !sdl.contains(argument_name("expectedVersion", "ExpectedVersion"))
            || sdl
                .matches(argument_name("expectedVersion", "ExpectedVersion"))
                .count()
                == 2,
        "only the managed entity's update and delete accept ExpectedVersion"
    );
    Ok(())
}

#[tokio::test]
async fn compare_and_swap_advances_a_managed_version_exactly_once() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let ticket = ManagedTicket::insert(&database, create("T-1")).await?;
    let outcome = ManagedTicket::compare_and_swap(
        &database,
        &ticket.id,
        0,
        ManagedTicketWhereInput::default(),
        status("assigned"),
    )
    .await?;
    let ConditionalUpdateOutcome::Updated(updated) = outcome else {
        panic!("matching CAS should update");
    };
    assert_eq!(updated.row_version, 1);
    assert!(matches!(
        ManagedTicket::compare_and_swap(
            &database,
            &ticket.id,
            0,
            ManagedTicketWhereInput::default(),
            status("stale"),
        )
        .await?,
        ConditionalUpdateOutcome::Conflict
    ));
    assert_eq!(stored_version(&database, &ticket.id).await, 1);
    Ok(())
}

#[tokio::test]
async fn key_based_repository_writes_advance() -> graphql_orm::Result<()> {
    let mut database = setup().await?;
    database.set_entity_policy(AllowEntityPolicy);
    let key = ManagedSlotKey {
        site: "north".into(),
        name: "a".into(),
    };
    ManagedSlot::insert(
        &database,
        CreateManagedSlotInput {
            site: "north".into(),
            name: "a".into(),
            state: "free".into(),
        },
    )
    .await?;
    ManagedSlot::insert(
        &database,
        CreateManagedSlotInput {
            site: "north".into(),
            name: "b".into(),
            state: "free".into(),
        },
    )
    .await?;
    let state = |value: &str| UpdateManagedSlotInput {
        state: Some(value.to_string()),
    };
    let by_key = ManagedSlot::update_by_key(&database, &key, state("held"))
        .await?
        .expect("slot updated");
    assert_eq!(by_key.revision, 1);

    let expected = ManagedSlotWhereInput {
        state: Some(StringFilter {
            eq: Some("held".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let PredicateUpdateOutcome::Updated(conditional) =
        ManagedSlot::update_if(&database, &key, expected, state("booked")).await?
    else {
        panic!("matching predicate should update");
    };
    assert_eq!(conditional.revision, 2);

    let outcome = ManagedSlot::update_where_bounded(
        &database,
        ManagedSlotWhereInput {
            site: Some(StringFilter {
                eq: Some("north".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
        state("closed"),
        MutationLimit::new(10).expect("limit"),
    )
    .await?;
    assert!(matches!(
        outcome,
        BoundedMutationOutcome::Applied { affected: 2 }
    ));
    let revisions = ManagedSlot::query(&database)
        .fetch_all()
        .await?
        .into_iter()
        .map(|slot| (slot.name, slot.revision))
        .collect::<Vec<_>>();
    assert!(revisions.contains(&("a".to_string(), 3)));
    assert!(revisions.contains(&("b".to_string(), 1)));
    Ok(())
}
