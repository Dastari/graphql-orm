// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Generated relations that join on a complete tuple whose partition member is
//! a server-only column (#898).
//!
//! A forward and a device each carry a private `tenant_id`. Both relations
//! join on `(device, organisation_id, tenant_id)` and declare the tenant pair
//! `server_only_key`. The tenant column is absent from every public surface,
//! yet every traversal (to-one, nullable, to-many, paged and batched) uses the
//! complete tuple, so a device with the same organisation in another tenant is
//! never related, even for a caller who may read both rows on their own.
//!
//! SQLite runs directly. PostgreSQL and SQL Server use a test-owned container:
//!
//! ```sh
//! cargo test -p graphql-orm --no-default-features --features postgres --test server_only_relation_keys -- --ignored
//! cargo test -p graphql-orm --no-default-features --features mssql --test server_only_relation_keys -- --ignored
//! ```

use graphql_orm::async_graphql::SimpleObject;
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

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "partitioned_devices",
        plural = "PartitionedDevices",
        read_policy = "partitioned.device.read",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "partitioned_devices",
        plural = "PartitionedDevices",
        read_policy = "partitioned.device.read",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "partitioned_devices",
        plural = "PartitionedDevices",
        read_policy = "partitioned.device.read",
        default_sort = "id ASC"
    )
)]
pub struct PartitionedDevice {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[filterable(type = "string")]
    pub organisation_id: String,
    #[graphql(skip)]
    #[graphql_orm(private)]
    pub tenant_id: String,
    #[filterable(type = "string")]
    pub label: String,
    pub blocked: bool,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "PartitionedForward",
        from = ["id", "organisation_id", "tenant_id"],
        to = ["device_id", "organisation_id", "tenant_id"],
        server_only_key(from = "tenant_id", to = "tenant_id"),
        multiple,
        emit_fk = false
    )]
    pub forwards: Vec<PartitionedForward>,
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "partitioned_forwards",
        plural = "PartitionedForwards",
        read_policy = "partitioned.forward.read",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "partitioned_forwards",
        plural = "PartitionedForwards",
        read_policy = "partitioned.forward.read",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "partitioned_forwards",
        plural = "PartitionedForwards",
        read_policy = "partitioned.forward.read",
        default_sort = "id ASC"
    )
)]
pub struct PartitionedForward {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[filterable(type = "string")]
    pub organisation_id: String,
    #[graphql(skip)]
    #[graphql_orm(private)]
    pub tenant_id: String,
    #[filterable(type = "string")]
    pub device_id: Option<String>,
    #[filterable(type = "string")]
    #[sortable]
    pub label: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "PartitionedDevice",
        from = ["device_id", "organisation_id", "tenant_id"],
        to = ["id", "organisation_id", "tenant_id"],
        server_only_key(from = "tenant_id", to = "tenant_id"),
        emit_fk = false
    )]
    pub device: Option<PartitionedDevice>,
}

// Key members whose database equality differs from their text: collation and
// padding (every backend), SQLite numeric affinity, and a non-canonically
// stored SQLite UUID. The loader and the direct path must agree on each.
#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "keyed_targets",
        plural = "KeyedTargets",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "keyed_targets",
        plural = "KeyedTargets",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "keyed_targets",
        plural = "KeyedTargets",
        default_sort = "id ASC"
    )
)]
pub struct KeyedTarget {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[graphql(skip)]
    #[graphql_orm(private)]
    pub part: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "KeyedSource",
        from = ["id", "part"],
        to = ["target_id", "part"],
        server_only_key(from = "part", to = "part"),
        multiple,
        emit_fk = false
    )]
    pub sources: Vec<KeyedSource>,
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "keyed_sources",
        plural = "KeyedSources",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "keyed_sources",
        plural = "KeyedSources",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "keyed_sources",
        plural = "KeyedSources",
        default_sort = "id ASC"
    )
)]
pub struct KeyedSource {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[filterable(type = "string")]
    pub target_id: String,
    #[graphql(skip)]
    #[graphql_orm(private)]
    pub part: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "KeyedTarget",
        from = ["target_id", "part"],
        to = ["id", "part"],
        server_only_key(from = "part", to = "part"),
        emit_fk = false
    )]
    pub target: Option<KeyedTarget>,
}

#[cfg(feature = "sqlite")]
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "real_targets",
        plural = "RealTargets",
        default_sort = "id ASC"
    )
)]
pub struct RealTarget {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[graphql_orm(private)]
    pub part: f64,
}

