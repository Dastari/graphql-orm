// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! #851: a required `ExpectedVersion` and conditional update assurance.
//!
//! `update_expected_version = "required"` makes the generated update's
//! `ExpectedVersion` non-null. `update_assurance(select, levels)` selects the
//! assurance level per request from the typed patch and the locked row, inside
//! the write transaction and before any write.
//!
//! SQLite runs every scenario as its own test. PostgreSQL and SQL Server run
//! all scenarios in one ignored test against a single test-owned container,
//! resetting the tables between scenarios:
//!
//! ```sh
//! cargo test -p graphql-orm --no-default-features --features postgres --test update_assurance -- --ignored
//! cargo test -p graphql-orm --no-default-features --features mssql --test update_assurance -- --ignored
//! ```

use std::sync::{Arc, Mutex};

use graphql_orm::async_graphql::{Request, Variables};
use graphql_orm::graphql::assurance::{
    AssuranceActorClass, AssuranceCheck, AssuranceEnforcement, AssuranceRequirementEvaluator,
    AssuranceSchemaConfig, OperationAssuranceRegistry, UpdateAssuranceInput,
};
use graphql_orm::graphql::errors::OrmPublicError;
use graphql_orm::graphql::orm::{
    ChangeAction, GeneratedGraphqlOperationCategory, GraphqlOperationKind, MutationContext,
    MutationEvent, MutationHook, MutationPhase,
};
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

/// A versioned review record: every update is compare-and-swap, a rename
/// needs ordinary write assurance and a state transition needs elevated
/// assurance.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "review_records",
        plural = "ReviewRecords",
        auth = "none",
        mutation_shapes = "create,update,delete",
        update_expected_version = "required",
        update_assurance(select = "select_review_assurance", levels = "write,elevated")
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "review_records",
        plural = "ReviewRecords",
        auth = "none",
        mutation_shapes = "create,update,delete",
        update_expected_version = "required",
        update_assurance(select = "select_review_assurance", levels = "write,elevated")
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "review_records",
        plural = "ReviewRecords",
        auth = "none",
        mutation_shapes = "create,update,delete",
        update_expected_version = "required",
        update_assurance(select = "select_review_assurance", levels = "write,elevated")
    )
)]
struct ReviewRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    display_name: String,
    #[filterable(type = "string")]
    state: String,
    #[graphql_orm(version = "managed", default = "0")]
    #[filterable(type = "number")]
    row_version: i64,
}

/// A related row written in the same transaction as each review update.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "review_audits",
        plural = "ReviewAudits",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "review_audits",
        plural = "ReviewAudits",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "review_audits",
        plural = "ReviewAudits",
        auth = "none"
    )
)]
struct ReviewAudit {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    record_id: String,
    #[filterable(type = "string")]
    state: String,
}

/// Host text a failing selector returns; it must never reach a client.
const SELECTOR_SECRET: &str = "selector-private-detail";

/// The typed selector: a state transition needs `elevated`, anything else
/// `write`. A display name of `__undeclared__` returns a level the entity
/// does not declare, and `__selector_error__` fails with host text, to pin
/// the fail-closed checks.
fn select_review_assurance(
    input: UpdateAssuranceInput<'_, ReviewRecord, UpdateReviewRecordInput>,
) -> async_graphql::Result<&'static str> {
    match input.patch().display_name.as_deref() {
        Some("__undeclared__") => return Ok("undeclared"),
        // Message, source and extensions all carry host text.
        Some("__selector_error__") => {
            return Err(
                async_graphql::Error::new_with_source(std::io::Error::other(SELECTOR_SECRET))
                    .extend_with(|_, extensions| extensions.set("hostDetail", SELECTOR_SECRET)),
            );
        }
        Some("__selector_panic__") => panic!("{SELECTOR_SECRET}"),
        _ => {}
    }
    match &input.patch().state {
        Some(state) if *state != input.current().state => Ok("elevated"),
        _ => Ok("write"),
    }
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [ReviewRecord, ReviewAudit], }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [ReviewRecord, ReviewAudit], }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [ReviewRecord, ReviewAudit], }

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

/// The assurance a request presents, as request data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Granted {
    Write,
    Elevated,
}

/// Admits `write` for any granted level and `elevated` only for `Elevated`.
/// Records every policy it was asked to decide.
#[derive(Debug, Default)]
struct LevelEvaluator(Mutex<Vec<String>>);

