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
        match ctx.data_opt::<NoMachineIdentity>() {
            // A coded public refusal carrying a diagnostic and an extension.
            Some(NoMachineIdentity::Coded) => {
                return Err(
                    OrmPublicError::with_message(OrmErrorCode::NotFound, BINDING_SECRET)
                        .into_graphql_error()
                        .extend_with(|_, extensions| extensions.set("detail", BINDING_SECRET)),
                );
            }
            // An uncoded error, which a compatibility setting can expose.
            Some(NoMachineIdentity::Uncoded) => {
                return Err(async_graphql::Error::new(BINDING_SECRET));
            }
            None => {}
        }
        Ok(ctx.auth_subject()?.id)
    }
}

/// Request data: the verified context carries no machine identity, and the
/// binding fails with an error that names a secret.
#[derive(Clone, Copy)]
enum NoMachineIdentity {
    Coded,
    Uncoded,
}
const BINDING_SECRET: &str = "issuer-key-4f1a-not-for-clients";
/// Request data: a first write of an absent key waits here for its peers.
#[derive(Clone)]
struct Race(Arc<tokio::sync::Barrier>);
/// Request data: the rendered execution document, as a logger would see it.
#[derive(Clone, Default)]
struct RenderedDocuments(Arc<Mutex<Vec<String>>>);
/// Request data: the input transform tries to replace the bound key.
struct HostileTransform;

/// Ordering proof sent with every snapshot. Never stored or published.
#[derive(InputObject, Clone)]
pub struct SnapshotProof {
    pub sequence: i64,
    pub digest: String,
    pub nonce: String,
    pub tags: Option<Vec<i32>>,
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
        transient_input(
            type = "SnapshotProof",
            on = "upsert",
            name = "proof",
            max_bytes = 512,
            max_nodes = 16
        ),
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
        transient_input(
            type = "SnapshotProof",
            on = "upsert",
            name = "proof",
            max_bytes = 512,
            max_nodes = 16
        ),
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
        transient_input(
            type = "SnapshotProof",
            on = "upsert",
            name = "proof",
            max_bytes = 512,
            max_nodes = 16
        ),
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
        mutation_shapes = "upsert,update,update_many",
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
        mutation_shapes = "upsert,update,update_many",
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
        mutation_shapes = "upsert,update,update_many",
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
    hook_subjects: Arc<Mutex<Vec<String>>>,
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

