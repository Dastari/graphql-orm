// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Generated upserts with a principal-bound private conflict key, a typed
//! transient input and a hook acknowledgement (#908).
//!
//! One current `StatusSnapshot` belongs to each authenticated machine. The
//! conflict key `machine_key` is private and bound from the request's
//! verified subject, so no client names it. Each upsert carries a typed
//! `proof` that the input transform and the mutation hook read inside the
//! write transaction: the hook refuses a stale or mismatched snapshot against
//! the locked existing row, and acknowledges an idempotent retry without
//! writing.
//!
//! SQLite runs directly. PostgreSQL and SQL Server use a test-owned container:
//!
//! ```sh
//! cargo test -p graphql-orm --no-default-features --features postgres --test principal_bound_upserts -- --ignored
//! cargo test -p graphql-orm --no-default-features --features mssql --test principal_bound_upserts -- --ignored
//! ```

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use graphql_orm::async_graphql::{ErrorExtensions, InputObject};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use serde_json::{Value, json};

#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{argument_name as a, field_name as f, resolver_name as r};

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "mssql")]
type Backend = MssqlBackend;

/// The request's verified machine identity, as the conflict key.
pub struct MachineIdentity;

impl PrincipalKeyBinding for MachineIdentity {
    type Key = String;

    fn bind(ctx: &async_graphql::Context<'_>) -> async_graphql::Result<String> {
        if let Some(probe) = ctx.data_opt::<Probe>() {
            probe.bindings.fetch_add(1, Ordering::SeqCst);
        }
        if ctx.data_opt::<NoMachineIdentity>().is_some() {
            return Err(OrmPublicError::with_message(
                OrmErrorCode::Forbidden,
                "this request has no machine identity",
            )
            .into_graphql_error());
        }
        Ok(ctx.auth_subject()?.id)
    }
}

/// Request data: the verified context carries no machine identity.
struct NoMachineIdentity;
/// Request data: the input transform tries to replace the bound key.
struct HostileTransform;

/// Ordering proof sent with every snapshot. Never stored or published.
#[derive(InputObject, Clone)]
pub struct SnapshotProof {
    pub sequence: i64,
    pub digest: String,
    pub nonce: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "status_snapshots",
        plural = "StatusSnapshots",
        read_policy = "snapshot.read",
        write_policy = "snapshot.write",
        upsert = "machine_key",
        upsert_bound_key(field = "machine_key", binding = "MachineIdentity"),
        transient_input(type = "SnapshotProof", on = "upsert", name = "proof"),
        upsert_acknowledge = true,
        mutation_shapes = "upsert",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "status_snapshots",
        plural = "StatusSnapshots",
        read_policy = "snapshot.read",
        write_policy = "snapshot.write",
        upsert = "machine_key",
        upsert_bound_key(field = "machine_key", binding = "MachineIdentity"),
        transient_input(type = "SnapshotProof", on = "upsert", name = "proof"),
        upsert_acknowledge = true,
        mutation_shapes = "upsert",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "status_snapshots",
        plural = "StatusSnapshots",
        read_policy = "snapshot.read",
        write_policy = "snapshot.write",
        upsert = "machine_key",
        upsert_bound_key(field = "machine_key", binding = "MachineIdentity"),
        transient_input(type = "SnapshotProof", on = "upsert", name = "proof"),
        upsert_acknowledge = true,
        mutation_shapes = "upsert",
        default_sort = "id ASC"
    )
)]
#[graphql_orm(operation_authorization(categories = ["upsert"], all_scopes = ["snapshots.write"]))]
pub struct StatusSnapshot {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[graphql_orm(private)]
    #[unique]
    pub machine_key: String,
    /// Set by the input transform from the proof, never by the client.
    #[graphql_orm(skip_input)]
    pub sequence: i64,
    #[graphql_orm(skip_input)]
    pub digest: String,
    #[filterable(type = "string")]
    pub payload: String,
}

/// A second entity without a transient input: the argument is optional there
/// and the acknowledgement is not armed.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "machine_notes",
        plural = "MachineNotes",
        upsert = "machine_key",
        upsert_bound_key(field = "machine_key", binding = "MachineIdentity"),
        transient_input(
            type = "SnapshotProof",
            on = "upsert",
            name = "proof",
            required = false
        ),
        mutation_shapes = "upsert,update",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "machine_notes",
        plural = "MachineNotes",
        upsert = "machine_key",
        upsert_bound_key(field = "machine_key", binding = "MachineIdentity"),
        transient_input(
            type = "SnapshotProof",
            on = "upsert",
            name = "proof",
            required = false
        ),
        mutation_shapes = "upsert,update",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "machine_notes",
        plural = "MachineNotes",
        upsert = "machine_key",
        upsert_bound_key(field = "machine_key", binding = "MachineIdentity"),
        transient_input(
            type = "SnapshotProof",
            on = "upsert",
            name = "proof",
            required = false
        ),
        mutation_shapes = "upsert,update",
        default_sort = "id ASC"
    )
)]
pub struct MachineNote {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[graphql_orm(private)]
    #[unique]
    pub machine_key: String,
    pub note: String,
}

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [StatusSnapshot, MachineNote],
    client_metadata: true,
}
#[cfg(feature = "postgres")]
schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [StatusSnapshot, MachineNote],
    client_metadata: true,
}
#[cfg(feature = "mssql")]
schema_roots! {
    backend: "mssql",
    schema_policy: "external_writable",
    query_custom_ops: [],
    entities: [StatusSnapshot, MachineNote],
    client_metadata: true,
}