impl AssuranceRequirementEvaluator for LevelEvaluator {
    fn enforce(
        &self,
        check: &AssuranceCheck<'_>,
        _actor_class: AssuranceActorClass,
        policy_id: &str,
    ) -> async_graphql::Result<()> {
        self.0.lock().unwrap().push(policy_id.to_owned());
        let required = match policy_id {
            "write" => Granted::Write,
            "elevated" => Granted::Elevated,
            _ => return Err(OrmPublicError::forbidden().into_graphql_error()),
        };
        match check.data_opt::<Granted>() {
            Some(granted) if *granted >= required => Ok(()),
            _ => Err(OrmPublicError::forbidden().into_graphql_error()),
        }
    }
}

/// Writes one audit row per committed review update, in the update's
/// transaction. A display name of `fail-invariant` fails after that write.
#[derive(Clone, Default)]
struct AuditHook(Arc<Mutex<Vec<MutationPhase>>>);

impl MutationHook for AuditHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.entity_name != "ReviewRecord" || event.action != ChangeAction::Updated {
                return Ok(());
            }
            self.0.lock().unwrap().push(event.phase.clone());
            if event.phase != MutationPhase::After {
                return Ok(());
            }
            let after = event
                .after::<ReviewRecord>()?
                .ok_or_else(|| async_graphql::Error::new("missing updated review"))?;
            hook_ctx
                .insert::<ReviewAudit>(CreateReviewAuditInput {
                    id: format!("{}-{}", after.id, after.row_version),
                    record_id: after.id.clone(),
                    state: after.state.clone(),
                })
                .await
                .map_err(|error| async_graphql::Error::new(error.to_string()))?;
            if after.display_name == "fail-invariant" {
                return Err(async_graphql::Error::new("review invariant failed"));
            }
            Ok(())
        })
    }
}

/// Runs inside the update transaction before the selector. It reads through
/// the transaction (no public write API exists at this point) and, for a
/// display name of `__inject_state__`, turns a rename into a state
/// transition, which the selector must see.
#[derive(Clone, Default)]
struct ReviewTransform(Arc<Mutex<usize>>);

impl graphql_orm::graphql::orm::WriteInputTransform for ReviewTransform {
    fn before_update_with_context<'a>(
        &'a self,
        write_ctx: &'a mut graphql_orm::graphql::orm::WriteInputContext<'_, '_>,
        _existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if write_ctx.entity_name() != "ReviewRecord" {
                return Ok(());
            }
            // An in-transaction read: the transform runs inside the write.
            write_ctx
                .query::<ReviewAudit>()
                .fetch_all()
                .await
                .map_err(|error| async_graphql::Error::new(error.to_string()))?;
            *self.0.lock().unwrap() += 1;
            let input = input
                .downcast_mut::<UpdateReviewRecordInput>()
                .ok_or_else(|| async_graphql::Error::new("unexpected update input"))?;
            if input.display_name.as_deref() == Some("__inject_state__") {
                input.state = Some("approved".to_string());
            }
            Ok(())
        })
    }
}