#[cfg(feature = "sqlite")]
#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "real_sources",
        plural = "RealSources",
        default_sort = "id ASC"
    )
)]
pub struct RealSource {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[filterable(type = "string")]
    pub target_id: String,
    #[graphql(skip)]
    #[graphql_orm(private)]
    pub part: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "RealTarget",
        from = ["target_id", "part"],
        to = ["id", "part"],
        server_only_key(from = "part", to = "part"),
        emit_fk = false
    )]
    pub target: Option<RealTarget>,
}

#[cfg(feature = "sqlite")]
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "uuid_keyed_targets",
        plural = "UuidTargets",
        default_sort = "id ASC"
    )
)]
pub struct UuidTarget {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[graphql_orm(private)]
    pub part: graphql_orm::uuid::Uuid,
}

#[cfg(feature = "sqlite")]
#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "uuid_keyed_sources",
        plural = "UuidSources",
        default_sort = "id ASC"
    )
)]
pub struct UuidSource {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[filterable(type = "string")]
    pub target_id: String,
    #[graphql(skip)]
    #[graphql_orm(private)]
    pub part: graphql_orm::uuid::Uuid,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "UuidTarget",
        from = ["target_id", "part"],
        to = ["id", "part"],
        server_only_key(from = "part", to = "part"),
        emit_fk = false
    )]
    pub target: Option<UuidTarget>,
}

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [
        PartitionedDevice,
        PartitionedForward,
        KeyedTarget,
        KeyedSource,
        RealTarget,
        RealSource,
        UuidTarget,
        UuidSource,
    ],
    client_metadata: true,
}
#[cfg(feature = "postgres")]
schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [PartitionedDevice, PartitionedForward, KeyedTarget, KeyedSource],
    client_metadata: true,
}
#[cfg(feature = "mssql")]
schema_roots! {
    backend: "mssql",
    schema_policy: "external_writable",
    query_custom_ops: [],
    entities: [PartitionedDevice, PartitionedForward, KeyedTarget, KeyedSource],
    client_metadata: true,
}

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "mssql")]
type Backend = MssqlBackend;

const TENANT_A: &str = "tenant-a";
const TENANT_B: &str = "tenant-b";
const ORGANISATION: &str = "org-shared";

/// Callers and the tenants each is admitted to. `no-devices` is admitted to
/// both tenants but denied the device entity.
fn admitted_tenants(subject: &str) -> &'static [&'static str] {
    match subject {
        "both" | "no-devices" | "no-labels" => &[TENANT_A, TENANT_B],
        "only-a" => &[TENANT_A],
        _ => &[],
    }
}

fn subject(ctx: Option<&async_graphql::Context<'_>>) -> String {
    ctx.and_then(|ctx| ctx.auth_subject_opt())
        .map(|subject| subject.id)
        .unwrap_or_default()
}

/// Complete SQL visibility over the private tenant column, plus a blocked
/// flag on devices. No callback can widen it.
struct TenantRows;

/// `tenant_id` is one of `tenants`. Every caller in this fixture is admitted
/// to at least one tenant.
fn tenant_predicate<T: Entity + 'static>(tenants: &[&str]) -> async_graphql::Result<ReadPredicate> {
    let tenant = |tenant: &str| {
        ReadPredicate::column::<Backend, T>(
            "tenant_id",
            ReadPredicateOp::Eq,
            SqlValue::String(tenant.to_owned()),
        )
    };
    let (first, rest) = tenants
        .split_first()
        .ok_or_else(|| async_graphql::Error::new("no admitted tenant"))?;
    rest.iter().try_fold(tenant(first)?, |predicate, next| {
        predicate.or(tenant(next)?)
    })
}

impl RowPolicy<Backend> for TenantRows {
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
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        let tenants = admitted_tenants(&subject(ctx));
        Box::pin(async move {
            let predicate =
                match entity {
                    "PartitionedDevice" => tenant_predicate::<PartitionedDevice>(tenants)?.and(
                        ReadPredicate::column::<Backend, PartitionedDevice>(
                            "blocked",
                            ReadPredicateOp::Eq,
                            SqlValue::Bool(false),
                        )?,
                    )?,
                    "PartitionedForward" => tenant_predicate::<PartitionedForward>(tenants)?,
                    _ => return Err(async_graphql::Error::new("unexpected entity")),
                };
            Ok(ReadVisibility::Complete(predicate))
        })
    }

    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        // Complete visibility never reaches the callback; refuse if it does.
        Box::pin(async { Ok(false) })
    }

    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

/// Every entity is readable, except devices for `no-devices`.
struct Entities;

impl EntityPolicy<Backend> for Entities {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        entity: &'static str,
        _: Option<&'static str>,
        kind: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        let allowed = kind == EntityAccessKind::Read
            && !(entity == "PartitionedDevice" && subject(ctx) == "no-devices");
        Box::pin(async move { Ok(allowed) })
    }
}

