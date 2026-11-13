// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Single-row reads of a composite-key entity under a `Prefilter` read
//! predicate, on every backend: get by key, `NodeId` and Federation
//! `_entities` render the predicate with the backend's placeholders and
//! quoting, then apply the row callback.
//!
//! SQLite runs directly. PostgreSQL and SQL Server use a test-owned container:
//!
//! ```sh
//! cargo test -p graphql-orm --no-default-features --features postgres --test point_read_visibility_backends -- --ignored
//! cargo test -p graphql-orm --no-default-features --features mssql --test point_read_visibility_backends -- --ignored
//! ```

use graphql_orm::prelude::*;
use serde_json::Value;

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
        table = "point_slots",
        plural = "PointSlots",
        node_id,
        federation_key(fields = ["site", "name"])
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "point_slots",
        plural = "PointSlots",
        node_id,
        federation_key(fields = ["site", "name"])
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "point_slots",
        plural = "PointSlots",
        node_id,
        federation_key(fields = ["site", "name"])
    )
)]
struct PointSlot {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    site: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    name: String,
    #[filterable(type = "string")]
    owner: String,
    title: String,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [PointSlot], }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [PointSlot], }
// SQL Server generates no GraphQL mutations for composite keys.
#[cfg(feature = "mssql")]
schema_roots! {
    backend: "mssql",
    schema_policy: "external_writable",
    query_custom_ops: [],
    entities: [PointSlot],
    generated_mutations: "denylist",
    generated_mutation_denylist: [PointSlot],
}

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "mssql")]
type Backend = MssqlBackend;

/// SQL narrows to the caller's rows; the callback also hides "hidden" titles.
struct OwnerPrefilter;

fn subject_id(ctx: Option<&async_graphql::Context<'_>>) -> String {
    ctx.and_then(|ctx| ctx.auth_subject_opt())
        .map(|subject| subject.id)
        .unwrap_or_default()
}

impl RowPolicy for OwnerPrefilter {
    fn read_visibility<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            let filter = PointSlotWhereInput {
                owner: Some(StringFilter {
                    eq: Some(subject_id(ctx)),
                    ..Default::default()
                }),
                ..Default::default()
            };
            Ok(ReadVisibility::Prefilter(ReadPredicate::from_filter::<
                Backend,
                _,
            >(&filter)?))
        })
    }

    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        let visible = row
            .downcast_ref::<PointSlot>()
            .is_some_and(|row| row.title != "hidden");
        Box::pin(async move { Ok(visible) })
    }

    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

struct AllowEntities;

impl EntityPolicy for AllowEntities {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// `(site, name, owner, title)`.
const ROWS: [(&str, &str, &str, &str); 3] = [
    ("north", "a", "alice", "first"),
    ("north", "b", "alice", "hidden"),
    ("south", "a", "bob", "other"),
];

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
    let mut db =
        Database::new(owned_postgres::OwnedPostgres::pool("point-read-visibility", 4).await?);
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "point-read-visibility-init",
                "point read visibility test",
                &[PointSlot::metadata()],
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
            "CREATE TABLE point_slots (site NVARCHAR(64) NOT NULL, name NVARCHAR(64) NOT NULL, \
             owner NVARCHAR(64) NOT NULL, title NVARCHAR(64) NOT NULL, PRIMARY KEY (site, name));",
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
    db.set_entity_policy(AllowEntities);
    db.set_row_policy(OwnerPrefilter);
    // Constant fixture rows, rendered once for every backend.
    let insert = format!(
        "INSERT INTO point_slots (site, name, owner, title) VALUES {}",
        ROWS.iter()
            .map(|(site, name, owner, title)| format!("('{site}', '{name}', '{owner}', '{title}')"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    sqlx::query(&insert).execute(db.pool()).await?;
    #[cfg(feature = "mssql")]
    owned_mssql::execute_batch(&owner.connection_string, &insert).await?;
    Ok(Fixture {
        db,
        #[cfg(feature = "mssql")]
        owner,
    })
}

async fn data(
    db: &Database,
    user: &str,
    query: String,
) -> Result<Value, Box<dyn std::error::Error>> {
    let response = schema_builder(db.clone())
        .finish()
        .execute(async_graphql::Request::new(query.clone()).data(AuthSubject::new(user)))
        .await;
    if !response.errors.is_empty() {
        return Err(format!("{query}: {:?}", response.errors).into());
    }
    Ok(response.data.into_json()?)
}

/// The title each single-row surface releases for `(site, name)`.
async fn point_reads(
    db: &Database,
    user: &str,
    site: &str,
    name: &str,
) -> Result<[Value; 3], Box<dyn std::error::Error>> {
    let title = field_name("title");
    let root = resolver_name("pointSlot");
    let by_key = data(
        db,
        user,
        format!(
            "{{ {root}({}: {site:?}, {}: {name:?}) {{ {title} }} }}",
            argument_name("site"),
            argument_name("name"),
        ),
    )
    .await?[root]
        .clone();
    let by_entity_key = data(
        db,
        user,
        format!(
            "{{ _entities(representations: [{{ __typename: \"PointSlot\", {}: {site:?}, {}: {name:?} }}]) {{ ... on PointSlot {{ {title} }} }} }}",
            field_name("site"),
            field_name("name"),
        ),
    )
    .await?["_entities"][0]
        .clone();
    let node_id = graphql_orm::graphql::federation::encode_node_id(
        "PointSlot",
        vec![serde_json::json!(site), serde_json::json!(name)],
    )
    .0;
    let by_node_id = data(
        db,
        user,
        format!(
            "{{ _entities(representations: [{{ __typename: \"PointSlot\", NodeId: {node_id:?} }}]) {{ ... on PointSlot {{ {title} }} }} }}",
        ),
    )
    .await?["_entities"][0]
        .clone();
    Ok([by_key, by_entity_key, by_node_id]
        .map(|value| value.get(title).cloned().unwrap_or(Value::Null)))
}

async fn prefilter_and_callback_govern_composite_point_reads(db: &Database) -> TestResult {
    let null = [Value::Null, Value::Null, Value::Null];
    let first = Value::from("first");
    assert_eq!(
        point_reads(db, "alice", "north", "a").await?,
        [first.clone(), first.clone(), first]
    );
    // Matched by SQL, refused by the residual callback.
    assert_eq!(point_reads(db, "alice", "north", "b").await?, null);
    // Outside the predicate although the callback would allow it.
    assert_eq!(point_reads(db, "alice", "south", "a").await?, null);
    // Half of another row's key matches no row.
    assert_eq!(point_reads(db, "alice", "south", "b").await?, null);

    let other = Value::from("other");
    assert_eq!(
        point_reads(db, "bob", "south", "a").await?,
        [other.clone(), other.clone(), other]
    );
    assert_eq!(point_reads(db, "bob", "north", "a").await?, null);
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn composite_point_reads_on_sqlite() -> TestResult {
    let fixture = fixture().await?;
    prefilter_and_callback_govern_composite_point_reads(&fixture.db).await?;
    fixture.finish()
}

#[cfg(not(feature = "sqlite"))]
#[tokio::test]
#[ignore = "starts a test-owned database container"]
async fn composite_point_reads_on_an_owned_database() -> TestResult {
    let fixture = fixture().await?;
    prefilter_and_callback_govern_composite_point_reads(&fixture.db).await?;
    fixture.finish()
}