    fn can_access_entity_with_authority<'a>(
        &'a self,
        authority: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database<Backend>,
        entity: &'static str,
        key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        if let Some(subject) = authority
            .hook_request()
            .and_then(|request| request.subject())
        {
            assert!(ctx.is_none());
            assert_eq!(surface, EntityAccessSurface::Repository);
            self.0
                .hook_subjects
                .lock()
                .unwrap()
                .push(subject.id.clone());
        }
        self.can_access_entity(ctx, db, entity, key, kind, surface)
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
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        let blind = subject(ctx).starts_with("blind-row");
        Box::pin(async move {
            Ok(if blind {
                ReadVisibility::CallbackOnly
            } else {
                ReadVisibility::Unrestricted
            })
        })
    }

    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        // `blind-row-*` may write its snapshot but never read it back.
        let readable = !subject(ctx).starts_with("blind-row");
        Box::pin(async move { Ok(readable) })
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
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database<Backend>,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        // `blind-field-*` may write a payload but never read one.
        let readable = !(field.eq_ignore_ascii_case("payload")
            && subject(Some(ctx)).starts_with("blind-field"));
        Box::pin(async move { Ok(readable) })
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
    fn before_update_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_, Backend>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let hostile = write_ctx
                .graphql_ctx()
                .is_some_and(|ctx| ctx.data_opt::<HostileTransform>().is_some());
            if let (true, Some(note)) = (hostile, input.downcast_mut::<UpdateMachineNoteInput>()) {
                note.machine_key = Some("hijacked".to_owned());
            }
            Ok(())
        })
    }

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
            if hook_ctx.database().propagates_request_authority_to_hooks() {
                hook_ctx
                    .__policy()
                    .ensure_entity_access(
                        None,
                        event.entity_name,
                        event.metadata.write_policy,
                        EntityAccessKind::Write,
                        EntityAccessSurface::Repository,
                    )
                    .await?;
            }
            if event.entity_name == "MachineNote" {
                // No acknowledgement is armed for this entity.
                if event.phase == MutationPhase::Before && event.before_state.is_some() {
                    assert!(!hook_ctx.can_acknowledge_without_write());
                    let nonce = hook_ctx
                        .transient_input::<SnapshotProof>()
                        .map(|proof| proof.nonce.clone());
                    match nonce.as_deref() {
                        Some("acknowledge") => hook_ctx.acknowledge_without_write()?,
                        // A hook that ignores the refusal.
                        Some("acknowledge-and-ignore") => {
                            let _ = hook_ctx.acknowledge_without_write();
                        }
                        _ => {}
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
                // Hold a first write of an absent key until its peers have
                // also found the key absent, where the backend lets them.
                if let Some(race) = ctx.and_then(|ctx| ctx.data_opt::<Race>()) {
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_millis(1500), race.0.wait())
                            .await;
                }
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
                &[
                    StatusSnapshot::metadata(),
                    #[cfg(feature = "postgres")]
                    MachineNote::metadata(),
                ],
            )
            .await?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    // A key column whose equality folds case, as SQL Server's default
    // collation does: distinct principals can compare equal in SQL.
    #[cfg(feature = "sqlite")]
    sqlx::query(
        "CREATE TABLE machine_notes (id TEXT NOT NULL PRIMARY KEY, \
         machine_key TEXT NOT NULL UNIQUE COLLATE NOCASE, note TEXT NOT NULL)",
    )
    .execute(db.pool())
    .await?;
    #[cfg(feature = "mssql")]
    let (db, owner) = {
        let owner = owned_mssql::OwnedSqlServer::start().await?;
        owned_mssql::execute_batch(
            &owner.connection_string,
            "CREATE TABLE status_snapshots (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             machine_key NVARCHAR(64) NOT NULL UNIQUE, sequence BIGINT NOT NULL, \
             digest NVARCHAR(400) NOT NULL, payload NVARCHAR(64) NOT NULL); \
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
    let db = db.with_request_authority_propagated_to_hooks(true);
    let label = format!("{mode:?}");
    let alpha = format!("alpha-{label}");
    let beta = format!("beta-{label}");
    let mut events = db
        .ensure_event_sender::<StatusSnapshotChangedEvent>()
        .subscribe();

    // First snapshot: the insert branch, keyed by the caller.
    let created = upsert(&db, &probe, &alpha, "first", 1, "d1").await?;
    assert_eq!(created["success"], json!(true), "{label}");
    assert_eq!(
        *probe.hook_subjects.lock().unwrap(),
        [alpha.clone(), alpha.clone()]
    );
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
    probe.hook_subjects.lock().unwrap().clear();
    let retried = upsert(&db, &probe, &alpha, "second", 2, "d2").await?;
    assert_eq!(
        probe.hook_subjects.lock().unwrap().as_slice(),
        std::slice::from_ref(&alpha)
    );
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
    let label = format!("{mode:?}");
    // The refusal is the same fixed code and message whatever the binding
    // returned, also when the host exposes uncoded callback messages.
    for (failure, exposed) in [
        (NoMachineIdentity::Coded, false),
        (NoMachineIdentity::Uncoded, false),
        (NoMachineIdentity::Coded, true),
        (NoMachineIdentity::Uncoded, true),
    ] {
        let probe = Probe::default();
        let mut db = configured(base, mode, &probe);
        if exposed {
            db = db.with_uncoded_callback_messages_exposed(true);
        }
        let rows = stored(&db).await?.len();
        let (schema, request) = request(&db, &probe, &snapshot_mutation("x", 1, "d"));
        let response = schema
            .execute(request.data(machine("unbound")).data(failure))
            .await;
        assert_eq!(code(&response).as_deref(), Some("FORBIDDEN"), "{label}");
        assert_eq!(response.errors[0].message, PRINCIPAL_KEY_BINDING_REFUSAL);
        let rendered = serde_json::to_string(&response)?;
        assert!(!rendered.contains(BINDING_SECRET), "{label}: {rendered}");
        assert!(!rendered.contains("detail"), "{label}: {rendered}");
        assert_eq!(
            probe.counts(),
            [1, 0, 0, 0, 0, 0],
            "{label}: only the binding ran"
        );
        assert_eq!(stored(&db).await?.len(), rows, "{label}");
    }
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
    // Both transient arguments carry the schema marker that tool-profile
    // compilers refuse to map.
    assert!(sdl.contains("snapshotproof! @transientinput"), "{sdl}");
    assert!(sdl.contains("snapshotproof @transientinput"), "{sdl}");
    assert!(
        sdl.contains("directive @transientinput on argument_definition"),
        "{sdl}"
    );

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
                // The declared type, not the generated wrapper.
                assert!(
                    [
                        "SnapshotProof",
                        ":: std :: option :: Option < SnapshotProof >"
                    ]
                    .contains(&argument.rust_type()),
                    "{}",
                    argument.rust_type()
                );
                assert!(
                    ["SnapshotProof!", "SnapshotProof"].contains(&argument.graphql_type()),
                    "{}",
                    argument.graphql_type()
                );
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

    // A hook that ignores the refusal still cannot let the write through.
    let proof = format!(
        ", {}: {{ sequence: 1, digest: \"d\", nonce: \"acknowledge-and-ignore\" }}",
        a("proof")
    );
    let ignored = execute(&db, &probe, "note-machine", &note("two", &proof)).await;
    assert_eq!(
        code(&ignored).as_deref(),
        Some("FORBIDDEN"),
        "{:?}",
        ignored.errors
    );
    let rows = MachineNote::find_many(&db, MachineNoteWhereInput::default()).await?;
    assert!(rows.iter().all(|row| row.note == "one"), "{rows:?}");

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

/// The generated update and bulk update never assign the bound column, even
/// when their input transform puts a value there.
async fn updates_cannot_rewrite_the_key(base: &Database<Backend>) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, AuthorizationMode::default(), &probe);
    let created = execute(
        &db,
        &probe,
        "update-machine",
        &format!(
            "mutation {{ {}({}: {{ {}: \"zero\" }}) {{ {} {{ {} }} }} }}",
            r("upsertMachineNote"),
            a("input"),
            f("note"),
            f("machineNote"),
            f("id"),
        ),
    )
    .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let id = created.data.into_json()?[r("upsertMachineNote")][f("machineNote")][f("id")]
        .as_str()
        .ok_or("note id")?
        .to_owned();
    for (text, mutation) in [
        (
            "single",
            format!(
                "mutation {{ {}({}: \"{id}\", {}: {{ {}: \"single\" }}) {{ success }} }}",
                r("updateMachineNote"),
                a("id"),
                a("input"),
                f("note"),
            ),
        ),
        (
            "bulk",
            format!(
                "mutation {{ {}({}: {{ {}: {{ eq: \"{id}\" }} }}, {}: {{ {}: \"bulk\" }}) {{ success }} }}",
                r("updateMachineNotes"),
                a("where"),
                f("id"),
                a("input"),
                f("note"),
            ),
        ),
    ] {
        let (schema, request) = request(&db, &probe, &mutation);
        let response = schema
            .execute(
                request
                    .data(machine("update-machine"))
                    .data(HostileTransform),
            )
            .await;
        assert!(response.errors.is_empty(), "{text}: {:?}", response.errors);
        let rows = MachineNote::find_many(&db, MachineNoteWhereInput::default()).await?;
        let row = rows.iter().find(|row| row.id == id).ok_or("updated note")?;
        assert_eq!(row.note, text, "the update itself was applied");
        assert_eq!(row.machine_key, "update-machine", "{text} rewrote the key");
        assert!(rows.iter().all(|row| row.machine_key != "hijacked"));
    }
    Ok(())
}

/// Two principals whose keys the database compares equal but which differ
/// exactly never share a row.
async fn database_equality_cannot_merge_principals(base: &Database<Backend>) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, AuthorizationMode::default(), &probe);
    let note = |text: &str| {
        format!(
            "mutation {{ {}({}: {{ {}: \"{text}\" }}) {{ success action }} }}",
            r("upsertMachineNote"),
            a("input"),
            f("note"),
        )
    };
    let first = execute(&db, &probe, "Collated-Machine", &note("mine")).await;
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let before = probe.counts();
    let second = execute(&db, &probe, "collated-machine", &note("theirs")).await;
    let rows = MachineNote::find_many(&db, MachineNoteWhereInput::default()).await?;
    let colliding = rows
        .iter()
        .filter(|row| row.machine_key.eq_ignore_ascii_case("collated-machine"))
        .collect::<Vec<_>>();
    if cfg!(feature = "postgres") {
        // A case-sensitive key column: the second principal has its own row.
        assert!(second.errors.is_empty(), "{:?}", second.errors);
        assert_eq!(colliding.len(), 2, "{rows:?}");
    } else {
        // SQLite NOCASE and SQL Server's default collation match the first
        // principal's row. The upsert refuses with the generic row-write
        // denial, before any row policy, field policy or hook, and writes
        // nothing.
        assert_eq!(
            code(&second).as_deref(),
            Some("FORBIDDEN"),
            "{:?}",
            second.errors
        );
        assert_eq!(
            second.errors[0].message,
            OrmPublicError::forbidden().to_string()
        );
        let after = probe.counts();
        assert_eq!(after[2], before[2], "no row policy was asked");
        assert_eq!(after[4], before[4], "no hook ran");
        assert_eq!(colliding.len(), 1, "{rows:?}");
        assert_eq!(colliding[0].machine_key, "Collated-Machine");
        assert_eq!(colliding[0].note, "mine");
    }
    Ok(())
}