/// Counts every callback a request reaches, and records what hooks saw.
#[derive(Clone, Default)]
struct Probe {
    bindings: Arc<AtomicUsize>,
    entity_checks: Arc<AtomicUsize>,
    row_checks: Arc<AtomicUsize>,
    transforms: Arc<AtomicUsize>,
    before_hooks: Arc<AtomicUsize>,
    after_hooks: Arc<AtomicUsize>,
    /// Everything an `After` hook could publish, rendered as text.
    published: Arc<Mutex<Vec<String>>>,
    /// The proof nonce each hook phase read.
    nonces: Arc<Mutex<Vec<String>>>,
}

impl Probe {
    fn counts(&self) -> [usize; 6] {
        [
            self.bindings.load(Ordering::SeqCst),
            self.entity_checks.load(Ordering::SeqCst),
            self.row_checks.load(Ordering::SeqCst),
            self.transforms.load(Ordering::SeqCst),
            self.before_hooks.load(Ordering::SeqCst),
            self.after_hooks.load(Ordering::SeqCst),
        ]
    }
}

fn subject(ctx: Option<&async_graphql::Context<'_>>) -> String {
    ctx.and_then(|ctx| ctx.auth_subject_opt())
        .map(|subject| subject.id)
        .unwrap_or_default()
}

struct Entities(Probe);

impl EntityPolicy<Backend> for Entities {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        kind: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        if ctx.is_some() {
            self.0.entity_checks.fetch_add(1, Ordering::SeqCst);
        }
        let allowed = !(kind == EntityAccessKind::Write && subject(ctx) == "no-entity");
        Box::pin(async move { Ok(allowed) })
    }
}

/// Every row is readable. No GraphQL request writes a frozen snapshot.
struct Rows(Probe);

impl RowPolicy<Backend> for Rows {
    fn read_visibility_capability(
        &self,
        _: &'static str,
        _: Option<&'static str>,
    ) -> ReadVisibilityCapability {
        ReadVisibilityCapability::Complete
    }

    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async { Ok(ReadVisibility::Unrestricted) })
    }

    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        if ctx.is_some() {
            self.0.row_checks.fetch_add(1, Ordering::SeqCst);
        }
        let frozen = ctx.is_some()
            && row
                .downcast_ref::<StatusSnapshot>()
                .is_some_and(|row| row.payload == "frozen");
        Box::pin(async move { Ok(!frozen) })
    }
}

/// `no-payload` may not write a snapshot's payload.
struct Fields;

impl FieldPolicy<Backend> for Fields {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database<Backend>,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        let allowed =
            !(field.eq_ignore_ascii_case("payload") && subject(Some(ctx)) == "no-payload");
        Box::pin(async move { Ok(allowed) })
    }
}

/// Copies the proof's ordering fields into the row. A GraphQL snapshot
/// without a proof is refused.
struct ProofTransform(Probe);

impl WriteInputTransform<Backend> for ProofTransform {
    fn before_upsert_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_, Backend>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if write_ctx.origin() != WriteOrigin::GraphqlMutation {
                return Ok(());
            }
            self.0.transforms.fetch_add(1, Ordering::SeqCst);
            let hostile = write_ctx
                .graphql_ctx()
                .is_some_and(|ctx| ctx.data_opt::<HostileTransform>().is_some());
            if let Some(note) = input.downcast_mut::<CreateMachineNoteInput>() {
                if hostile {
                    note.machine_key = "hijacked".to_owned();
                }
                return Ok(());
            }
            let Some(snapshot) = input.downcast_mut::<CreateStatusSnapshotInput>() else {
                return Ok(());
            };
            let proof = write_ctx
                .transient_input::<SnapshotProof>()
                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InvalidInput))?;
            snapshot.sequence = proof.sequence;
            snapshot.digest = proof.digest.clone();
            if hostile {
                snapshot.machine_key = "hijacked".to_owned();
            }
            Ok(())
        })
    }
}

/// Validates the proof against the locked existing row.
struct ProofHook(Probe);

