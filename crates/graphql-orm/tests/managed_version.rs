#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! `#[graphql_orm(version = "managed")]` advances on every generated write.
//!
//! A bare `#[graphql_orm(version)]` stays compare-and-swap only; the CAS-only
//! scenario pins that existing behaviour.
//!
//! SQLite runs every scenario as its own test. PostgreSQL and SQL Server run
//! all scenarios in one ignored test against a single test-owned container,
//! resetting the tables between scenarios:
//!
//! ```sh
//! cargo test -p graphql-orm --no-default-features --features postgres --test managed_version -- --ignored
//! cargo test -p graphql-orm --no-default-features --features mssql --test managed_version -- --ignored
//! ```

use graphql_orm::async_graphql::Request;
use graphql_orm::prelude::*;

#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{argument_name, field_name, resolver_name};

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "managed_tickets",
        plural = "ManagedTickets",
        upsert = "reference",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "managed_tickets",
        plural = "ManagedTickets",
        upsert = "reference",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "managed_tickets",
        plural = "ManagedTickets",
        upsert = "reference",
        auth = "none"
    )
)]
struct ManagedTicket {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
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
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "cas_only_jobs",
        plural = "CasOnlyJobs",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "cas_only_jobs",
        plural = "CasOnlyJobs",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "cas_only_jobs",
        plural = "CasOnlyJobs",
        auth = "none"
    )
)]
struct CasOnlyJob {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    status: String,
    #[graphql_orm(version, default = "0")]
    version: i64,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "managed_slots",
        plural = "ManagedSlots",
        repository_mutations = true,
        unique_composite = "site,name"
    )
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "managed_slots",
        plural = "ManagedSlots",
        repository_mutations = true,
        unique_composite = "site,name"
    )
)]
#[cfg_attr(
    feature = "mssql",
    repository_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "managed_slots",
        plural = "ManagedSlots",
        repository_mutations = true,
        unique_composite = "site,name"
    )
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

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [ManagedTicket, CasOnlyJob], }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [ManagedTicket, CasOnlyJob], }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [ManagedTicket, CasOnlyJob], }

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

/// Replaces the whole update input, as a status-only `before_update` transform does.
struct ReplaceUpdateInput;

impl graphql_orm::graphql::orm::WriteInputTransform for ReplaceUpdateInput {
    fn before_update<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        entity_name: &'static str,
        _existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if entity_name == "ManagedTicket" {
                let input = input
                    .downcast_mut::<UpdateManagedTicketInput>()
                    .ok_or_else(|| async_graphql::Error::new("unexpected update input type"))?;
                *input = UpdateManagedTicketInput {
                    status: Some("replaced".to_string()),
                    ..Default::default()
                };
            }
            Ok(())
        })
    }
}

#[cfg(not(feature = "sqlite"))]
const TABLES: [&str; 3] = ["managed_tickets", "cas_only_jobs", "managed_slots"];

struct Fixture {
    db: Database,
    #[cfg(feature = "mssql")]
    owner: owned_mssql::OwnedSqlServer,
}

impl Fixture {
    fn finish(self) -> TestResult {
        #[cfg(feature = "mssql")]
        {
            let Self { db, mut owner } = self;
            drop(db);
            owner.cleanup()?;
        }
        Ok(())
    }
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    // The pool keeps the owned container alive; it is removed when the last clone drops.
    #[cfg(feature = "postgres")]
    let mut db = Database::new(owned_postgres::OwnedPostgres::pool("managed-version", 12).await?);
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let plan = db
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
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    #[cfg(feature = "mssql")]
    let (mut db, owner) = {
        let owner = owned_mssql::OwnedSqlServer::start().await?;
        owned_mssql::execute_batch(
            &owner.connection_string,
            "CREATE TABLE managed_tickets (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             reference NVARCHAR(64) NOT NULL UNIQUE, status NVARCHAR(64) NOT NULL, \
             row_version BIGINT NOT NULL DEFAULT 0); \
             CREATE TABLE cas_only_jobs (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             status NVARCHAR(64) NOT NULL, version BIGINT NOT NULL DEFAULT 0); \
             CREATE TABLE managed_slots (site NVARCHAR(64) NOT NULL, name NVARCHAR(64) NOT NULL, \
             state NVARCHAR(64) NOT NULL, revision BIGINT NOT NULL DEFAULT 0, \
             PRIMARY KEY (site, name));",
        )
        .await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        let db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        (db, owner)
    };
    db.set_entity_policy(AllowEntityPolicy);
    Ok(Fixture {
        db,
        #[cfg(feature = "mssql")]
        owner,
    })
}