/// Every field is readable, except a device's label for `no-labels`.
struct Fields;

impl FieldPolicy<Backend> for Fields {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database<Backend>,
        entity: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        let allowed = !(entity == "PartitionedDevice"
            && field.eq_ignore_ascii_case("label")
            && subject(Some(ctx)) == "no-labels");
        Box::pin(async move { Ok(allowed) })
    }

    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

/// `(id, organisation_id, tenant_id, label, blocked)`.
const DEVICES: [(&str, &str, &str, &str, bool); 3] = [
    ("dev-a", ORGANISATION, TENANT_A, "device in a", false),
    ("dev-b", ORGANISATION, TENANT_B, "device in b", false),
    (
        "dev-blocked",
        ORGANISATION,
        TENANT_A,
        "blocked device",
        true,
    ),
];

/// `(id, organisation_id, tenant_id, device_id, label)`.
const FORWARDS: [(&str, &str, &str, Option<&str>, &str); 7] = [
    // Positive control: the same tenant and organisation.
    (
        "fwd-same",
        ORGANISATION,
        TENANT_A,
        Some("dev-a"),
        "same partition",
    ),
    // The regression: tenant A stores the ID of a device in tenant B.
    (
        "fwd-cross",
        ORGANISATION,
        TENANT_A,
        Some("dev-b"),
        "cross partition",
    ),
    // The target row exists in the same partition but is denied.
    (
        "fwd-denied",
        ORGANISATION,
        TENANT_A,
        Some("dev-blocked"),
        "denied target",
    ),
    // A nullable key member: no relation.
    ("fwd-unlinked", ORGANISATION, TENANT_A, None, "no device"),
    // A populated key whose target does not exist.
    (
        "fwd-missing",
        ORGANISATION,
        TENANT_A,
        Some("dev-ghost"),
        "missing target",
    ),
    // A second same-partition forward, so windows can exclude a match.
    (
        "fwd-same-2",
        ORGANISATION,
        TENANT_A,
        Some("dev-a"),
        "same partition two",
    ),
    ("fwd-b", ORGANISATION, TENANT_B, Some("dev-b"), "tenant b"),
];

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

fn boolean(value: bool) -> &'static str {
    match (cfg!(feature = "postgres"), value) {
        (true, true) => "TRUE",
        (true, false) => "FALSE",
        (false, true) => "1",
        (false, false) => "0",
    }
}