impl MutationHook<Backend> for ProofHook {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_, Backend>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if ctx.is_none() {
                return Ok(());
            }
            if event.entity_name == "MachineNote" {
                // No acknowledgement is armed for this entity.
                if event.phase == MutationPhase::Before && event.before_state.is_some() {
                    assert!(!hook_ctx.can_acknowledge_without_write());
                    if hook_ctx
                        .transient_input::<SnapshotProof>()
                        .is_some_and(|proof| proof.nonce == "acknowledge")
                    {
                        hook_ctx.acknowledge_without_write()?;
                    }
                }
                return Ok(());
            }
            let proof = hook_ctx
                .transient_input::<SnapshotProof>()
                .cloned()
                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InvalidInput))?;
            self.0.nonces.lock().unwrap().push(proof.nonce.clone());
            if event.phase == MutationPhase::After {
                self.0.after_hooks.fetch_add(1, Ordering::SeqCst);
                // The acknowledgement belongs to the Before phase.
                assert!(!hook_ctx.can_acknowledge_without_write());
                self.0.published.lock().unwrap().push(format!(
                    "{:?} {:?} {:?}",
                    event.changes,
                    event.before_state.as_ref().map(EntityState::as_json),
                    event.after_state.as_ref().map(EntityState::as_json),
                ));
                return Ok(());
            }
            self.0.before_hooks.fetch_add(1, Ordering::SeqCst);
            let Some(current) = event.before::<StatusSnapshot>()? else {
                // Nothing to acknowledge on the insert branch.
                assert!(!hook_ctx.can_acknowledge_without_write());
                return Ok(());
            };
            if proof.sequence < current.sequence {
                return Err(
                    OrmPublicError::with_message(OrmErrorCode::Conflict, "stale snapshot")
                        .into_graphql_error(),
                );
            }
            if proof.sequence == current.sequence {
                if proof.digest != current.digest {
                    return Err(OrmPublicError::with_message(
                        OrmErrorCode::Conflict,
                        "snapshot does not match its sequence",
                    )
                    .into_graphql_error());
                }
                hook_ctx.acknowledge_without_write()?;
            }
            Ok(())
        })
    }
}

struct Fixture {
    db: Database<Backend>,
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
        #[cfg(not(feature = "mssql"))]
        drop(self);
        Ok(())
    }
}