/// Every ownership-specific refusal is one identical response: an existing
/// row whose key only the database considers equal, a written row whose key
/// is not exactly the bound one, and a written row that cannot be read back.
#[cfg(feature = "sqlite")]
async fn ownership_refusals_are_indistinguishable(base: &Database<Backend>) -> TestResult {
    // Triggers stand in for whatever lets a write land somewhere else.
    for trigger in [
        "CREATE TRIGGER notes_vanish AFTER INSERT ON machine_notes WHEN NEW.note = 'vanish' \
         BEGIN DELETE FROM machine_notes WHERE id = NEW.id; END",
        "CREATE TRIGGER notes_rekey AFTER INSERT ON machine_notes WHEN NEW.note = 'rekey' \
         BEGIN UPDATE machine_notes SET machine_key = upper(machine_key) WHERE id = NEW.id; END",
    ] {
        sqlx::query(trigger).execute(base.pool()).await?;
    }
    let probe = Probe::default();
    let db =
        configured(base, AuthorizationMode::default(), &probe).with_mutation_error_outcome(true);
    let note = |text: &str| {
        format!(
            "mutation {{ {}({}: {{ {}: \"{text}\" }}) {{ success action }} }}",
            r("upsertMachineNote"),
            a("input"),
            f("note"),
        )
    };
    let seeded = execute(&db, &probe, "Owned-Machine", &note("mine")).await;
    assert!(seeded.errors.is_empty(), "{:?}", seeded.errors);
    let rows_before = MachineNote::find_many(&db, MachineNoteWhereInput::default())
        .await?
        .len();
    let mut responses = Vec::new();
    for (who, text) in [
        // Pre-image: the lookup returns another principal's row.
        ("owned-machine", "taken"),
        // Post-image: the written row no longer holds the bound key exactly.
        ("rekey-machine", "rekey"),
        // Missing post-image: the written row cannot be read back.
        ("vanish-machine", "vanish"),
    ] {
        let response = execute(&db, &probe, who, &note(text)).await;
        assert_eq!(
            code(&response).as_deref(),
            Some("FORBIDDEN"),
            "{who}: {:?}",
            response.errors
        );
        responses.push((who, serde_json::to_value(&response)?));
    }
    for (who, response) in &responses {
        assert_eq!(response, &responses[0].1, "{who} is distinguishable");
        assert_eq!(
            response["errors"][0]["extensions"]["outcome"],
            json!("refused")
        );
    }
    // None of them wrote anything.
    let rows = MachineNote::find_many(&db, MachineNoteWhereInput::default()).await?;
    assert_eq!(rows.len(), rows_before, "{rows:?}");
    for trigger in ["notes_vanish", "notes_rekey"] {
        sqlx::query(&format!("DROP TRIGGER {trigger}"))
            .execute(base.pool())
            .await?;
    }
    Ok(())
}