fn quoted(value: Option<&str>) -> String {
    value.map_or_else(|| "NULL".to_owned(), |value| format!("'{value}'"))
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    // The pool keeps the owned container alive; it is removed when the last clone drops.
    #[cfg(feature = "postgres")]
    let db =
        Database::new(owned_postgres::OwnedPostgres::pool("server-only-relation-keys", 4).await?);
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "server-only-relation-keys-init",
                "server-only relation keys test",
                &[
                    PartitionedDevice::metadata(),
                    PartitionedForward::metadata(),
                    KeyedTarget::metadata(),
                    KeyedSource::metadata(),
                    #[cfg(feature = "sqlite")]
                    RealTarget::metadata(),
                    #[cfg(feature = "sqlite")]
                    RealSource::metadata(),
                    #[cfg(feature = "sqlite")]
                    UuidTarget::metadata(),
                    #[cfg(feature = "sqlite")]
                    UuidSource::metadata(),
                ],
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
            "CREATE TABLE partitioned_devices (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             organisation_id NVARCHAR(64) NOT NULL, tenant_id NVARCHAR(64) NOT NULL, \
             label NVARCHAR(64) NOT NULL, blocked BIT NOT NULL); \
             CREATE TABLE partitioned_forwards (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             organisation_id NVARCHAR(64) NOT NULL, tenant_id NVARCHAR(64) NOT NULL, \
             device_id NVARCHAR(64) NULL, label NVARCHAR(64) NOT NULL); \
             CREATE TABLE keyed_targets (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             part NVARCHAR(64) NOT NULL); \
             CREATE TABLE keyed_sources (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             target_id NVARCHAR(64) NOT NULL, part NVARCHAR(64) NOT NULL);",
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
    // Constant fixture rows, rendered once for every backend.
    let devices = format!(
        "INSERT INTO partitioned_devices (id, organisation_id, tenant_id, label, blocked) VALUES {}",
        DEVICES
            .iter()
            .map(|(id, organisation, tenant, label, blocked)| format!(
                "('{id}', '{organisation}', '{tenant}', '{label}', {})",
                boolean(*blocked)
            ))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let forwards = format!(
        "INSERT INTO partitioned_forwards (id, organisation_id, tenant_id, device_id, label) VALUES {}",
        FORWARDS
            .iter()
            .map(|(id, organisation, tenant, device, label)| format!(
                "('{id}', '{organisation}', '{tenant}', {}, '{label}')",
                quoted(*device)
            ))
            .collect::<Vec<_>>()
            .join(", ")
    );
    // One target; sources whose partition equals it exactly, by case only,
    // by trailing spaces only, and not at all.
    let keyed = "INSERT INTO keyed_targets (id, part) VALUES ('t', 'Tenant')".to_owned();
    let keyed_sources = "INSERT INTO keyed_sources (id, target_id, part) VALUES \
        ('s-exact', 't', 'Tenant'), ('s-case', 't', 'TENANT'), \
        ('s-pad', 't', 'Tenant  '), ('s-other', 't', 'Other')"
        .to_owned();
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    for statement in [devices, forwards, keyed, keyed_sources] {
        sqlx::query(&statement).execute(db.pool()).await?;
    }
    // A REAL target partition that SQLite matches to TEXT by numeric
    // affinity, and that renders as text with fewer digits than it holds.
    // A UUID partition stored in upper case is not canonical.
    #[cfg(feature = "sqlite")]
    for statement in [
        "INSERT INTO real_targets (id, part) VALUES ('t', 0.1000000000000001)",
        "INSERT INTO real_sources (id, target_id, part) VALUES \
         ('s-exact', 't', '0.1000000000000001'), ('s-rounded', 't', '0.1')",
        "INSERT INTO uuid_keyed_targets (id, part) VALUES \
         ('t-upper', 'AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE'), \
         ('t-lower', 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee')",
        "INSERT INTO uuid_keyed_sources (id, target_id, part) VALUES \
         ('s-upper', 't-upper', 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee'), \
         ('s-lower', 't-lower', 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee'), \
         ('s-upper-source', 't-lower', 'AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE')",
    ] {
        sqlx::query(statement).execute(db.pool()).await?;
    }
    #[cfg(feature = "mssql")]
    owned_mssql::execute_batch(
        &owner.connection_string,
        &format!("{devices}; {forwards}; {keyed}; {keyed_sources};"),
    )
    .await?;
    Ok(Fixture {
        db,
        #[cfg(feature = "mssql")]
        owner,
    })
}

/// A database clone with the policies installed under `mode`.
fn configured(db: &Database<Backend>, mode: AuthorizationMode) -> Database<Backend> {
    let mut db = db.clone().with_authorization_mode(mode);
    db.set_entity_policy(Entities);
    db.set_row_policy(TenantRows);
    db.set_field_policy(Fields);
    db
}

/// A database and the resolver path its requests take: the generated schema
/// with its relation `DataLoader`s, or a hand-built schema without them,
/// where every relation resolves with its own direct query.
struct Api {
    db: Database<Backend>,
    direct: bool,
}

impl Api {
    fn label(&self) -> &'static str {
        if self.direct { "direct" } else { "loader" }
    }
}

async fn execute(api: &Api, user: &str, query: &str) -> async_graphql::Response {
    let request = async_graphql::Request::new(query).data(AuthSubject::new(user));
    if api.direct {
        AppSchema::build(QueryRoot::default(), Default::default(), Default::default())
            .data(api.db.clone())
            .data(graphql_orm::graphql::loaders::DirectRelationQueries)
            .data(graphql_orm::graphql::orm::DeclaredPageMaximum(
                GRAPHQL_ORM_MAX_PAGE_LIMIT,
            ))
            .extension(graphql_orm::graphql::orm::PageMaximumValidation)
            .extension(graphql_orm::graphql::input_validation::GeneratedInputValidation)
            .finish()
            .execute(request)
            .await
    } else {
        schema_builder(api.db.clone())
            .finish()
            .execute(request)
            .await
    }
}

async fn data(api: &Api, user: &str, query: String) -> Result<Value, String> {
    let response = execute(api, user, &query).await;
    if !response.errors.is_empty() {
        return Err(format!("{}: {query}: {:?}", api.label(), response.errors));
    }
    response.data.into_json().map_err(|error| error.to_string())
}

fn forward_query(forward: &str, device_fields: &str) -> String {
    format!(
        "{{ {}({}: {forward:?}) {{ {} {{ {device_fields} }} }} }}",
        r("partitionedForward"),
        a("id"),
        f("device"),
    )
}