async fn execute(db: &Database, sql: &str) -> graphql_orm::Result<()> {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[]).await?;
    Ok(())
}

#[cfg(not(feature = "sqlite"))]
async fn reset(db: &Database) -> graphql_orm::Result<()> {
    for table in TABLES {
        execute(db, &format!("DELETE FROM {table}")).await?;
    }
    Ok(())
}

fn create(id: &str) -> CreateManagedTicketInput {
    CreateManagedTicketInput {
        id: id.to_string(),
        reference: format!("ref-{id}"),
        status: "open".to_string(),
    }
}

fn status(value: &str) -> UpdateManagedTicketInput {
    UpdateManagedTicketInput {
        status: Some(value.to_string()),
        ..Default::default()
    }
}

async fn load_ticket(db: &Database, id: &str) -> ManagedTicket {
    ManagedTicket::find_by_id(db, &id.to_string())
        .await
        .expect("read ticket")
        .expect("ticket exists")
}

async fn stored_version(db: &Database, id: &str) -> i64 {
    load_ticket(db, id).await.row_version
}

async fn set_ticket_version(db: &Database, id: &str, version: i64) -> graphql_orm::Result<()> {
    execute(
        db,
        &format!("UPDATE managed_tickets SET row_version = {version} WHERE id = '{id}'"),
    )
    .await
}

async fn graphql(db: &Database, query: String) -> async_graphql::Response {
    schema_builder(db.clone()).finish().execute(query).await
}

fn error_code(response: &async_graphql::Response) -> Option<String> {
    response
        .errors
        .first()
        .and_then(|error| error.extensions.as_ref())
        .and_then(|extensions| extensions.get("code"))
        .map(ToString::to_string)
}

/// A GraphQL mutation that applied: no errors, and no payload-level error.
fn applied(response: &async_graphql::Response, field: &str) -> bool {
    let success = field_name("success");
    response.errors.is_empty()
        && response
            .data
            .clone()
            .into_json()
            .ok()
            .is_some_and(|data| data[field][success] == true)
}

fn update_mutation(id: &str, new_status: &str, expected: Option<i64>) -> String {
    let update = resolver_name("updateManagedTicket");
    let id_arg = argument_name("id");
    let input = argument_name("input");
    let expected_arg = argument_name("expectedVersion");
    let status_field = field_name("status");
    let success = field_name("success");
    let expected = expected
        .map(|version| format!(", {expected_arg}: {version}"))
        .unwrap_or_default();
    format!(
        r#"mutation {{ {update}({id_arg}: "{id}", {input}: {{ {status_field}: "{new_status}" }}{expected}) {{ {success} }} }}"#
    )
}

async fn insert_starts_at_default_and_repository_updates_advance(db: &Database) -> TestResult {
    let ticket = ManagedTicket::insert(db, create("t-1")).await?;
    assert_eq!(ticket.row_version, 0);

    let updated = ManagedTicket::update_by_id(db, &ticket.id, status("assigned"))
        .await?
        .expect("ticket updated");
    assert_eq!(updated.row_version, 1);

    // An update that sets no field still advances: the row was written.
    let touched = ManagedTicket::update_by_id(db, &ticket.id, UpdateManagedTicketInput::default())
        .await?
        .expect("ticket touched");
    assert_eq!(touched.row_version, 2);
    Ok(())
}