/// Captures the execution document the way a logging extension renders it.
struct RenderDocuments;

impl async_graphql::extensions::ExtensionFactory for RenderDocuments {
    fn create(&self) -> Arc<dyn async_graphql::extensions::Extension> {
        Arc::new(RenderDocuments)
    }
}

#[async_graphql::async_trait::async_trait]
impl async_graphql::extensions::Extension for RenderDocuments {
    async fn parse_query(
        &self,
        ctx: &async_graphql::extensions::ExtensionContext<'_>,
        query: &str,
        variables: &async_graphql::Variables,
        next: async_graphql::extensions::NextParseQuery<'_>,
    ) -> async_graphql::ServerResult<async_graphql::parser::types::ExecutableDocument> {
        let document = next.run(ctx, query, variables).await?;
        if let Some(rendered) = ctx.data_opt::<RenderedDocuments>() {
            rendered
                .0
                .lock()
                .unwrap()
                .push(ctx.stringify_execute_doc(&document, variables));
        }
        Ok(document)
    }
}

/// The transient argument is redacted where async-graphql renders a request,
/// and an oversized one is refused before it is coerced.
async fn transient_argument_is_secret_and_bounded(base: &Database<Backend>) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, AuthorizationMode::default(), &probe);
    let rendered = RenderedDocuments::default();
    let run = |query: String, variables: Value| {
        let (db, probe, rendered) = (db.clone(), probe.clone(), rendered.clone());
        async move {
            schema_builder(db)
                .extension(RenderDocuments)
                .finish()
                .execute(
                    async_graphql::Request::new(query)
                        .variables(async_graphql::Variables::from_json(variables))
                        .data(probe)
                        .data(rendered)
                        .data(machine("bounded-machine")),
                )
                .await
        }
    };
    let with_variable = format!(
        "mutation($proof: SnapshotProof!) {{ {}({}: {{ {}: \"x\" }}, {}: $proof) {{ success }} }}",
        r("upsertStatusSnapshot"),
        a("input"),
        f("payload"),
        a("proof"),
    );
    let proof = |digest: &str, tags: usize| json!({ "sequence": 1, "digest": digest, "nonce": NONCE, "tags": vec![7; tags] });

    // Redaction, for a literal and for a variable.
    let literal = run(snapshot_mutation("x", 1, "d1"), json!({})).await;
    assert!(literal.errors.is_empty(), "{:?}", literal.errors);
    let variable = run(with_variable.clone(), json!({ "proof": proof("d1", 2) })).await;
    assert!(variable.errors.is_empty(), "{:?}", variable.errors);
    let documents = rendered.0.lock().unwrap().clone();
    assert_eq!(documents.len(), 2);
    for document in &documents {
        assert!(document.contains("<secret>"), "{document}");
        assert!(!document.contains(NONCE), "{document}");
        assert!(!document.contains("d1"), "{document}");
    }

    // Size: 512 bytes and 16 values for this entity.
    let before = probe.counts();
    let large = "x".repeat(600);
    let literal_large = snapshot_mutation("x", 2, &large);
    for (label, response) in [
        ("literal bytes", run(literal_large, json!({})).await),
        (
            "variable bytes",
            run(with_variable.clone(), json!({ "proof": proof(&large, 0) })).await,
        ),
        (
            "variable values",
            run(with_variable.clone(), json!({ "proof": proof("d2", 20) })).await,
        ),
    ] {
        assert_eq!(response.errors.len(), 1, "{label}");
        assert!(
            response.errors[0]
                .message
                .contains("transient input exceeds its size limit"),
            "{label}: {:?}",
            response.errors
        );
        assert!(
            !serde_json::to_string(&response)?.contains(&large),
            "{label}"
        );
    }
    assert_eq!(probe.counts(), before, "refused before the resolver ran");
    assert_eq!(stored_for(&db, "bounded-machine").await?[0].sequence, 1);
    // A value of the wrong shape is refused without being quoted back, and
    // so is a variable default, which would put the value in the query text.
    let before = probe.counts();
    const LEAK: &str = "leak-9f2c-not-for-logs";
    let mut malformed = proof("d2", 0);
    malformed["sequence"] = json!(LEAK);
    let wrong_shape = run(with_variable.clone(), json!({ "proof": malformed })).await;
    assert!(!wrong_shape.errors.is_empty());
    assert!(
        !serde_json::to_string(&wrong_shape)?.contains(LEAK),
        "{:?}",
        wrong_shape.errors
    );
    let wrong_literal = run(
        snapshot_mutation("x", 2, "d2").replace("sequence: 2", &format!("sequence: \"{LEAK}\"")),
        json!({}),
    )
    .await;
    assert!(!wrong_literal.errors.is_empty());
    assert!(
        !serde_json::to_string(&wrong_literal)?.contains(LEAK),
        "{:?}",
        wrong_literal.errors
    );
    let defaulted = run(
        with_variable.replace(
            "$proof: SnapshotProof!",
            &format!(
                "$proof: SnapshotProof! = {{ sequence: 2, digest: \"{LEAK}\", nonce: \"n\" }}"
            ),
        ),
        json!({}),
    )
    .await;
    assert_eq!(
        code(&defaulted).as_deref(),
        Some("INVALID_INPUT"),
        "{:?}",
        defaulted.errors
    );
    assert!(!serde_json::to_string(&defaulted)?.contains(LEAK));
    assert_eq!(
        probe.counts(),
        before,
        "none reached a binding, policy or hook"
    );
    assert_eq!(stored_for(&db, "bounded-machine").await?[0].sequence, 1);
    // A value inside both limits is accepted.
    let mut inside = proof(&"y".repeat(300), 8);
    inside["sequence"] = json!(5);
    let fits = run(with_variable, json!({ "proof": inside })).await;
    assert!(fits.errors.is_empty(), "{:?}", fits.errors);
    Ok(())
}