/// The forward `id` and its related device id (or null), through the root
/// single read.
async fn forward_device(api: &Api, user: &str, forward: &str) -> Result<Value, String> {
    let root = r("partitionedForward");
    let value = data(
        api,
        user,
        format!(
            "{{ {root}({}: {forward:?}) {{ {} {} {{ {} {} }} }} }}",
            a("id"),
            f("id"),
            f("device"),
            f("id"),
            f("label"),
        ),
    )
    .await?;
    Ok(value[root].clone())
}

async fn device_by_id(api: &Api, user: &str, device: &str) -> Result<Value, String> {
    let root = r("partitionedDevice");
    let value = data(
        api,
        user,
        format!("{{ {root}({}: {device:?}) {{ {} }} }}", a("id"), f("id")),
    )
    .await?;
    Ok(value[root][f("id")].clone())
}

fn device_of(forward: &Value) -> Value {
    forward[f("device")]
        .get(f("id"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The related forward ids of one device and the connection's total, with
/// optional relation arguments (`where`, `orderBy`, `page`).
async fn device_forwards(
    api: &Api,
    user: &str,
    device: &str,
    arguments: &str,
) -> Result<(Vec<String>, i64), String> {
    let root = r("partitionedDevice");
    let arguments = if arguments.is_empty() {
        String::new()
    } else {
        format!("({arguments})")
    };
    let value = data(
        api,
        user,
        format!(
            "{{ {root}({}: {device:?}) {{ {}{arguments} {{ edges {{ node {{ {} }} }} pageInfo {{ totalCount }} }} }} }}",
            a("id"),
            f("forwards"),
            f("id"),
        ),
    )
    .await?;
    let connection = &value[root][f("forwards")];
    let ids = connection["edges"]
        .as_array()
        .ok_or_else(|| format!("no edges: {value}"))?
        .iter()
        .map(|edge| {
            edge["node"][f("id")]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    let total = connection["pageInfo"]["totalCount"].as_i64().unwrap_or(-1);
    Ok((ids, total))
}

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// The complete response, `data` and `errors`, as one comparable value.
async fn whole_response(api: &Api, user: &str, query: &str) -> TestResult<Value> {
    let response = execute(api, user, query).await;
    Ok(json!({
        "data": response.data.into_json()?,
        "errors": serde_json::to_value(&response.errors)?,
    }))
}

const MODES: [AuthorizationMode; 3] = [
    AuthorizationMode::LegacyPermissive,
    AuthorizationMode::DeclaredPoliciesRequired,
    AuthorizationMode::ExplicitPolicyForAllExposedOperations,
];

async fn complete_tuple_joins_in_every_mode(base: &Database<Backend>) -> TestResult {
    for mode in MODES {
        for direct in [false, true] {
            let api = Api {
                db: configured(base, mode),
                direct,
            };
            complete_tuple_joins(&api, mode).await?;
        }
    }
    Ok(())
}

async fn complete_tuple_joins(api: &Api, mode: AuthorizationMode) -> TestResult {
    let context = |what: &str| format!("{mode:?}/{}: {what}", api.label());

    // Independent read admission to both rows of the regression pair.
    assert_eq!(
        forward_device(api, "both", "fwd-cross").await?[f("id")],
        json!("fwd-cross"),
        "{}",
        context("forward readable")
    );
    assert_eq!(
        device_by_id(api, "both", "dev-b").await?,
        json!("dev-b"),
        "{}",
        context("device readable")
    );
    // Same organisation, device ID stored on the forward, other tenant: null.
    assert_eq!(
        device_of(&forward_device(api, "both", "fwd-cross").await?),
        Value::Null,
        "{}",
        context("cross-partition relation")
    );
    // Positive control.
    let same = forward_device(api, "both", "fwd-same").await?;
    assert_eq!(
        device_of(&same),
        json!("dev-a"),
        "{}",
        context("same partition")
    );
    assert_eq!(same[f("device")][f("label")], json!("device in a"));
    // A null key member.
    assert_eq!(
        device_of(&forward_device(api, "both", "fwd-unlinked").await?),
        Value::Null,
        "{}",
        context("nullable key")
    );
    // A mismatched partition, a row-denied same-partition target and a
    // genuinely missing target with a populated key are one response, data
    // and errors alike.
    let device_fields = format!("{} {}", f("id"), f("label"));
    let missing =
        whole_response(api, "both", &forward_query("fwd-missing", &device_fields)).await?;
    assert_eq!(
        missing,
        json!({ "data": { r("partitionedForward"): { f("device"): null } }, "errors": [] }),
        "{}",
        context("missing target")
    );
    for forward in ["fwd-cross", "fwd-denied"] {
        assert_eq!(
            whole_response(api, "both", &forward_query(forward, &device_fields)).await?,
            missing,
            "{}",
            context(forward)
        );
    }
    // A caller admitted only to tenant A cannot see the B device at all.
    assert_eq!(device_by_id(api, "only-a", "dev-b").await?, Value::Null);
    assert_eq!(
        device_of(&forward_device(api, "only-a", "fwd-cross").await?),
        Value::Null
    );
    assert_eq!(
        device_of(&forward_device(api, "only-a", "fwd-same").await?),
        json!("dev-a")
    );

    // One list read resolves every forward's relation.
    let root = r("partitionedForwards");
    let listed = data(
        api,
        "both",
        format!(
            "{{ {root} {{ edges {{ node {{ {} {} {{ {} }} }} }} }} }}",
            f("id"),
            f("device"),
            f("id"),
        ),
    )
    .await?;
    let related = listed[root]["edges"]
        .as_array()
        .ok_or("no forwards")?
        .iter()
        .map(|edge| {
            (
                edge["node"][f("id")]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                device_of(&edge["node"]),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        related,
        vec![
            ("fwd-b".to_owned(), json!("dev-b")),
            ("fwd-cross".to_owned(), Value::Null),
            ("fwd-denied".to_owned(), Value::Null),
            ("fwd-missing".to_owned(), Value::Null),
            ("fwd-same".to_owned(), json!("dev-a")),
            ("fwd-same-2".to_owned(), json!("dev-a")),
            ("fwd-unlinked".to_owned(), Value::Null),
        ],
        "{}",
        context("list to-one")
    );

    // To-many: fwd-cross stores dev-b's ID but is in tenant A, so it belongs
    // to neither device's partition, with or without relation arguments.
    let label = f("label");
    let page = a("page");
    let filter = a("where");
    let order = a("orderBy");
    let windows: [(&str, &str, Vec<String>, i64); 9] = [
        ("dev-a", "", ids(&["fwd-same", "fwd-same-2"]), 2),
        ("dev-b", "", ids(&["fwd-b"]), 1),
        (
            "dev-a",
            &format!("{page}: {{ limit: 10, offset: 0 }}"),
            ids(&["fwd-same", "fwd-same-2"]),
            2,
        ),
        // A page window that excludes a matching row.
        (
            "dev-a",
            &format!("{page}: {{ limit: 1, offset: 1 }}"),
            ids(&["fwd-same-2"]),
            2,
        ),
        // A filter that excludes a matching row.
        (
            "dev-a",
            &format!("{filter}: {{ {label}: {{ ne: \"same partition\" }} }}"),
            ids(&["fwd-same-2"]),
            1,
        ),
        // An explicit order, then a window over it.
        (
            "dev-a",
            &format!("{order}: {{ {label}: DESC }}"),
            ids(&["fwd-same-2", "fwd-same"]),
            2,
        ),
        (
            "dev-a",
            &format!("{order}: {{ {label}: DESC }}, {page}: {{ limit: 1, offset: 0 }}"),
            ids(&["fwd-same-2"]),
            2,
        ),
        // The public members alone would match fwd-cross here.
        (
            "dev-b",
            &format!("{filter}: {{ {label}: {{ eq: \"cross partition\" }} }}"),
            Vec::new(),
            0,
        ),
        (
            "dev-b",
            &format!("{order}: {{ {label}: ASC }}, {page}: {{ limit: 5, offset: 0 }}"),
            ids(&["fwd-b"]),
            1,
        ),
    ];
    for (device, arguments, expected, total) in windows {
        assert_eq!(
            device_forwards(api, "both", device, arguments).await?,
            (expected, total),
            "{}",
            context(&format!("to-many {device}({arguments})"))
        );
    }

    // An entity-level target denial is the documented FORBIDDEN refusal on
    // the relation path; the parent stays and nothing names the tenant.
    let response = execute(
        api,
        "no-devices",
        &format!(
            "{{ {}({}: \"fwd-same\") {{ {} {} {{ {} }} }} }}",
            r("partitionedForward"),
            a("id"),
            f("id"),
            f("device"),
            f("id"),
        ),
    )
    .await;
    let errors = format!("{:?}", response.errors).to_ascii_lowercase();
    assert!(
        errors.contains("forbidden"),
        "{}: {errors}",
        context("entity denial")
    );
    assert!(!errors.contains("tenant"), "{errors}");
    let value = response.data.into_json()?;
    assert_eq!(value[r("partitionedForward")][f("id")], json!("fwd-same"));
    assert_eq!(device_of(&value[r("partitionedForward")]), Value::Null);

    // The relation reveals a match on a target row the caller may read, by
    // definition: selecting only `__typename` tells a same-partition target
    // from a mismatch without any field check. A selected-field denial is
    // therefore the ordinary relation refusal, never turned into null, and
    // row visibility is what hides a target.
    let typename = |forward: &str| forward_query(forward, "__typename");
    assert_eq!(
        whole_response(api, "no-labels", &typename("fwd-same")).await?["data"],
        json!({ r("partitionedForward"): { f("device"): { "__typename": "PartitionedDevice" } } }),
        "{}",
        context("typename match")
    );
    assert_eq!(
        whole_response(api, "no-labels", &typename("fwd-cross")).await?,
        json!({ "data": { r("partitionedForward"): { f("device"): null } }, "errors": [] }),
        "{}",
        context("typename mismatch")
    );
    let denied = execute(api, "no-labels", &forward_query("fwd-same", &device_fields)).await;
    let errors = format!("{:?}", denied.errors).to_ascii_lowercase();
    assert!(
        errors.contains("forbidden"),
        "{}: {errors}",
        context("field denial")
    );
    assert!(!errors.contains("tenant"), "{errors}");
    Ok(())
}

/// Each source's related target id (or null), from one list read.
async fn related_targets(api: &Api, root: &str) -> TestResult<Vec<(String, Value)>> {
    let root = r(root);
    let value = data(
        api,
        "both",
        format!(
            "{{ {root} {{ edges {{ node {{ {} {} {{ {} }} }} }} }} }}",
            f("id"),
            f("target"),
            f("id"),
        ),
    )
    .await?;
    Ok(value[root]["edges"]
        .as_array()
        .ok_or("no sources")?
        .iter()
        .map(|edge| {
            (
                edge["node"][f("id")]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                edge["node"][f("target")]
                    .get(f("id"))
                    .cloned()
                    .unwrap_or(Value::Null),
            )
        })
        .collect())
}

fn pairs(expected: &[(&str, Option<&str>)]) -> Vec<(String, Value)> {
    expected
        .iter()
        .map(|(source, target)| {
            (
                (*source).to_owned(),
                target.map_or(Value::Null, |id| json!(id)),
            )
        })
        .collect()
}

/// The batched loader associates rows by the database's own match of the
/// complete tuple, so it agrees with the direct relation query wherever
/// database equality differs from the key's text.
async fn loader_agrees_with_database_equality(base: &Database<Backend>) -> TestResult {
    let api = |direct| Api {
        db: base.clone(),
        direct,
    };
    // SQL Server's default collation ignores case and trailing spaces; the
    // other backends compare exactly.
    let collated = cfg!(feature = "mssql");
    let keyed = if collated {
        pairs(&[
            ("s-case", Some("t")),
            ("s-exact", Some("t")),
            ("s-other", None),
            ("s-pad", Some("t")),
        ])
    } else {
        pairs(&[
            ("s-case", None),
            ("s-exact", Some("t")),
            ("s-other", None),
            ("s-pad", None),
        ])
    };
    for direct in [false, true] {
        assert_eq!(
            related_targets(&api(direct), "keyedSources").await?,
            keyed,
            "direct={direct}"
        );
    }
    let keyed_many = if collated {
        (ids(&["s-case", "s-exact", "s-pad"]), 3)
    } else {
        (ids(&["s-exact"]), 1)
    };
    for direct in [false, true] {
        for arguments in ["", "page: { limit: 10, offset: 0 }"] {
            let arguments = arguments.replace("page", a("page"));
            let root = r("keyedTarget");
            let value = data(
                &api(direct),
                "both",
                format!(
                    "{{ {root}({}: \"t\") {{ {}{} {{ edges {{ node {{ {} }} }} pageInfo {{ totalCount }} }} }} }}",
                    a("id"),
                    f("sources"),
                    if arguments.is_empty() { String::new() } else { format!("({arguments})") },
                    f("id"),
                ),
            )
            .await?;
            let connection = &value[root][f("sources")];
            let found = connection["edges"]
                .as_array()
                .ok_or("no edges")?
                .iter()
                .map(|edge| {
                    edge["node"][f("id")]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect::<Vec<_>>();
            assert_eq!(
                (
                    found,
                    connection["pageInfo"]["totalCount"].as_i64().unwrap_or(-1)
                ),
                keyed_many.clone(),
                "direct={direct} {arguments}"
            );
        }
    }
    #[cfg(feature = "sqlite")]
    for direct in [false, true] {
        // The REAL partition matches only its exact numeric text; the
        // rounded spelling is another value and is never given the row.
        assert_eq!(
            related_targets(&api(direct), "realSources").await?,
            pairs(&[("s-exact", Some("t")), ("s-rounded", None)]),
            "direct={direct}"
        );
        // UUID members compare by decoded value: a non-canonically stored
        // source is bound in canonical form and matches its target by value,
        // while a non-canonically stored target matches nothing.
        assert_eq!(
            related_targets(&api(direct), "uuidSources").await?,
            pairs(&[
                ("s-lower", Some("t-lower")),
                ("s-upper", None),
                ("s-upper-source", Some("t-lower")),
            ]),
            "direct={direct}"
        );
    }
    Ok(())
}

async fn private_key_has_no_public_surface(base: &Database<Backend>) -> TestResult {
    let db = configured(base, AuthorizationMode::default());
    let sdl = schema_builder(db.clone())
        .finish()
        .sdl()
        .to_ascii_lowercase();
    assert!(sdl.contains("organisation"), "fixture sanity: {sdl}");
    assert!(!sdl.contains("tenant"), "private key in SDL:\n{sdl}");

    // Selecting, filtering and ordering by it are all unknown to the schema.
    for query in [
        format!(
            "{{ {} {{ edges {{ node {{ tenantId }} }} }} }}",
            r("partitionedForwards")
        ),
        format!(
            "{{ {}({}: {{ tenantId: {{ eq: \"{TENANT_B}\" }} }}) {{ edges {{ node {{ {} }} }} }} }}",
            r("partitionedForwards"),
            a("where"),
            f("id"),
        ),
        format!(
            "{{ {}({}: [{{ tenantId: ASC }}]) {{ edges {{ node {{ {} }} }} }} }}",
            r("partitionedDevices"),
            a("orderBy"),
            f("id"),
        ),
        format!(
            "{{ {}({}: \"dev-a\") {{ {}({}: {{ tenantId: {{ eq: \"{TENANT_B}\" }} }}) {{ edges {{ node {{ {} }} }} }} }} }}",
            r("partitionedDevice"),
            a("id"),
            f("forwards"),
            a("where"),
            f("id"),
        ),
    ] {
        let response = execute(
            &Api {
                db: db.clone(),
                direct: false,
            },
            "both",
            &query,
        )
        .await;
        assert!(!response.errors.is_empty(), "{query} was accepted");
        assert_eq!(response.data.into_json()?, Value::Null, "{query}");
    }

    // Descriptive catalogues never name it either.
    let catalogues = [
        serde_json::to_string(graphql_orm_semantic_catalog())?,
        serde_json::to_string(graphql_orm_resolver_catalog().map_err(|error| error.to_string())?)?,
        serde_json::to_string(graphql_orm_client_metadata())?,
    ];
    for catalogue in catalogues {
        assert!(
            !catalogue.to_ascii_lowercase().contains("tenant"),
            "private key in catalogue: {catalogue}"
        );
    }
    // The operation catalogue publishes no tenant field, argument, type or
    // description. Its host-side schema signature, a fingerprint input that
    // lists every persisted field, records the column as private and without
    // any surface.
    for operation in graphql_orm_operation_catalog().operations() {
        let generated = operation.generated();
        let public = std::iter::once(generated.field_name().to_owned())
            .chain([
                generated.description().to_owned(),
                generated.graphql_result_type().to_owned(),
            ])
            .chain(generated.arguments().iter().flat_map(|argument| {
                [
                    argument.graphql_name().to_owned(),
                    argument.description().to_owned(),
                    argument.graphql_type().to_owned(),
                ]
            }))
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        assert!(!public.contains("tenant"), "{public}");
        for line in generated
            .schema_signature()
            .lines()
            .filter(|line| line.contains("tenant"))
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
    // The public half of the key does not make the relation batchable by
    // clients: its key mapping stays unexposed.
    let metadata = serde_json::to_string(graphql_orm_client_metadata())?;
    assert!(!metadata.contains("\"batchable\""), "{metadata}");
    assert!(metadata.contains("\"no_exposed_key\""), "{metadata}");
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn server_only_partition_keys_on_sqlite() -> TestResult {
    let fixture = fixture().await?;
    complete_tuple_joins_in_every_mode(&fixture.db).await?;
    loader_agrees_with_database_equality(&fixture.db).await?;
    private_key_has_no_public_surface(&fixture.db).await?;
    fixture.finish()
}

#[cfg(not(feature = "sqlite"))]
#[tokio::test]
#[ignore = "starts a test-owned database container"]
async fn server_only_partition_keys_on_an_owned_database() -> TestResult {
    let fixture = fixture().await?;
    complete_tuple_joins_in_every_mode(&fixture.db).await?;
    loader_agrees_with_database_equality(&fixture.db).await?;
    private_key_has_no_public_surface(&fixture.db).await?;
    fixture.finish()
}