async fn fixture() -> TestResult<Fixture> {
    #[cfg(feature = "sqlite")]
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    // The pool keeps the owned container alive; it is removed when the last clone drops.
    #[cfg(feature = "postgres")]
    let db =
        Database::new(owned_postgres::OwnedPostgres::pool("principal-bound-upserts", 8).await?);
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "principal-bound-upserts-init",
                "principal-bound upserts test",
                &[StatusSnapshot::metadata(), MachineNote::metadata()],
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
            "CREATE TABLE status_snapshots (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             machine_key NVARCHAR(64) NOT NULL UNIQUE, sequence BIGINT NOT NULL, \
             digest NVARCHAR(64) NOT NULL, payload NVARCHAR(64) NOT NULL); \
             CREATE TABLE machine_notes (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             machine_key NVARCHAR(64) NOT NULL UNIQUE, note NVARCHAR(64) NOT NULL);",
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

/// A database clone with the policies, transform and hook installed.
fn configured(
    base: &Database<Backend>,
    mode: AuthorizationMode,
    probe: &Probe,
) -> Database<Backend> {
    let mut db = base.clone().with_authorization_mode(mode);
    db.set_entity_policy(Entities(probe.clone()));
    db.set_row_policy(Rows(probe.clone()));
    db.set_field_policy(Fields);
    db.set_write_input_transform(ProofTransform(probe.clone()));
    db.set_mutation_hook(ProofHook(probe.clone()));
    db
}

fn machine(id: &str) -> AuthSubject {
    AuthSubject::from_parts(id, Vec::new(), vec!["snapshots.write".to_owned()], None)
}

fn request(
    db: &Database<Backend>,
    probe: &Probe,
    query: &str,
) -> (AppSchema, async_graphql::Request) {
    (
        schema_builder(db.clone()).finish(),
        async_graphql::Request::new(query).data(probe.clone()),
    )
}

async fn execute(
    db: &Database<Backend>,
    probe: &Probe,
    who: &str,
    query: &str,
) -> async_graphql::Response {
    let (schema, request) = request(db, probe, query);
    schema.execute(request.data(machine(who))).await
}

fn code(response: &async_graphql::Response) -> Option<String> {
    let error = response.errors.first()?;
    match error.extensions.as_ref()?.get("code")? {
        async_graphql::Value::String(code) => Some(code.clone()),
        _ => None,
    }
}

const NONCE: &str = "nonce-7c1d9e-never-stored";

fn snapshot_mutation(payload: &str, sequence: i64, digest: &str) -> String {
    format!(
        "mutation {{ {}({}: {{ {}: \"{payload}\" }}, {}: {{ sequence: {sequence}, digest: \"{digest}\", nonce: \"{NONCE}\" }}) {{ success action {} {{ {} {} {} {} }} }} }}",
        r("upsertStatusSnapshot"),
        a("input"),
        f("payload"),
        a("proof"),
        f("statusSnapshot"),
        f("id"),
        f("sequence"),
        f("digest"),
        f("payload"),
    )
}

/// The upsert's result object, or the response's first error code.
async fn upsert(
    db: &Database<Backend>,
    probe: &Probe,
    who: &str,
    payload: &str,
    sequence: i64,
    digest: &str,
) -> Result<Value, String> {
    let response = execute(
        db,
        probe,
        who,
        &snapshot_mutation(payload, sequence, digest),
    )
    .await;
    if !response.errors.is_empty() {
        return Err(code(&response).unwrap_or_else(|| format!("{:?}", response.errors)));
    }
    let mut data = response
        .data
        .into_json()
        .map_err(|error| error.to_string())?;
    Ok(data[r("upsertStatusSnapshot")].take())
}

/// Every stored snapshot, read through the repository surface.
async fn stored(db: &Database<Backend>) -> TestResult<Vec<StatusSnapshot>> {
    let mut rows = StatusSnapshot::find_many(db, StatusSnapshotWhereInput::default()).await?;
    rows.sort_by(|left, right| left.machine_key.cmp(&right.machine_key));
    Ok(rows)
}

async fn stored_for(db: &Database<Backend>, machine: &str) -> TestResult<Vec<StatusSnapshot>> {
    Ok(stored(db)
        .await?
        .into_iter()
        .filter(|row| row.machine_key == machine)
        .collect())
}

async fn one_current_row_per_principal(
    base: &Database<Backend>,
    mode: AuthorizationMode,
) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, mode, &probe);
    let label = format!("{mode:?}");
    let alpha = format!("alpha-{label}");
    let beta = format!("beta-{label}");
    let mut events = db
        .ensure_event_sender::<StatusSnapshotChangedEvent>()
        .subscribe();

    // First snapshot: the insert branch, keyed by the caller.
    let created = upsert(&db, &probe, &alpha, "first", 1, "d1").await?;
    assert_eq!(created["success"], json!(true), "{label}");
    assert_eq!(created["action"], json!("CREATED"), "{label}");
    assert_eq!(created[f("statusSnapshot")][f("sequence")], json!(1));
    let id = created[f("statusSnapshot")][f("id")].clone();
    let rows = stored_for(&db, &alpha).await?;
    assert_eq!(rows.len(), 1, "{label}");
    assert_eq!((rows[0].sequence, rows[0].digest.as_str()), (1, "d1"));
    events.recv().await?;

    // A later snapshot from the same caller updates that row.
    let updated = upsert(&db, &probe, &alpha, "second", 2, "d2").await?;
    assert_eq!(updated["action"], json!("UPDATED"), "{label}");
    assert_eq!(updated[f("statusSnapshot")][f("id")], id, "{label}");
    events.recv().await?;

    // Another caller gets its own row; neither can reach the other's.
    let other = upsert(&db, &probe, &beta, "other", 1, "o1").await?;
    assert_eq!(other["action"], json!("CREATED"), "{label}");
    assert_ne!(other[f("statusSnapshot")][f("id")], id, "{label}");
    events.recv().await?;
    assert_eq!(stored_for(&db, &alpha).await?.len(), 1, "{label}");
    assert_eq!(stored_for(&db, &beta).await?.len(), 1, "{label}");
    assert_eq!(stored_for(&db, &alpha).await?[0].payload, "second");

    // A stale snapshot is refused against the locked existing row.
    let before = probe.counts();
    assert_eq!(
        upsert(&db, &probe, &alpha, "stale", 1, "d1").await,
        Err("CONFLICT".to_owned()),
        "{label}"
    );
    // Same sequence, another digest: mismatched, refused.
    assert_eq!(
        upsert(&db, &probe, &alpha, "forged", 2, "other-digest").await,
        Err("CONFLICT".to_owned()),
        "{label}"
    );
    let after = probe.counts();
    assert_eq!(after[4], before[4] + 2, "both reached the Before hook");
    assert_eq!(after[5], before[5], "no After hook ran");
    assert_eq!(
        stored_for(&db, &alpha).await?[0].payload,
        "second",
        "{label}"
    );

    // An idempotent retry is acknowledged: success, no action, the existing
    // row, no After hook and no Changed event.
    let retried = upsert(&db, &probe, &alpha, "second", 2, "d2").await?;
    assert_eq!(retried["success"], json!(true), "{label}");
    assert_eq!(retried["action"], Value::Null, "{label}");
    assert_eq!(retried[f("statusSnapshot")][f("id")], id, "{label}");
    assert_eq!(retried[f("statusSnapshot")][f("payload")], json!("second"));
    assert_eq!(probe.counts()[5], after[5], "the retry ran no After hook");
    assert!(
        events.try_recv().is_err(),
        "{label}: refusals and the retry published nothing"
    );

    // The generated write and change delivery continue afterwards.
    let next = upsert(&db, &probe, &alpha, "third", 3, "d3").await?;
    assert_eq!(next["action"], json!("UPDATED"), "{label}");
    events.recv().await?;

    // An input transform cannot replace the key: the write lands on the
    // caller's own row.
    let (schema, request) = request(&db, &probe, &snapshot_mutation("fourth", 4, "d4"));
    let hostile = schema
        .execute(request.data(machine(&alpha)).data(HostileTransform))
        .await;
    assert!(hostile.errors.is_empty(), "{label}: {:?}", hostile.errors);
    events.recv().await?;
    let rows = stored(&db).await?;
    assert!(
        rows.iter().all(|row| row.machine_key != "hijacked"),
        "{rows:?}"
    );
    assert_eq!(
        stored_for(&db, &alpha).await?[0].payload,
        "fourth",
        "{label}"
    );

    // The transient input reached both hook phases and nothing else.
    assert!(
        probe
            .nonces
            .lock()
            .unwrap()
            .iter()
            .all(|nonce| nonce == NONCE)
    );
    for published in probe.published.lock().unwrap().iter() {
        assert!(!published.contains(NONCE), "{published}");
    }
    for row in stored(&db).await? {
        assert!(!serde_json::to_string(&row)?.contains(NONCE));
    }
    let mut delivered = 0;
    while let Ok(event) = events.try_recv() {
        delivered += 1;
        assert!(!format!("{event:?}").contains(NONCE));
    }
    assert_eq!(delivered, 0, "{label}: every event was already received");
    Ok(())
}