async fn update_where_advances_every_matched_row_once(db: &Database) -> TestResult {
    ManagedTicket::insert(db, create("t-1")).await?;
    ManagedTicket::insert(db, create("t-2")).await?;
    let untouched = ManagedTicket::insert(db, create("t-3")).await?;
    ManagedTicket::update_by_id(db, &untouched.id, status("closed")).await?;

    let open = || ManagedTicketWhereInput {
        status: Some(StringFilter {
            eq: Some("open".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let affected = ManagedTicket::update_where(db, open(), status("triaged")).await?;
    assert_eq!(affected, 2);
    assert_eq!(stored_version(db, "t-1").await, 1);
    assert_eq!(stored_version(db, "t-2").await, 1);
    assert_eq!(stored_version(db, "t-3").await, 1);

    // Generated GraphQL updateMany takes the same path.
    let update_many = resolver_name("updateManagedTickets");
    let where_arg = argument_name("where");
    let input = argument_name("input");
    let status_field = field_name("status");
    let eq = field_name("eq");
    let response = graphql(
        db,
        format!(
            r#"mutation {{ {update_many}({where_arg}: {{ {status_field}: {{ {eq}: "triaged" }} }}, {input}: {{ {status_field}: "queued" }}) {{ __typename }} }}"#
        ),
    )
    .await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(stored_version(db, "t-1").await, 2);
    assert_eq!(stored_version(db, "t-2").await, 2);
    assert_eq!(stored_version(db, "t-3").await, 1);
    Ok(())
}

async fn transaction_writes_advance_per_statement_and_rollback_keeps_the_old_version(
    db: &Database,
) -> TestResult {
    let ticket = ManagedTicket::insert(db, create("t-1")).await?;
    ManagedTicket::insert(db, create("t-2")).await?;

    let id = ticket.id.clone();
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            tx.update_by_id::<ManagedTicket>(&id, status("assigned"))
                .await?;
            tx.update_by_id::<ManagedTicket>(&id, status("in_progress"))
                .await?;
            // A predicate write over the same row in the same transaction
            // advances once more, never skipped.
            let same_row = ManagedTicketWhereInput {
                id: Some(StringFilter {
                    eq: Some(id.clone()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            tx.update_where::<ManagedTicket>(same_row, status("held"))
                .await?;
            Ok::<_, OrmPublicError>(())
        })
    })
    .await
    .expect("transaction commits");
    assert_eq!(stored_version(db, "t-1").await, 3);
    assert_eq!(stored_version(db, "t-2").await, 0);

    let id = ticket.id.clone();
    let failed = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.update_by_id::<ManagedTicket>(&id, status("closed"))
                    .await?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(failed.is_err());
    let stored = load_ticket(db, "t-1").await;
    assert_eq!((stored.row_version, stored.status.as_str()), (3, "held"));
    Ok(())
}

async fn upsert_update_branch_advances_and_insert_branch_starts_at_default(
    db: &Database,
) -> TestResult {
    let inserted = ManagedTicket::upsert(db, create("t-1")).await?.entity;
    assert_eq!(inserted.row_version, 0);
    let updated = ManagedTicket::upsert(
        db,
        CreateManagedTicketInput {
            status: "assigned".to_string(),
            ..create("t-1")
        },
    )
    .await?
    .entity;
    assert_eq!(updated.id, inserted.id);
    assert_eq!(updated.status, "assigned");
    assert_eq!(updated.row_version, 1);

    // The generated GraphQL upsert shares the managed update branch.
    let upsert = resolver_name("upsertManagedTicket");
    let input = argument_name("input");
    let id_field = field_name("id");
    let reference_field = field_name("reference");
    let status_field = field_name("status");
    let response = graphql(
        db,
        format!(
            r#"mutation {{ {upsert}({input}: {{ {id_field}: "t-1", {reference_field}: "ref-t-1", {status_field}: "in_progress" }}) {{ __typename }} }}"#
        ),
    )
    .await;
    assert!(response.errors.is_empty(), "{response:?}");
    let stored = load_ticket(db, "t-1").await;
    assert_eq!(
        (stored.row_version, stored.status.as_str()),
        (2, "in_progress")
    );
    Ok(())
}

async fn changed_events_carry_the_post_commit_version(db: &Database) -> TestResult {
    let mut events = db
        .ensure_event_sender::<ManagedTicketChangedEvent>()
        .subscribe();
    let mut next_version = async || {
        events
            .recv()
            .await
            .expect("changed event")
            .entity
            .expect("event entity")
            .row_version
    };
    let ticket = ManagedTicket::insert(db, create("t-1")).await?;
    assert_eq!(next_version().await, 0);
    ManagedTicket::update_by_id(db, &ticket.id, status("assigned")).await?;
    assert_eq!(next_version().await, 1);
    let response = graphql(db, update_mutation("t-1", "in_progress", Some(1))).await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(next_version().await, 2);
    ManagedTicket::upsert(
        db,
        CreateManagedTicketInput {
            status: "held".to_string(),
            ..create("t-1")
        },
    )
    .await?;
    assert_eq!(next_version().await, 3);
    // A hard delete carries the last committed version in its preimage.
    ManagedTicket::delete_by_id(db, &ticket.id).await?;
    assert_eq!(next_version().await, 3);
    Ok(())
}

async fn before_update_input_replacement_still_advances(db: &Database) -> TestResult {
    let mut db = db.clone();
    db.set_write_input_transform(ReplaceUpdateInput);
    ManagedTicket::insert(&db, create("t-1")).await?;
    let response = graphql(&db, update_mutation("t-1", "ignored", None)).await;
    assert!(response.errors.is_empty(), "{response:?}");
    let stored = load_ticket(&db, "t-1").await;
    assert_eq!(stored.status, "replaced");
    assert_eq!(stored.row_version, 1);
    Ok(())
}

async fn advancing_past_i64_max_is_refused_on_every_path(db: &Database) -> TestResult {
    let ticket = ManagedTicket::insert(db, create("t-1")).await?;
    set_ticket_version(db, "t-1", i64::MAX).await?;
    let id = ticket.id.clone();

    assert!(
        ManagedTicket::update_by_id(db, &id, status("assigned"))
            .await
            .is_err(),
        "update_by_id"
    );
    assert!(
        ManagedTicket::update_where(
            db,
            ManagedTicketWhereInput {
                status: Some(StringFilter {
                    eq: Some("open".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            status("assigned"),
        )
        .await
        .is_err(),
        "update_where"
    );
    let tx_id = id.clone();
    assert!(
        db.transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.update_by_id::<ManagedTicket>(&tx_id, status("assigned"))
                    .await?;
                Ok::<_, OrmPublicError>(())
            })
        })
        .await
        .is_err(),
        "transaction update_by_id"
    );
    assert!(
        ManagedTicket::upsert(
            db,
            CreateManagedTicketInput {
                status: "assigned".to_string(),
                ..create("t-1")
            },
        )
        .await
        .is_err(),
        "upsert update branch"
    );
    assert!(matches!(
        ManagedTicket::compare_and_swap(
            db,
            &id,
            i64::MAX,
            ManagedTicketWhereInput::default(),
            status("assigned"),
        )
        .await?,
        ConditionalUpdateOutcome::Conflict
    ));
    let update = resolver_name("updateManagedTicket");
    let response = graphql(db, update_mutation("t-1", "assigned", None)).await;
    assert!(!applied(&response, update), "GraphQL update: {response:?}");
    let upsert = resolver_name("upsertManagedTicket");
    let input = argument_name("input");
    let id_field = field_name("id");
    let reference_field = field_name("reference");
    let status_field = field_name("status");
    let response = graphql(
        db,
        format!(
            r#"mutation {{ {upsert}({input}: {{ {id_field}: "t-1", {reference_field}: "ref-t-1", {status_field}: "assigned" }}) {{ __typename }} }}"#
        ),
    )
    .await;
    assert!(!response.errors.is_empty(), "GraphQL upsert: {response:?}");
    let stored = load_ticket(db, "t-1").await;
    assert_eq!(
        (stored.row_version, stored.status.as_str()),
        (i64::MAX, "open")
    );

    let key = ManagedSlotKey {
        site: "north".into(),
        name: "a".into(),
    };
    ManagedSlot::insert(
        db,
        CreateManagedSlotInput {
            site: "north".into(),
            name: "a".into(),
            state: "free".into(),
        },
    )
    .await?;
    execute(
        db,
        &format!("UPDATE managed_slots SET revision = {}", i64::MAX),
    )
    .await?;
    let state = |value: &str| UpdateManagedSlotInput {
        state: Some(value.to_string()),
    };
    assert!(
        ManagedSlot::update_by_key(db, &key, state("held"))
            .await
            .is_err(),
        "update_by_key"
    );
    assert!(
        ManagedSlot::update_if(db, &key, ManagedSlotWhereInput::default(), state("held"))
            .await
            .is_err(),
        "update_if"
    );
    assert!(
        ManagedSlot::update_where_bounded(
            db,
            ManagedSlotWhereInput {
                site: Some(StringFilter {
                    eq: Some("north".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            state("held"),
            MutationLimit::new(10).expect("limit"),
        )
        .await
        .is_err(),
        "update_where_bounded"
    );
    let slot = ManagedSlot::query(db)
        .fetch_all()
        .await?
        .pop()
        .expect("slot exists");
    assert_eq!((slot.revision, slot.state.as_str()), (i64::MAX, "free"));
    Ok(())
}

async fn graphql_update_and_delete_honour_expected_version(db: &Database) -> TestResult {
    ManagedTicket::insert(db, create("t-1")).await?;
    let delete = resolver_name("deleteManagedTicket");
    let id_arg = argument_name("id");
    let expected = argument_name("expectedVersion");
    let success = field_name("success");

    let stale = graphql(db, update_mutation("t-1", "stale", Some(5))).await;
    assert_eq!(stale.errors.len(), 1, "{stale:?}");
    assert_eq!(error_code(&stale).as_deref(), Some("\"CONFLICT\""));
    let unchanged = load_ticket(db, "t-1").await;
    assert_eq!(
        (unchanged.row_version, unchanged.status.as_str()),
        (0, "open")
    );

    let current = graphql(db, update_mutation("t-1", "assigned", Some(0))).await;
    assert!(current.errors.is_empty(), "{current:?}");
    assert_eq!(stored_version(db, "t-1").await, 1);

    // Without ExpectedVersion the update applies and still advances.
    let unconditional = graphql(db, update_mutation("t-1", "in_progress", None)).await;
    assert!(unconditional.errors.is_empty(), "{unconditional:?}");
    assert_eq!(stored_version(db, "t-1").await, 2);

    let stale_delete = graphql(
        db,
        format!(r#"mutation {{ {delete}({id_arg}: "t-1", {expected}: 1) {{ {success} }} }}"#),
    )
    .await;
    assert_eq!(stale_delete.errors.len(), 1, "{stale_delete:?}");
    assert_eq!(error_code(&stale_delete).as_deref(), Some("\"CONFLICT\""));
    assert_eq!(stored_version(db, "t-1").await, 2);

    let deleted = graphql(
        db,
        format!(r#"mutation {{ {delete}({id_arg}: "t-1", {expected}: 2) {{ {success} }} }}"#),
    )
    .await;
    assert!(deleted.errors.is_empty(), "{deleted:?}");
    assert!(
        ManagedTicket::find_by_id(db, &"t-1".to_string())
            .await?
            .is_none()
    );
    Ok(())
}

/// Repository writes run serializably (`TransactionMode::StateMachine`), so a
/// racing writer may be refused with a retryable error and write nothing. Only
/// such refusals are retried; every committed write advances exactly once.
async fn concurrent_writers_lose_no_increments(db: &Database) -> TestResult {
    const WRITERS: usize = 8;
    const WRITES_EACH: usize = 5;
    const MAX_ATTEMPTS: usize = 200;
    ManagedTicket::insert(db, create("t-1")).await?;
    let writers = (0..WRITERS).map(|writer| {
        let db = db.clone();
        tokio::spawn(async move {
            let mut retries = 0_usize;
            for write in 0..WRITES_EACH {
                let mut attempts = 0;
                loop {
                    attempts += 1;
                    match ManagedTicket::update_by_id(
                        &db,
                        &"t-1".to_string(),
                        status(&format!("w{writer}-{write}")),
                    )
                    .await
                    {
                        Ok(updated) => {
                            updated.expect("ticket updated");
                            break;
                        }
                        Err(error)
                            if OrmPublicError::from_sqlx(&error).is_retryable()
                                && attempts < MAX_ATTEMPTS =>
                        {
                            retries += 1;
                            tokio::task::yield_now().await;
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            Ok::<_, graphql_orm::Error>(retries)
        })
    });
    let mut retries = 0;
    for writer in graphql_orm::futures::future::join_all(writers).await {
        retries += writer??;
    }
    println!(
        "concurrent writers: {} commits, {retries} retryable refusals",
        WRITERS * WRITES_EACH
    );
    assert_eq!(
        stored_version(db, "t-1").await,
        (WRITERS * WRITES_EACH) as i64
    );
    Ok(())
}

/// Racing writers with the same `ExpectedVersion`: exactly one commits. A loser
/// either fails with `CONFLICT`, or (on serializable backends) with a retryable
/// `SERVICE_UNAVAILABLE` that writes nothing; retrying it then yields `CONFLICT`.
async fn exactly_one_concurrent_writer_wins_under_expected_version(db: &Database) -> TestResult {
    const WRITERS: usize = 8;
    const MAX_ATTEMPTS: usize = 200;
    ManagedTicket::insert(db, create("t-1")).await?;
    ManagedTicket::update_by_id(db, &"t-1".to_string(), status("assigned")).await?;
    let schema = schema_builder(db.clone()).finish();
    let writers = (0..WRITERS).map(|writer| {
        let schema = schema.clone();
        tokio::spawn(async move {
            let mut retries = 0_usize;
            loop {
                let response = schema
                    .execute(Request::new(update_mutation(
                        "t-1",
                        &format!("w{writer}"),
                        Some(1),
                    )))
                    .await;
                if response.errors.is_empty() {
                    return (true, retries);
                }
                match error_code(&response).as_deref() {
                    Some("\"CONFLICT\"") => return (false, retries),
                    Some("\"SERVICE_UNAVAILABLE\"") if retries < MAX_ATTEMPTS => {
                        retries += 1;
                        tokio::task::yield_now().await;
                    }
                    _ => panic!("unexpected response: {response:?}"),
                }
            }
        })
    });
    let mut winners = 0;
    let mut conflicts = 0;
    let mut retries = 0;
    for outcome in graphql_orm::futures::future::join_all(writers).await {
        let (won, writer_retries) = outcome?;
        retries += writer_retries;
        if won {
            winners += 1;
        } else {
            conflicts += 1;
        }
    }
    println!(
        "expected-version race: {winners} winner, {conflicts} conflicts, {retries} retryable refusals"
    );
    assert_eq!((winners, conflicts), (1, WRITERS - 1));
    assert_eq!(stored_version(db, "t-1").await, 2);
    Ok(())
}

async fn cas_only_version_is_unchanged_by_ordinary_writes(db: &Database) -> TestResult {
    let job = CasOnlyJob::insert(
        db,
        CreateCasOnlyJobInput {
            id: "j-1".to_string(),
            status: "pending".to_string(),
        },
    )
    .await?;
    let updated = CasOnlyJob::update_by_id(
        db,
        &job.id,
        UpdateCasOnlyJobInput {
            status: Some("running".to_string()),
        },
    )
    .await?
    .expect("job updated");
    assert_eq!(updated.version, 0);

    // Only the managed entity's update and delete accept ExpectedVersion.
    let sdl = schema_builder(db.clone()).finish().sdl();
    let expected = argument_name("expectedVersion");
    for line in sdl.lines() {
        if line.contains("CasOnlyJob") {
            assert!(!line.contains(expected), "{line}");
        }
    }
    let managed = sdl
        .lines()
        .filter(|line| line.contains(expected))
        .collect::<Vec<_>>();
    assert_eq!(managed.len(), 2, "{managed:#?}");
    Ok(())
}

/// The SDL block of one object type.
fn object_block<'a>(sdl: &'a str, type_name: &str) -> &'a str {
    let start = sdl
        .find(&format!("type {type_name} "))
        .unwrap_or_else(|| panic!("type {type_name} in SDL"));
    let end = sdl[start..].find("\n}").expect("type block end") + start;
    &sdl[start..end]
}

async fn record_version_is_served_under_its_fixed_name(db: &Database) -> TestResult {
    use graphql_orm::graphql::orm::{GraphqlRecordVersionMode, RECORD_VERSION_FIELD};
    let row_version = field_name("rowVersion");

    // SDL: managed entities gain the fixed name beside their own field; the
    // CAS-only entity does not.
    let sdl = schema_builder(db.clone()).finish().sdl();
    let managed = object_block(&sdl, "ManagedTicket");
    assert!(managed.contains("\tRecordVersion: Int!"), "{managed}");
    assert!(
        managed.contains(&format!("\t{row_version}: Int!")),
        "{managed}"
    );
    let cas_only = object_block(&sdl, "CasOnlyJob");
    assert!(!cas_only.contains(RECORD_VERSION_FIELD), "{cas_only}");
    assert_eq!(ManagedTicket::managed_version_field(), Some(row_version));
    assert_eq!(CasOnlyJob::managed_version_field(), None);

    // Catalogue: the declared field and who advances it.
    let managed = ManagedTicket::graphql_semantic_metadata()
        .and_then(|entity| entity.record_version.as_ref())
        .expect("managed record version metadata");
    assert_eq!(managed.field, row_version);
    assert_eq!(managed.mode, GraphqlRecordVersionMode::Managed);
    let cas_only = CasOnlyJob::graphql_semantic_metadata()
        .and_then(|entity| entity.record_version.as_ref())
        .expect("CAS-only record version metadata");
    assert_eq!(cas_only.field, field_name("version"));
    assert_eq!(cas_only.mode, GraphqlRecordVersionMode::Cas);

    // Query: both names resolve the advanced value.
    ManagedTicket::insert(db, create("t-1")).await?;
    ManagedTicket::update_by_id(db, &"t-1".to_string(), status("assigned")).await?;
    let ticket = resolver_name("managedTicket");
    let id = argument_name("id");
    let response = graphql(
        db,
        format!(r#"{{ {ticket}({id}: "t-1") {{ RecordVersion {row_version} }} }}"#),
    )
    .await;
    assert!(response.errors.is_empty(), "{response:?}");
    let json = response.data.into_json()?;
    assert_eq!(json[ticket]["RecordVersion"], 1);
    assert_eq!(json[ticket][row_version], 1);

    // Subscription: a Changed payload may select the fixed name.
    let changed = resolver_name("managedTicketChanged");
    let entity = field_name("managedTicket");
    let mut stream = schema_builder(db.clone())
        .finish()
        .execute_stream(Request::new(format!(
            "subscription {{ {changed} {{ {entity} {{ RecordVersion }} }} }}"
        )));
    graphql_orm::futures::future::poll_fn(|cx| {
        use graphql_orm::futures::StreamExt;
        match stream.poll_next_unpin(cx) {
            std::task::Poll::Pending => std::task::Poll::Ready(()),
            std::task::Poll::Ready(other) => panic!("subscription was not admitted: {other:?}"),
        }
    })
    .await;
    ManagedTicket::update_by_id(db, &"t-1".to_string(), status("closed")).await?;
    let event = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        graphql_orm::futures::StreamExt::next(&mut stream),
    )
    .await?
    .expect("subscription event");
    assert!(event.errors.is_empty(), "{event:?}");
    assert_eq!(event.data.into_json()?[changed][entity]["RecordVersion"], 2);
    Ok(())
}

async fn compare_and_swap_advances_a_managed_version_exactly_once(db: &Database) -> TestResult {
    let ticket = ManagedTicket::insert(db, create("t-1")).await?;
    let outcome = ManagedTicket::compare_and_swap(
        db,
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
            db,
            &ticket.id,
            0,
            ManagedTicketWhereInput::default(),
            status("stale"),
        )
        .await?,
        ConditionalUpdateOutcome::Conflict
    ));
    assert_eq!(stored_version(db, "t-1").await, 1);
    Ok(())
}

async fn key_based_repository_writes_advance(db: &Database) -> TestResult {
    let key = ManagedSlotKey {
        site: "north".into(),
        name: "a".into(),
    };
    for name in ["a", "b"] {
        ManagedSlot::insert(
            db,
            CreateManagedSlotInput {
                site: "north".into(),
                name: name.into(),
                state: "free".into(),
            },
        )
        .await?;
    }
    let state = |value: &str| UpdateManagedSlotInput {
        state: Some(value.to_string()),
    };
    let by_key = ManagedSlot::update_by_key(db, &key, state("held"))
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
        ManagedSlot::update_if(db, &key, expected, state("booked")).await?
    else {
        panic!("matching predicate should update");
    };
    assert_eq!(conditional.revision, 2);

    let outcome = ManagedSlot::update_where_bounded(
        db,
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
    let revisions = ManagedSlot::query(db)
        .fetch_all()
        .await?
        .into_iter()
        .map(|slot| (slot.name, slot.revision))
        .collect::<Vec<_>>();
    assert!(revisions.contains(&("a".to_string(), 3)));
    assert!(revisions.contains(&("b".to_string(), 1)));
    Ok(())
}

macro_rules! scenarios {
    ($($scenario:ident),* $(,)?) => {
        #[cfg(feature = "sqlite")]
        mod sqlite_scenarios {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $scenario() -> super::TestResult {
                    let fixture = super::fixture().await?;
                    super::$scenario(&fixture.db).await?;
                    fixture.finish()
                }
            )*
        }

        #[cfg(not(feature = "sqlite"))]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        #[ignore = "starts a test-owned database container"]
        async fn every_scenario_on_an_owned_database() -> TestResult {
            let fixture = fixture().await?;
            $(
                reset(&fixture.db).await?;
                $scenario(&fixture.db)
                    .await
                    .map_err(|error| format!("{}: {error}", stringify!($scenario)))?;
                println!("managed version scenario passed: {}", stringify!($scenario));
            )*
            fixture.finish()
        }
    };
}

scenarios![
    insert_starts_at_default_and_repository_updates_advance,
    update_where_advances_every_matched_row_once,
    transaction_writes_advance_per_statement_and_rollback_keeps_the_old_version,
    upsert_update_branch_advances_and_insert_branch_starts_at_default,
    changed_events_carry_the_post_commit_version,
    before_update_input_replacement_still_advances,
    advancing_past_i64_max_is_refused_on_every_path,
    graphql_update_and_delete_honour_expected_version,
    concurrent_writers_lose_no_increments,
    exactly_one_concurrent_writer_wins_under_expected_version,
    cas_only_version_is_unchanged_by_ordinary_writes,
    compare_and_swap_advances_a_managed_version_exactly_once,
    record_version_is_served_under_its_fixed_name,
    key_based_repository_writes_advance,
];