#[cfg(not(feature = "sqlite"))]
const TABLES: [&str; 2] = ["review_audits", "review_records"];

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
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    // The pool keeps the owned container alive; it is removed when the last clone drops.
    #[cfg(feature = "postgres")]
    let db = Database::new(owned_postgres::OwnedPostgres::pool("update-assurance", 12).await?);
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "update-assurance-init",
                "update assurance test",
                &[ReviewRecord::metadata(), ReviewAudit::metadata()],
            )
            .await?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    #[cfg(feature = "mssql")]
    let (db, owner) = {
        let owner = owned_mssql::OwnedSqlServer::start().await?;
        owned_mssql::execute_batch(
            &owner.connection_string,
            "CREATE TABLE review_records (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             display_name NVARCHAR(64) NOT NULL, state NVARCHAR(64) NOT NULL, \
             row_version BIGINT NOT NULL DEFAULT 0); \
             CREATE TABLE review_audits (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             record_id NVARCHAR(64) NOT NULL, state NVARCHAR(64) NOT NULL);",
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
    Ok(Fixture {
        db,
        #[cfg(feature = "mssql")]
        owner,
    })
}

#[cfg(not(feature = "sqlite"))]
async fn reset(db: &Database) -> graphql_orm::Result<()> {
    for table in TABLES {
        <DefaultBackend as WriteBackend>::execute_with_binds(
            db.pool(),
            &format!("DELETE FROM {table}"),
            &[],
        )
        .await?;
    }
    Ok(())
}

/// One test's database view: the audit hook, the entity policy and the
/// recording evaluator installed.
struct Harness {
    db: Database,
    phases: Arc<Mutex<Vec<MutationPhase>>>,
    transforms: Arc<Mutex<usize>>,
    evaluator: Arc<LevelEvaluator>,
}

fn harness(db: &Database) -> Harness {
    let mut db = db.clone();
    let hook = AuditHook::default();
    let phases = hook.0.clone();
    db.set_mutation_hook(hook);
    let transform = ReviewTransform::default();
    let transforms = transform.0.clone();
    db.set_write_input_transform(transform);
    db.set_entity_policy(AllowEntityPolicy);
    Harness {
        db,
        phases,
        transforms,
        evaluator: Arc::new(LevelEvaluator::default()),
    }
}

fn update_field() -> String {
    resolver_name("updateReviewRecord").to_string()
}

impl Harness {
    /// The schema with assurance enforcement: `write` is the update's
    /// registry requirement, checked before the row is read.
    fn schema(&self) -> AppSchema {
        let mut registry = OperationAssuranceRegistry::builder(
            graphql_orm_operation_catalog(),
            AssuranceSchemaConfig::legacy(),
        );
        registry
            .require(GraphqlOperationKind::Mutation, &update_field(), "write")
            .expect("update is registered");
        let enforcement = AssuranceEnforcement::new(
            Arc::new(registry.build().expect("registry")),
            self.evaluator.clone(),
        );
        enforcement
            .finish_schema(schema_builder(self.db.clone()))
            .expect("schema")
    }

    async fn run(&self, query: String, granted: Granted) -> async_graphql::Response {
        self.schema()
            .execute(Request::new(query).data(granted))
            .await
    }

    fn policies(&self) -> Vec<String> {
        std::mem::take(&mut *self.evaluator.0.lock().unwrap())
    }

    fn phases(&self) -> Vec<MutationPhase> {
        std::mem::take(&mut *self.phases.lock().unwrap())
    }

    fn transforms(&self) -> usize {
        std::mem::take(&mut *self.transforms.lock().unwrap())
    }
}

fn create(id: &str) -> CreateReviewRecordInput {
    CreateReviewRecordInput {
        id: id.to_string(),
        display_name: "Draft".to_string(),
        state: "open".to_string(),
    }
}

async fn load(db: &Database, id: &str) -> ReviewRecord {
    ReviewRecord::find_by_id(db, &id.to_string())
        .await
        .expect("read review")
        .expect("review exists")
}

async fn audit_count(db: &Database) -> usize {
    ReviewAudit::query(db.pool())
        .fetch_all()
        .await
        .expect("read audits")
        .len()
}

fn error_code(response: &async_graphql::Response) -> Option<String> {
    response
        .errors
        .first()
        .and_then(|error| error.extensions.as_ref())
        .and_then(|extensions| extensions.get("code"))
        .map(|code| code.to_string().trim_matches('"').to_owned())
}

/// `expected` is spliced in verbatim: `Some("3")`, `Some("null")` or `None`
/// to omit the argument.
fn update(id: &str, patch: &str, expected: Option<&str>) -> String {
    let update = update_field();
    let id_arg = argument_name("id");
    let input = argument_name("input");
    let expected_arg = argument_name("expectedVersion");
    let success = field_name("success");
    let expected = expected
        .map(|version| format!(", {expected_arg}: {version}"))
        .unwrap_or_default();
    format!(
        r#"mutation {{ {update}({id_arg}: "{id}", {input}: {{ {patch} }}{expected}) {{ {success} }} }}"#
    )
}

fn rename(name: &str) -> String {
    format!(r#"{}: "{name}""#, field_name("displayName"))
}

fn transition(state: &str) -> String {
    format!(r#"{}: "{state}""#, field_name("state"))
}

fn applied(response: &async_graphql::Response) -> bool {
    response.errors.is_empty()
        && response
            .data
            .clone()
            .into_json()
            .ok()
            .is_some_and(|data| data[update_field()][field_name("success")] == true)
}

/// The SDL argument block of one root field: from `field(` to its `): `.
fn root_arguments<'a>(sdl: &'a str, field: &str) -> Option<&'a str> {
    let start = sdl
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("{field}(")))
        .map(|line| line.as_ptr() as usize - sdl.as_ptr() as usize)?;
    let rest = &sdl[start..];
    let end = rest.find("): ")?;
    Some(&rest[..end])
}