/// Entity, row and field write policies, scopes and operation assurance all
/// still decide a principal-bound upsert.
async fn write_authorization_still_applies(
    base: &Database<Backend>,
    mode: AuthorizationMode,
) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, mode, &probe);
    let label = format!("{mode:?}");

    // Entity policy.
    assert_eq!(
        upsert(&db, &probe, "no-entity", "x", 1, "d").await,
        Err("FORBIDDEN".to_owned()),
        "{label}"
    );
    // Field write policy on a required field.
    assert_eq!(
        upsert(&db, &probe, "no-payload", "x", 1, "d").await,
        Err("FORBIDDEN".to_owned()),
        "{label}"
    );
    // Row write policy: on the existing row, and on the row as written.
    let frozen = format!("frozen-{label}");
    assert_eq!(
        upsert(&db, &probe, &frozen, "frozen", 1, "d").await,
        Err("FORBIDDEN".to_owned()),
        "{label}: post-image"
    );
    assert!(stored_for(&db, &frozen).await?.is_empty(), "{label}");
    // Trusted repository code supplies the private key itself.
    StatusSnapshot::upsert(
        &db,
        CreateStatusSnapshotInput {
            machine_key: frozen.clone(),
            sequence: 1,
            digest: "d".to_owned(),
            payload: "frozen".to_owned(),
        },
    )
    .await?;
    assert_eq!(
        upsert(&db, &probe, &frozen, "thawed", 2, "d2").await,
        Err("FORBIDDEN".to_owned()),
        "{label}: preimage"
    );
    assert_eq!(
        stored_for(&db, &frozen).await?[0].payload,
        "frozen",
        "{label}"
    );

    // Resolver authentication and operation scopes come first: the binding
    // is never asked.
    let bindings = probe.counts()[0];
    let query = snapshot_mutation("x", 1, "d");
    let (schema, anonymous) = request(&db, &probe, &query);
    assert_eq!(
        code(&schema.execute(anonymous).await).as_deref(),
        Some("UNAUTHENTICATED"),
        "{label}"
    );
    let (schema, unscoped) = request(&db, &probe, &query);
    assert_eq!(
        code(
            &schema
                .execute(unscoped.data(AuthSubject::new("unscoped")))
                .await
        )
        .as_deref(),
        Some("FORBIDDEN"),
        "{label}"
    );
    // Operation assurance.
    let config = AssuranceSchemaConfig::legacy()
        .with_default_interactive_mutation_policy("interactive.recent-auth")?
        .with_strict_mutation_classification(true);
    let registry =
        OperationAssuranceRegistry::builder(graphql_orm_operation_catalog(), config).build()?;
    let (schema, stepped) = request(&db, &probe, &query);
    let stepped = stepped
        .data(machine("needs-step-up"))
        .data(AssuranceEnforcement::new(
            Arc::new(registry),
            Arc::new(StepUp),
        ));
    assert_eq!(
        code(&schema.execute(stepped).await).as_deref(),
        Some("STEP_UP_REQUIRED"),
        "{label}"
    );
    assert_eq!(
        probe.counts()[0],
        bindings,
        "{label}: no binding was resolved"
    );
    assert!(
        stored_for(&db, "needs-step-up").await?.is_empty(),
        "{label}"
    );
    assert!(stored_for(&db, "unscoped").await?.is_empty(), "{label}");
    Ok(())
}

#[derive(Debug)]
struct StepUp;

impl AssuranceRequirementEvaluator for StepUp {
    fn enforce(
        &self,
        _: &AssuranceCheck<'_>,
        _: AssuranceActorClass,
        _: &str,
    ) -> async_graphql::Result<()> {
        Err(
            async_graphql::Error::new("additional authentication is required")
                .extend_with(|_, extensions| extensions.set("code", "STEP_UP_REQUIRED")),
        )
    }
}