/// An acknowledged retry releases the existing row through the read policy,
/// like any mutation result.
async fn acknowledgement_respects_read_policy(base: &Database<Backend>) -> TestResult {
    let probe = Probe::default();
    let db = configured(base, AuthorizationMode::default(), &probe);
    for who in ["blind-row-machine", "blind-field-machine"] {
        let first = execute(
            &db,
            &probe,
            who,
            &snapshot_mutation("hidden-payload", 1, "h1"),
        )
        .await;
        let retry = execute(
            &db,
            &probe,
            who,
            &snapshot_mutation("hidden-payload", 1, "h1"),
        )
        .await;
        assert_eq!(stored_for(&db, who).await?.len(), 1, "{who}");
        // The retry was acknowledged, and it discloses exactly what the
        // write that created the row disclosed.
        let (first, retry) = (serde_json::to_value(&first)?, serde_json::to_value(&retry)?);
        assert!(
            !retry.to_string().contains("hidden-payload"),
            "{who}: {retry}"
        );
        let result = |response: &Value| response["data"][r("upsertStatusSnapshot")].clone();
        if who == "blind-row-machine" {
            assert_eq!(result(&retry)["success"], json!(true), "{retry}");
            assert_eq!(result(&retry)["action"], Value::Null, "{retry}");
            assert_eq!(result(&retry)[f("statusSnapshot")], Value::Null, "{retry}");
            assert_eq!(result(&first)[f("statusSnapshot")], Value::Null, "{first}");
        } else {
            assert_eq!(retry["errors"], first["errors"], "{who}");
            assert_eq!(
                retry["errors"][0]["extensions"]["code"],
                json!("FORBIDDEN"),
                "{retry}"
            );
        }
    }
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
    for (sequence, outcome) in &outcomes {
        if let Err(code) = outcome {
            assert!(LOSER_CODES.contains(&code.as_str()), "{sequence}: {code}");
        }
    }

    // Two first writes of a key that does not exist yet, held in their
    // `Before` hooks until both have looked the key up (where the backend
    // lets two transactions get that far).
    let race = Race(Arc::new(tokio::sync::Barrier::new(2)));
    let first_writes = graphql_orm::futures::future::join_all((1..=2).map(|sequence| {
        let (db, probe, race) = (db.clone(), probe.clone(), race.clone());
        async move {
            let query = snapshot_mutation(&format!("absent-{sequence}"), sequence, "a");
            let (schema, request) = request(&db, &probe, &query);
            let response = schema
                .execute(request.data(machine("absent-racer")).data(race))
                .await;
            (sequence, code(&response), response.errors.is_empty())
        }
    }))
    .await;
    let absent = stored_for(&db, "absent-racer").await?;
    assert_eq!(absent.len(), 1, "{first_writes:?}");
    let winners = first_writes
        .iter()
        .filter(|(_, _, accepted)| *accepted)
        .map(|(sequence, _, _)| *sequence)
        .collect::<Vec<_>>();
    assert!(!winners.is_empty(), "{first_writes:?}");
    assert_eq!(
        Some(absent[0].sequence),
        winners.iter().copied().max(),
        "{first_writes:?}"
    );
    for (sequence, code, accepted) in &first_writes {
        if !accepted {
            let code = code.as_deref().unwrap_or("no code");
            assert!(LOSER_CODES.contains(&code), "{sequence}: {code}");
        }
    }

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

/// What a request that lost a race may report: the hook's stale-snapshot
/// refusal (`CONFLICT`), the database's serialization failure or deadlock
/// (retryable `SERVICE_UNAVAILABLE`), or its uniqueness violation.
const LOSER_CODES: [&str; 3] = ["CONFLICT", "SERVICE_UNAVAILABLE", "CONSTRAINT_VIOLATION"];

/// Trusted transaction code can run the same hooks with its own transient
/// input; a hook cannot grant itself one.
async fn transaction_code_sets_its_own_transient_input(base: &Database<Backend>) -> TestResult {
    /// Phase, the proof sequence the hook read, and whether its own
    /// attempts to set, take and acknowledge were all refused.
    type Observed = (MutationPhase, Option<i64>, bool);
    struct Seen(Arc<Mutex<Vec<Observed>>>);
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
                        tags: None,
                    })
                    .is_err()
                    && hook_ctx.take_transient_input::<SnapshotProof>().is_err()
                    && !hook_ctx.can_acknowledge_without_write();
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
                tags: None,
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
    updates_cannot_rewrite_the_key(&fixture.db)
        .await
        .map_err(|error| ("update paths", error))?;
    database_equality_cannot_merge_principals(&fixture.db)
        .await
        .map_err(|error| ("database equality", error))?;
    #[cfg(feature = "sqlite")]
    ownership_refusals_are_indistinguishable(&fixture.db)
        .await
        .map_err(|error| ("ownership refusals", error))?;
    transient_argument_is_secret_and_bounded(&fixture.db)
        .await
        .map_err(|error| ("secret and bounded transient input", error))?;
    acknowledgement_respects_read_policy(&fixture.db)
        .await
        .map_err(|error| ("acknowledgement read policy", error))?;
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