async fn sdl_and_catalogue_require_expected_version_and_list_conditional_levels(
    db: &Database,
) -> TestResult {
    let sdl = harness(db).schema().sdl();
    let update = update_field();
    let expected = argument_name("expectedVersion");
    let update_arguments = root_arguments(&sdl, &update).ok_or("update root in SDL")?;
    assert!(
        update_arguments.contains(&format!("{expected}: Int!")),
        "{update_arguments}"
    );
    // Delete keeps its optional argument.
    let delete = resolver_name("deleteReviewRecord");
    let delete_arguments = root_arguments(&sdl, delete).ok_or("delete root in SDL")?;
    assert!(
        delete_arguments.contains(&format!("{expected}: Int"))
            && !delete_arguments.contains(&format!("{expected}: Int!")),
        "{delete_arguments}"
    );
    // No root rewrites an existing row without the per-row checks.
    assert!(!sdl.contains(resolver_name("updateReviewRecords")));
    assert!(!sdl.contains(resolver_name("upsertReviewRecord")));

    let catalog = graphql_orm_operation_catalog();
    let operation = catalog
        .operations()
        .iter()
        .find(|operation| operation.field_name() == update)
        .ok_or("update in catalogue")?;
    assert_eq!(
        operation.category(),
        GeneratedGraphqlOperationCategory::Update
    );
    let argument = operation
        .arguments()
        .iter()
        .find(|argument| argument.graphql_name() == expected)
        .ok_or("ExpectedVersion in catalogue")?;
    assert_eq!(
        (argument.graphql_type(), argument.rust_type()),
        ("Int!", "i64")
    );
    let selection = operation
        .conditional_assurance()
        .ok_or("conditional assurance in catalogue")?;
    assert!(selection.is_conditional());
    assert_eq!(selection.levels(), ["elevated", "write"]);
    for operation in catalog.operations() {
        if operation.field_name() != update {
            assert_eq!(operation.conditional_assurance(), None, "{operation:?}");
        }
    }
    Ok(())
}

async fn omitted_null_and_stale_expected_version_refuse_the_write(db: &Database) -> TestResult {
    let harness = harness(db);
    ReviewRecord::insert(db, create("r-1")).await?;
    let mut events = harness
        .db
        .ensure_event_sender::<ReviewRecordChangedEvent>()
        .subscribe();
    for expected in [None, Some("null")] {
        let response = harness
            .run(
                update("r-1", &rename("Renamed"), expected),
                Granted::Elevated,
            )
            .await;
        assert!(!response.errors.is_empty(), "{expected:?}: {response:?}");
        assert!(!applied(&response));
    }
    // A null value for a non-null variable is refused as well.
    let root = update_field();
    let query = format!(
        r#"mutation($v: Int!) {{ {root}({}: "r-1", {}: {{ {} }}, {}: $v) {{ {} }} }}"#,
        argument_name("id"),
        argument_name("input"),
        rename("Renamed"),
        argument_name("expectedVersion"),
        field_name("success"),
    );
    let response = harness
        .schema()
        .execute(
            Request::new(query)
                .variables(Variables::from_json(serde_json::json!({ "v": null })))
                .data(Granted::Elevated),
        )
        .await;
    assert!(!response.errors.is_empty(), "{response:?}");
    // The omitted and null forms never reach the resolver.
    assert!(harness.policies().is_empty());
    assert_eq!(harness.transforms(), 0);

    let stale = harness
        .run(
            update("r-1", &rename("Renamed"), Some("7")),
            Granted::Elevated,
        )
        .await;
    assert_eq!(error_code(&stale).as_deref(), Some("CONFLICT"), "{stale:?}");

    let stored = load(db, "r-1").await;
    assert_eq!(
        (stored.display_name.as_str(), stored.row_version),
        ("Draft", 0)
    );
    assert_eq!(audit_count(db).await, 0);
    assert!(events.try_recv().is_err(), "no Changed event is committed");
    assert!(harness.phases().is_empty());
    Ok(())
}