/// A failed binding refuses the mutation before any policy, transform, hook,
/// read or write.
async fn failed_binding_refuses_first(
    base: &Database<Backend>,
    mode: AuthorizationMode,
) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, mode, &probe);
    let label = format!("{mode:?}");
    let rows = stored(&db).await?.len();
    let (schema, request) = request(&db, &probe, &snapshot_mutation("x", 1, "d"));
    let response = schema
        .execute(request.data(machine("unbound")).data(NoMachineIdentity))
        .await;
    assert_eq!(code(&response).as_deref(), Some("FORBIDDEN"), "{label}");
    assert_eq!(
        response.errors[0].message,
        "this request has no machine identity"
    );
    assert_eq!(
        probe.counts(),
        [1, 0, 0, 0, 0, 0],
        "{label}: only the binding ran"
    );
    assert_eq!(stored(&db).await?.len(), rows, "{label}");
    Ok(())
}

/// The key has no client-facing surface, and no client value reaches it.
async fn key_has_no_public_surface(base: &Database<Backend>) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, AuthorizationMode::default(), &probe);
    let sdl = schema_builder(db.clone())
        .finish()
        .sdl()
        .to_ascii_lowercase();
    assert!(sdl.contains("statussnapshot"), "fixture sanity: {sdl}");
    assert!(
        !sdl.contains("machine_key") && !sdl.contains("machinekey"),
        "{sdl}"
    );
    // The transient type appears as the upsert's argument type only.
    assert!(sdl.contains("input snapshotproof"), "{sdl}");
    assert_eq!(sdl.matches("snapshotproof").count(), 3, "{sdl}");

    let proof = format!(
        "{}: {{ sequence: 9, digest: \"d\", nonce: \"n\" }}",
        a("proof")
    );
    for query in [
        // In the upsert input, under either spelling.
        format!(
            "mutation {{ {}({}: {{ {}: \"x\", machineKey: \"victim\" }}, {proof}) {{ success }} }}",
            r("upsertStatusSnapshot"),
            a("input"),
            f("payload"),
        ),
        format!(
            "mutation {{ {}({}: {{ {}: \"x\", machine_key: \"victim\" }}, {proof}) {{ success }} }}",
            r("upsertStatusSnapshot"),
            a("input"),
            f("payload"),
        ),
        // As an argument, an output, a filter and an ordering.
        format!(
            "mutation {{ {}({}: {{ {}: \"x\" }}, {proof}, machineKey: \"victim\") {{ success }} }}",
            r("upsertStatusSnapshot"),
            a("input"),
            f("payload"),
        ),
        format!(
            "{{ {} {{ edges {{ node {{ machineKey }} }} }} }}",
            r("statusSnapshots")
        ),
        format!(
            "{{ {}({}: {{ machineKey: {{ eq: \"victim\" }} }}) {{ edges {{ node {{ {} }} }} }} }}",
            r("statusSnapshots"),
            a("where"),
            f("id"),
        ),
        format!(
            "{{ {}({}: [{{ machineKey: ASC }}]) {{ edges {{ node {{ {} }} }} }} }}",
            r("statusSnapshots"),
            a("orderBy"),
            f("id"),
        ),
        // The other update path for the same column: the generated update.
        format!(
            "mutation {{ {}({}: \"any\", {}: {{ {}: \"x\", machineKey: \"victim\" }}) {{ success }} }}",
            r("updateMachineNote"),
            a("id"),
            a("input"),
            f("note"),
        ),
        // A required transient input cannot be left out.
        format!(
            "mutation {{ {}({}: {{ {}: \"x\" }}) {{ success }} }}",
            r("upsertStatusSnapshot"),
            a("input"),
            f("payload"),
        ),
    ] {
        let response = execute(&db, &probe, "client", &query).await;
        assert!(!response.errors.is_empty(), "{query} was accepted");
        assert!(code(&response).is_none(), "{query}: {:?}", response.errors);
    }
    assert_eq!(probe.counts(), [0; 6], "schema validation refused each one");
    assert!(stored_for(&db, "victim").await?.is_empty());
    assert!(stored_for(&db, "client").await?.is_empty());

    // Neither create nor a second upsert shape exists for these entities.
    for absent in [
        "createStatusSnapshot",
        "createMachineNote",
        "updateStatusSnapshot",
    ] {
        let field = format!("{}(", r(absent)).to_ascii_lowercase();
        assert!(!sdl.contains(&field), "{absent} in SDL");
    }
    assert!(sdl.contains(&format!("{}(", r("upsertMachineNote")).to_ascii_lowercase()));

    // Descriptive catalogues never name the key.
    for catalogue in [
        serde_json::to_string(graphql_orm_semantic_catalog())?,
        serde_json::to_string(graphql_orm_resolver_catalog().map_err(|error| error.to_string())?)?,
        serde_json::to_string(graphql_orm_client_metadata())?,
    ] {
        let catalogue = catalogue.to_ascii_lowercase();
        assert!(
            !catalogue.contains("machine_key") && !catalogue.contains("machinekey"),
            "private key in catalogue: {catalogue}"
        );
        assert!(!catalogue.contains("nonce"), "{catalogue}");
    }
    // The operation catalogue lists the transient input as an argument type
    // of the upsert and publishes no key field, argument or type. Its
    // host-side schema signature records the key as private.
    let mut transient_arguments = 0;
    for operation in graphql_orm_operation_catalog().operations() {
        let generated = operation.generated();
        let mut public = vec![
            generated.field_name().to_owned(),
            generated.description().to_owned(),
            generated.graphql_result_type().to_owned(),
        ];
        for argument in generated.arguments() {
            if argument.graphql_type().contains("SnapshotProof") {
                transient_arguments += 1;
                assert!(
                    generated
                        .field_name()
                        .to_ascii_lowercase()
                        .contains("upsert")
                );
            }
            public.extend([
                argument.graphql_name().to_owned(),
                argument.description().to_owned(),
                argument.graphql_type().to_owned(),
            ]);
        }
        let public = public.join(" ").to_ascii_lowercase();
        assert!(
            !public.contains("machine_key") && !public.contains("machinekey"),
            "{public}"
        );
        for line in generated
            .schema_signature()
            .lines()
            .filter(|line| line.starts_with("field ") && line.contains("machine_key"))
        {
            for marker in [
                "output=false",
                "create=false",
                "update=false",
                "private=true",
                "filter=false",
                "order=false",
                "subscribe=false",
            ] {
                assert!(line.contains(marker), "{marker} missing: {line}");
            }
        }
    }
    assert_eq!(transient_arguments, 2, "one per declaring upsert");
    Ok(())
}

/// An optional transient input may be left out, and an acknowledgement that
/// the entity did not declare fails the mutation instead of skipping it.
async fn undeclared_acknowledgement_is_refused(base: &Database<Backend>) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, AuthorizationMode::default(), &probe);
    let note = |text: &str, proof: &str| {
        format!(
            "mutation {{ {}({}: {{ {}: \"{text}\" }}{proof}) {{ success action {} {{ {} }} }} }}",
            r("upsertMachineNote"),
            a("input"),
            f("note"),
            f("machineNote"),
            f("note"),
        )
    };
    let first = execute(&db, &probe, "note-machine", &note("one", "")).await;
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let proof = format!(
        ", {}: {{ sequence: 1, digest: \"d\", nonce: \"acknowledge\" }}",
        a("proof")
    );
    let refused = execute(&db, &probe, "note-machine", &note("two", &proof)).await;
    assert_eq!(
        code(&refused).as_deref(),
        Some("CONFLICT"),
        "{:?}",
        refused.errors
    );
    let rows = MachineNote::find_many(&db, MachineNoteWhereInput::default()).await?;
    let mine = rows
        .iter()
        .filter(|row| row.machine_key == "note-machine")
        .collect::<Vec<_>>();
    assert_eq!(mine.len(), 1);
    assert_eq!(
        mine[0].note, "one",
        "the refused acknowledgement wrote nothing"
    );

    // The transform cannot move a note to another key either.
    let (schema, request) = request(&db, &probe, &note("three", ""));
    let hostile = schema
        .execute(request.data(machine("note-machine")).data(HostileTransform))
        .await;
    assert!(hostile.errors.is_empty(), "{:?}", hostile.errors);
    let rows = MachineNote::find_many(&db, MachineNoteWhereInput::default()).await?;
    assert!(
        rows.iter().all(|row| row.machine_key != "hijacked"),
        "{rows:?}"
    );
    Ok(())
}

/// Concurrent snapshots from one machine leave exactly one row, holding the
/// newest snapshot that was accepted.
async fn concurrent_upserts_serialise(base: &Database<Backend>) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, AuthorizationMode::default(), &probe);
    let outcomes = graphql_orm::futures::future::join_all((1..=8).map(|sequence| {
        let (db, probe) = (db.clone(), probe.clone());
        async move {
            let digest = format!("c{sequence}");
            let payload = format!("concurrent-{sequence}");
            (
                sequence,
                upsert(&db, &probe, "racer", &payload, sequence, &digest).await,
            )
        }
    }))
    .await;
    let rows = stored_for(&db, "racer").await?;
    assert_eq!(rows.len(), 1, "{outcomes:?}");
    let accepted = outcomes
        .iter()
        .filter(|(_, outcome)| outcome.is_ok())
        .map(|(sequence, _)| *sequence)
        .collect::<Vec<_>>();
    assert!(!accepted.is_empty(), "{outcomes:?}");
    // A refusal is a stale snapshot or a database serialization conflict; it
    // never leaves a second row or an older snapshot on top.
    assert_eq!(
        Some(rows[0].sequence),
        accepted.iter().copied().max(),
        "{outcomes:?}"
    );
    assert_eq!(rows[0].payload, format!("concurrent-{}", rows[0].sequence));
    assert_eq!(rows[0].digest, format!("c{}", rows[0].sequence));

    // A sequential tail still orders correctly after the race.
    let latest = rows[0].sequence;
    assert_eq!(
        upsert(&db, &probe, "racer", "late", latest - 1, "x").await,
        Err("CONFLICT".to_owned())
    );
    let next = upsert(&db, &probe, "racer", "tail", latest + 1, "t").await?;
    assert_eq!(next["action"], json!("UPDATED"));
    assert_eq!(stored_for(&db, "racer").await?.len(), 1);
    Ok(())
}