async fn a_matching_version_advances_exactly_once(db: &Database) -> TestResult {
    let harness = harness(db);
    ReviewRecord::insert(db, create("r-1")).await?;
    let first = harness
        .run(update("r-1", &rename("First"), Some("0")), Granted::Write)
        .await;
    assert!(applied(&first), "{first:?}");
    assert_eq!(load(db, "r-1").await.row_version, 1);
    // Replaying the same version is now stale.
    let replay = harness
        .run(update("r-1", &rename("Replay"), Some("0")), Granted::Write)
        .await;
    assert_eq!(
        error_code(&replay).as_deref(),
        Some("CONFLICT"),
        "{replay:?}"
    );
    let stored = load(db, "r-1").await;
    assert_eq!(
        (stored.display_name.as_str(), stored.row_version),
        ("First", 1)
    );
    let second = harness
        .run(update("r-1", &rename("Second"), Some("1")), Granted::Write)
        .await;
    assert!(applied(&second), "{second:?}");
    assert_eq!(load(db, "r-1").await.row_version, 2);
    Ok(())
}

async fn a_rename_runs_at_ordinary_assurance(db: &Database) -> TestResult {
    let harness = harness(db);
    ReviewRecord::insert(db, create("r-1")).await?;
    let mut events = harness
        .db
        .ensure_event_sender::<ReviewRecordChangedEvent>()
        .subscribe();
    let response = harness
        .run(update("r-1", &rename("Renamed"), Some("0")), Granted::Write)
        .await;
    assert!(applied(&response), "{response:?}");
    // The registry requirement before the read, then the selected level.
    assert_eq!(harness.policies(), ["write", "write"]);
    // Setting the state to its current value is not a transition.
    let same_state = harness
        .run(
            update(
                "r-1",
                &format!("{}, {}", rename("Again"), transition("open")),
                Some("1"),
            ),
            Granted::Write,
        )
        .await;
    assert!(applied(&same_state), "{same_state:?}");
    assert_eq!(harness.policies(), ["write", "write"]);
    let stored = load(db, "r-1").await;
    assert_eq!(
        (stored.display_name.as_str(), stored.row_version),
        ("Again", 2)
    );
    assert_eq!(audit_count(db).await, 2);
    assert!(events.try_recv().is_ok());
    assert!(events.try_recv().is_ok());
    Ok(())
}

async fn a_state_transition_requires_elevated_assurance(db: &Database) -> TestResult {
    let harness = harness(db);
    ReviewRecord::insert(db, create("r-1")).await?;
    let mut events = harness
        .db
        .ensure_event_sender::<ReviewRecordChangedEvent>()
        .subscribe();
    let refused = harness
        .run(
            update(
                "r-1",
                &format!("{}, {}", rename("Approved"), transition("approved")),
                Some("0"),
            ),
            Granted::Write,
        )
        .await;
    assert_eq!(
        error_code(&refused).as_deref(),
        Some("FORBIDDEN"),
        "{refused:?}"
    );
    assert_eq!(harness.policies(), ["write", "elevated"]);
    // The transform ran inside the transaction; the selection then ran
    // before the Before hook and every write.
    assert_eq!(harness.transforms(), 1);
    assert!(harness.phases().is_empty());
    let stored = load(db, "r-1").await;
    assert_eq!(
        (
            stored.display_name.as_str(),
            stored.state.as_str(),
            stored.row_version
        ),
        ("Draft", "open", 0)
    );
    assert_eq!(audit_count(db).await, 0);
    assert!(events.try_recv().is_err(), "no Changed event is committed");

    // The selector sees the patch the write applies: a rename the transform
    // turns into a transition needs elevated assurance too.
    let injected = harness
        .run(
            update("r-1", &rename("__inject_state__"), Some("0")),
            Granted::Write,
        )
        .await;
    assert_eq!(
        error_code(&injected).as_deref(),
        Some("FORBIDDEN"),
        "{injected:?}"
    );
    assert_eq!(harness.policies(), ["write", "elevated"]);
    assert_eq!(harness.transforms(), 1);
    assert!(harness.phases().is_empty());
    let stored = load(db, "r-1").await;
    assert_eq!(
        (
            stored.display_name.as_str(),
            stored.state.as_str(),
            stored.row_version
        ),
        ("Draft", "open", 0)
    );
    assert_eq!(audit_count(db).await, 0);
    assert!(events.try_recv().is_err(), "no Changed event is committed");

    let elevated = harness
        .run(
            update("r-1", &transition("approved"), Some("0")),
            Granted::Elevated,
        )
        .await;
    assert!(applied(&elevated), "{elevated:?}");
    assert_eq!(harness.policies(), ["write", "elevated"]);
    assert_eq!(
        harness.phases(),
        [MutationPhase::Before, MutationPhase::After]
    );
    let stored = load(db, "r-1").await;
    assert_eq!((stored.state.as_str(), stored.row_version), ("approved", 1));
    assert_eq!(audit_count(db).await, 1);
    assert!(events.try_recv().is_ok());
    Ok(())
}