/// Trusted transaction code can run the same hooks with its own transient
/// input; a hook cannot grant itself one.
async fn transaction_code_sets_its_own_transient_input(base: &Database<Backend>) -> TestResult {
    struct Seen(Arc<Mutex<Vec<(MutationPhase, Option<i64>, bool)>>>);
    impl MutationHook<Backend> for Seen {
        fn on_mutation<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            hook_ctx: &'a mut MutationContext<'_, Backend>,
            event: &'a MutationEvent,
        ) -> BoxFuture<'a, async_graphql::Result<()>> {
            Box::pin(async move {
                let sequence = hook_ctx
                    .transient_input::<SnapshotProof>()
                    .map(|proof| proof.sequence);
                let refused = hook_ctx
                    .set_transient_input(SnapshotProof {
                        sequence: 99,
                        digest: String::new(),
                        nonce: String::new(),
                    })
                    .is_err()
                    && hook_ctx.take_transient_input::<SnapshotProof>().is_err()
                    && hook_ctx.acknowledge_without_write().is_err();
                self.0
                    .lock()
                    .unwrap()
                    .push((event.phase.clone(), sequence, refused));
                Ok(())
            })
        }
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut db = configured(base, AuthorizationMode::default(), &Probe::default());
    db.set_mutation_hook(Seen(seen.clone()));
    let input = |sequence| CreateStatusSnapshotInput {
        machine_key: "transaction-machine".to_owned(),
        sequence,
        digest: "t".to_owned(),
        payload: "transaction".to_owned(),
    };
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            tx.set_transient_input(SnapshotProof {
                sequence: 5,
                digest: "t".to_owned(),
                nonce: NONCE.to_owned(),
            })?;
            // An extension of the same type is a separate value.
            assert!(tx.extension::<SnapshotProof>().is_none());
            tx.upsert::<StatusSnapshot>(input(5)).await?;
            let taken = tx.take_transient_input::<SnapshotProof>()?;
            assert_eq!(taken.map(|proof| proof.sequence), Some(5));
            tx.upsert::<StatusSnapshot>(input(6)).await?;
            Ok(())
        })
    })
    .await?;
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            (MutationPhase::Before, Some(5), true),
            (MutationPhase::After, Some(5), true),
            (MutationPhase::Before, None, true),
            (MutationPhase::After, None, true),
        ]
    );
    assert_eq!(stored_for(&db, "transaction-machine").await?[0].sequence, 6);
    Ok(())
}

/// Name a repository refusal by its public code and message.
fn explain(step: &str, error: Box<dyn std::error::Error>) -> Box<dyn std::error::Error> {
    match error.downcast::<graphql_orm::sqlx::Error>() {
        Ok(error) => format!("{step}: {:?}", OrmPublicError::from_sqlx(&error)).into(),
        Err(error) => format!("{step}: {error}").into(),
    }
}

async fn every_contract(fixture: &Fixture) -> TestResult {
    every_contract_step(fixture)
        .await
        .map_err(|(step, error)| explain(step, error))
}

async fn every_contract_step(
    fixture: &Fixture,
) -> Result<(), (&'static str, Box<dyn std::error::Error>)> {
    for mode in [
        AuthorizationMode::LegacyPermissive,
        AuthorizationMode::DeclaredPoliciesRequired,
        AuthorizationMode::ExplicitPolicyForAllExposedOperations,
    ] {
        one_current_row_per_principal(&fixture.db, mode)
            .await
            .map_err(|error| ("one current row per principal", error))?;
        write_authorization_still_applies(&fixture.db, mode)
            .await
            .map_err(|error| ("write authorization", error))?;
        failed_binding_refuses_first(&fixture.db, mode)
            .await
            .map_err(|error| ("failed binding", error))?;
    }
    key_has_no_public_surface(&fixture.db)
        .await
        .map_err(|error| ("public surface", error))?;
    undeclared_acknowledgement_is_refused(&fixture.db)
        .await
        .map_err(|error| ("undeclared acknowledgement", error))?;
    concurrent_upserts_serialise(&fixture.db)
        .await
        .map_err(|error| ("concurrent upserts", error))?;
    transaction_code_sets_its_own_transient_input(&fixture.db)
        .await
        .map_err(|error| ("transaction transient input", error))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn principal_bound_upserts_on_sqlite() -> TestResult {
    let fixture = fixture().await?;
    every_contract(&fixture).await?;
    fixture.finish()
}

#[cfg(not(feature = "sqlite"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "starts a test-owned database container"]
async fn principal_bound_upserts_on_an_owned_database() -> TestResult {
    let fixture = fixture().await?;
    every_contract(&fixture).await?;
    fixture.finish()
}