async fn a_failed_invariant_rolls_back_the_transition_and_its_related_writes(
    db: &Database,
) -> TestResult {
    let harness = harness(db);
    ReviewRecord::insert(db, create("r-1")).await?;
    let mut events = harness
        .db
        .ensure_event_sender::<ReviewRecordChangedEvent>()
        .subscribe();
    // The update and the audit row are written, then the invariant fails.
    let response = harness
        .run(
            update(
                "r-1",
                &format!("{}, {}", rename("fail-invariant"), transition("approved")),
                Some("0"),
            ),
            Granted::Elevated,
        )
        .await;
    assert!(!applied(&response), "{response:?}");
    assert_eq!(
        harness.phases(),
        [MutationPhase::Before, MutationPhase::After]
    );
    let stored = load(db, "r-1").await;
    assert_eq!(
        (
            stored.display_name.as_str(),
            stored.state.as_str(),
            stored.row_version
        ),
        ("Draft", "open", 0)
    );
    assert_eq!(audit_count(db).await, 0);
    assert!(events.try_recv().is_err(), "no Changed event is committed");
    Ok(())
}

async fn conditional_assurance_fails_closed_when_misconfigured(db: &Database) -> TestResult {
    let harness = harness(db);
    ReviewRecord::insert(db, create("r-1")).await?;
    // No AssuranceEnforcement installed: even a rename is refused.
    let unenforced = schema_builder(harness.db.clone())
        .finish()
        .execute(Request::new(update("r-1", &rename("Renamed"), Some("0"))).data(Granted::Elevated))
        .await;
    assert_eq!(
        error_code(&unenforced).as_deref(),
        Some("AUTHORIZATION_MISCONFIGURED"),
        "{unenforced:?}"
    );
    // The check precedes the selector: a selector that would fail is never
    // called, so the refusal stays AUTHORIZATION_MISCONFIGURED.
    let unenforced = schema_builder(harness.db.clone())
        .finish()
        .execute(
            Request::new(update("r-1", &rename("__selector_error__"), Some("0")))
                .data(Granted::Elevated),
        )
        .await;
    assert_eq!(
        error_code(&unenforced).as_deref(),
        Some("AUTHORIZATION_MISCONFIGURED"),
        "{unenforced:?}"
    );
    // A selector returning an undeclared level is refused before evaluation.
    let undeclared = harness
        .run(
            update("r-1", &rename("__undeclared__"), Some("0")),
            Granted::Elevated,
        )
        .await;
    assert_eq!(
        error_code(&undeclared).as_deref(),
        Some("AUTHORIZATION_MISCONFIGURED"),
        "{undeclared:?}"
    );
    assert_eq!(harness.policies(), ["write"]);
    let stored = load(db, "r-1").await;
    assert_eq!(
        (stored.display_name.as_str(), stored.row_version),
        ("Draft", 0)
    );
    assert_eq!(audit_count(db).await, 0);
    Ok(())
}

async fn a_selector_error_or_panic_is_sanitized(db: &Database) -> TestResult {
    let harness = harness(db);
    ReviewRecord::insert(db, create("r-1")).await?;
    for trigger in ["__selector_error__", "__selector_panic__"] {
        let response = harness
            .run(update("r-1", &rename(trigger), Some("0")), Granted::Elevated)
            .await;
        assert_eq!(
            error_code(&response).as_deref(),
            Some("INTERNAL_ERROR"),
            "{trigger}: {response:?}"
        );
        let json = serde_json::to_string(&response.errors)?;
        assert!(!json.contains(SELECTOR_SECRET), "{trigger}: {json}");
        assert!(!json.contains("hostDetail"), "{trigger}: {json}");
        // Only the registry requirement was evaluated; nothing was written.
        assert_eq!(harness.policies(), ["write"], "{trigger}");
        assert!(harness.phases().is_empty(), "{trigger}");
        let stored = load(db, "r-1").await;
        assert_eq!(
            (stored.display_name.as_str(), stored.row_version),
            ("Draft", 0),
            "{trigger}"
        );
    }
    // The transaction rolled back cleanly: the next update applies.
    let renamed = harness
        .run(update("r-1", &rename("Renamed"), Some("0")), Granted::Write)
        .await;
    assert!(applied(&renamed), "{renamed:?}");
    assert_eq!(load(db, "r-1").await.row_version, 1);
    Ok(())
}

async fn a_row_that_cannot_advance_is_refused_before_selection(db: &Database) -> TestResult {
    let harness = harness(db);
    ReviewRecord::insert(db, create("r-1")).await?;
    <DefaultBackend as WriteBackend>::execute_with_binds(
        db.pool(),
        &format!(
            "UPDATE review_records SET row_version = {} WHERE id = 'r-1'",
            i64::MAX
        ),
        &[],
    )
    .await?;
    let response = harness
        .run(
            update("r-1", &rename("Renamed"), Some(&i64::MAX.to_string())),
            Granted::Elevated,
        )
        .await;
    assert!(!applied(&response), "{response:?}");
    assert!(!response.errors.is_empty(), "{response:?}");
    // The registry requirement ran; the selector and its level did not.
    assert_eq!(harness.policies(), ["write"]);
    assert!(harness.phases().is_empty());
    let stored = load(db, "r-1").await;
    assert_eq!(
        (stored.display_name.as_str(), stored.row_version),
        ("Draft", i64::MAX)
    );
    Ok(())
}

async fn runtime_ir_conversion_refuses_guarded_entities(_db: &Database) -> TestResult {
    use graphql_orm::graphql::orm::RuntimeSchema;
    const MESSAGE: &str = "update_expected_version and update_assurance are not represented";
    let refused =
        |result: Result<RuntimeSchema, graphql_orm::graphql::orm::RuntimeSchemaDiagnostics>| {
            result.err().is_some_and(|diagnostics| {
                diagnostics
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(MESSAGE))
            })
        };
    assert!(refused(RuntimeSchema::from_static_entities(&[
        ReviewRecord::metadata()
    ])));
    assert!(refused(RuntimeSchema::from_static_entities_for_repository(
        &[ReviewRecord::metadata()]
    )));
    // A copy of the metadata names the same table and is refused too.
    let mut copy = ReviewRecord::metadata().clone();
    copy.entity_name = "ReviewRecordCopy";
    assert!(refused(RuntimeSchema::from_static_entities(&[&copy])));
    // An unguarded entity is not refused for this reason.
    assert!(!refused(RuntimeSchema::from_static_entities(&[
        ReviewAudit::metadata()
    ])));
    Ok(())
}

async fn repository_updates_are_host_writes_and_unaffected(db: &Database) -> TestResult {
    ReviewRecord::insert(db, create("r-1")).await?;
    let updated = ReviewRecord::update_by_id(
        db,
        &"r-1".to_string(),
        UpdateReviewRecordInput {
            state: Some("approved".to_string()),
            ..Default::default()
        },
    )
    .await?
    .expect("review updated");
    assert_eq!(
        (updated.state.as_str(), updated.row_version),
        ("approved", 1)
    );
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
                println!("update assurance scenario passed: {}", stringify!($scenario));
            )*
            fixture.finish()
        }
    };
}

scenarios![
    sdl_and_catalogue_require_expected_version_and_list_conditional_levels,
    omitted_null_and_stale_expected_version_refuse_the_write,
    a_matching_version_advances_exactly_once,
    a_rename_runs_at_ordinary_assurance,
    a_state_transition_requires_elevated_assurance,
    a_failed_invariant_rolls_back_the_transition_and_its_related_writes,
    conditional_assurance_fails_closed_when_misconfigured,
    a_selector_error_or_panic_is_sanitized,
    a_row_that_cannot_advance_is_refused_before_selection,
    runtime_ir_conversion_refuses_guarded_entities,
    repository_updates_are_host_writes_and_unaffected,
];
