#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Generated `<Entity>Changed(Keys: [...])` observes a bounded set of complete keys.
//!
//! SQLite runs every scenario as its own test. PostgreSQL and SQL Server run
//! all scenarios in one ignored test against a single test-owned container,
//! resetting the tables between scenarios:
//!
//! ```sh
//! cargo test -p graphql-orm --no-default-features --features postgres --test keyed_changed_subscriptions -- --ignored
//! cargo test -p graphql-orm --no-default-features --features mssql --test keyed_changed_subscriptions -- --ignored
//! ```

use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "keyed_parents", plural = "KeyedParents")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "keyed_parents", plural = "KeyedParents")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "keyed_parents",
        plural = "KeyedParents"
    )
)]
struct KeyedParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    owner: String,
    label: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "keyed_children", plural = "KeyedChildren")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "keyed_children",
        plural = "KeyedChildren"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "keyed_children",
        plural = "KeyedChildren"
    )
)]
struct KeyedChild {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    parent_id: String,
    body: String,
    #[graphql(skip)]
    #[relation(
        target = "KeyedParent",
        from = "parent_id",
        to = "id",
        propagate_change = "up"
    )]
    parent: Option<KeyedParent>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "keyed_slots",
        plural = "KeyedSlots",
        repository_mutations = true
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "keyed_slots",
        plural = "KeyedSlots",
        repository_mutations = true
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "keyed_slots",
        plural = "KeyedSlots",
        repository_mutations = true
    )
)]
struct KeyedSlot {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    site: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    name: String,
    owner: String,
    label: String,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [KeyedParent, KeyedChild, KeyedSlot], }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [KeyedParent, KeyedChild, KeyedSlot], }
// SQL Server generates no GraphQL mutations for composite keys; the test writes
// slots through the repository on every backend.
#[cfg(feature = "mssql")]
schema_roots! {
    backend: "mssql",
    schema_policy: "external_writable",
    query_custom_ops: [],
    entities: [KeyedParent, KeyedChild, KeyedSlot],
    generated_mutations: "denylist",
    generated_mutation_denylist: [KeyedSlot],
}

struct AllowEntityPolicy;

impl EntityPolicy for AllowEntityPolicy {
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

/// Parents and slots are readable only by their owner; children by anyone.
struct OwnerRowPolicy;

impl RowPolicy for OwnerRowPolicy {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let subject = ctx
                .and_then(|ctx| ctx.auth_subject_opt())
                .map(|s| s.id.clone());
            let owner = row
                .downcast_ref::<KeyedParent>()
                .map(|row| row.owner.clone())
                .or_else(|| row.downcast_ref::<KeyedSlot>().map(|row| row.owner.clone()));
            Ok(match owner {
                Some(owner) => subject.as_deref() == Some(owner.as_str()),
                None => true,
            })
        })
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

#[cfg(not(feature = "sqlite"))]
const TABLES: [&str; 3] = ["keyed_children", "keyed_parents", "keyed_slots"];

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
    let mut db = Database::new(owned_postgres::OwnedPostgres::pool("keyed-changed", 4).await?);
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "keyed-changed-init",
                "keyed changed subscriptions test",
                &[
                    KeyedParent::metadata(),
                    KeyedChild::metadata(),
                    KeyedSlot::metadata(),
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
            "CREATE TABLE keyed_parents (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             owner NVARCHAR(64) NOT NULL, label NVARCHAR(64) NOT NULL); \
             CREATE TABLE keyed_children (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             parent_id NVARCHAR(64) NOT NULL, body NVARCHAR(64) NOT NULL); \
             CREATE TABLE keyed_slots (site NVARCHAR(64) NOT NULL, name NVARCHAR(64) NOT NULL, \
             owner NVARCHAR(64) NOT NULL, label NVARCHAR(64) NOT NULL, PRIMARY KEY (site, name));",
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
    db.set_row_policy(OwnerRowPolicy);
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

async fn parent(db: &Database, id: &str, owner: &str) -> graphql_orm::Result<()> {
    KeyedParent::insert(
        db,
        CreateKeyedParentInput {
            id: id.to_string(),
            owner: owner.to_string(),
            label: "created".to_string(),
        },
    )
    .await?;
    Ok(())
}

async fn touch(db: &Database, id: &str, label: &str) -> graphql_orm::Result<()> {
    KeyedParent::update_by_id(
        db,
        &id.to_string(),
        UpdateKeyedParentInput {
            label: Some(label.to_string()),
            ..Default::default()
        },
    )
    .await?
    .expect("parent exists");
    Ok(())
}

fn parent_subscription(arguments: &str) -> String {
    let changed = resolver_name("keyedParentChanged", "KeyedParentChanged");
    let id = field_name("id", "Id");
    let change_kind = field_name("changeKind", "ChangeKind");
    let entity = field_name("keyedParent", "KeyedParent");
    let label = field_name("label", "Label");
    format!(
        "subscription {{ {changed}{arguments} {{ {id} {change_kind} {entity} {{ {label} }} }} }}"
    )
}

fn keys_argument(keys: &[&str]) -> String {
    let keys_arg = argument_name("keys", "Keys");
    let list = keys
        .iter()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("({keys_arg}: [{list}])")
}

type Events = std::pin::Pin<Box<dyn Stream<Item = Response> + Send>>;

/// Open a subscription as `actor`. Admission errors are the first item.
fn open(db: &Database, query: String, actor: &str) -> Events {
    Box::pin(
        schema_builder(db.clone())
            .finish()
            .execute_stream(Request::new(query).data(AuthSubject::new(actor))),
    )
}

/// Require the subscription to be admitted: no admission error is ready.
async fn admitted(stream: &mut Events) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(other) => panic!("subscription was not admitted: {other:?}"),
    })
    .await;
}

async fn refused(stream: &mut Events) -> Option<String> {
    let response = timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("admission response")
        .expect("admission item");
    assert_eq!(response.errors.len(), 1, "{response:?}");
    response.errors[0]
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        .map(ToString::to_string)
}

async fn next_event(stream: &mut Events, changed: &str) -> serde_json::Value {
    let response = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("an event within the timeout")
        .expect("stream open");
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().expect("json")[changed].clone()
}

fn parent_changed() -> &'static str {
    resolver_name("keyedParentChanged", "KeyedParentChanged")
}

fn id_of(event: &serde_json::Value) -> String {
    event[field_name("id", "Id")]
        .as_str()
        .expect("event id")
        .to_string()
}

async fn keys_deliver_only_listed_keys(db: &Database) -> TestResult {
    for id in ["p1", "p2", "p3"] {
        parent(db, id, "alice").await?;
    }
    let mut stream = open(
        db,
        parent_subscription(&keys_argument(&["p1", "p3", "p1"])),
        "alice",
    );
    admitted(&mut stream).await;
    touch(db, "p2", "outside").await?;
    touch(db, "p1", "first").await?;
    touch(db, "p2", "outside").await?;
    touch(db, "p3", "second").await?;
    // Events arrive in write order, so an unlisted key would be seen first.
    // A duplicated key in the list is delivered once per write.
    let first = next_event(&mut stream, parent_changed()).await;
    assert_eq!(id_of(&first), "p1");
    let second = next_event(&mut stream, parent_changed()).await;
    assert_eq!(id_of(&second), "p3");
    Ok(())
}

async fn one_key_matches_like_key(db: &Database) -> TestResult {
    parent(db, "p1", "alice").await?;
    parent(db, "p2", "alice").await?;
    let mut stream = open(db, parent_subscription(&keys_argument(&["p1"])), "alice");
    admitted(&mut stream).await;
    touch(db, "p2", "outside").await?;
    touch(db, "p1", "inside").await?;
    let event = next_event(&mut stream, parent_changed()).await;
    assert_eq!(id_of(&event), "p1");
    assert_eq!(
        event[field_name("keyedParent", "KeyedParent")][field_name("label", "Label")],
        "inside"
    );
    Ok(())
}

async fn an_empty_list_matches_nothing(db: &Database) -> TestResult {
    parent(db, "p1", "alice").await?;
    let mut empty = open(db, parent_subscription(&keys_argument(&[])), "alice");
    admitted(&mut empty).await;
    let mut control = open(db, parent_subscription(""), "alice");
    admitted(&mut control).await;
    touch(db, "p1", "changed").await?;
    assert_eq!(
        id_of(&next_event(&mut control, parent_changed()).await),
        "p1"
    );
    assert!(
        timeout(Duration::from_millis(500), empty.next())
            .await
            .is_err(),
        "an empty Keys list delivered an event"
    );
    Ok(())
}

async fn the_maximum_is_accepted_and_one_more_is_refused(db: &Database) -> TestResult {
    assert_eq!(db.subscription_key_limit(), 500);
    parent(db, "p1", "alice").await?;
    let mut at_limit = (1..500).map(|n| format!("missing-{n}")).collect::<Vec<_>>();
    at_limit.push("p1".to_string());
    let at_limit = at_limit.iter().map(String::as_str).collect::<Vec<_>>();
    let mut stream = open(db, parent_subscription(&keys_argument(&at_limit)), "alice");
    admitted(&mut stream).await;
    touch(db, "p1", "at-limit").await?;
    assert_eq!(
        id_of(&next_event(&mut stream, parent_changed()).await),
        "p1"
    );

    let mut over = at_limit.clone();
    over.push("one-more");
    let mut refused_stream = open(db, parent_subscription(&keys_argument(&over)), "alice");
    assert_eq!(
        refused(&mut refused_stream).await.as_deref(),
        Some("\"INVALID_INPUT\"")
    );

    // The bound is configurable per database handle.
    let limited = db.clone().with_subscription_key_limit(2);
    let mut two = open(
        &limited,
        parent_subscription(&keys_argument(&["p1", "p2"])),
        "alice",
    );
    admitted(&mut two).await;
    let mut three = open(
        &limited,
        parent_subscription(&keys_argument(&["p1", "p2", "p3"])),
        "alice",
    );
    assert_eq!(
        refused(&mut three).await.as_deref(),
        Some("\"INVALID_INPUT\"")
    );
    Ok(())
}

async fn key_and_keys_together_are_refused(db: &Database) -> TestResult {
    let key_arg = argument_name("key", "Key");
    let keys_arg = argument_name("keys", "Keys");
    let mut stream = open(
        db,
        parent_subscription(&format!(r#"({key_arg}: "p1", {keys_arg}: ["p1"])"#)),
        "alice",
    );
    assert_eq!(
        refused(&mut stream).await.as_deref(),
        Some("\"INVALID_INPUT\"")
    );
    Ok(())
}

async fn an_unreadable_key_is_indistinguishable_from_a_missing_one(db: &Database) -> TestResult {
    parent(db, "mine", "alice").await?;
    parent(db, "theirs", "bob").await?;
    // Admission reads no rows, so neither set reveals anything.
    let mut denied = open(
        db,
        parent_subscription(&keys_argument(&["theirs"])),
        "alice",
    );
    admitted(&mut denied).await;
    let mut missing = open(
        db,
        parent_subscription(&keys_argument(&["missing"])),
        "alice",
    );
    admitted(&mut missing).await;
    let mut mixed = open(
        db,
        parent_subscription(&keys_argument(&["theirs", "mine"])),
        "alice",
    );
    admitted(&mut mixed).await;
    touch(db, "theirs", "private").await?;
    touch(db, "mine", "public").await?;
    let event = next_event(&mut mixed, parent_changed()).await;
    assert_eq!(id_of(&event), "mine");
    assert!(!event.to_string().contains("private"));
    for stream in [&mut denied, &mut missing] {
        assert!(
            timeout(Duration::from_millis(300), stream.next())
                .await
                .is_err(),
            "an unreadable or missing key delivered an event"
        );
    }
    Ok(())
}

async fn propagated_parent_events_match_the_parents_key(db: &Database) -> TestResult {
    parent(db, "p1", "alice").await?;
    parent(db, "p2", "alice").await?;
    let mut stream = open(db, parent_subscription(&keys_argument(&["p1"])), "alice");
    admitted(&mut stream).await;
    for (id, parent_id) in [("c2", "p2"), ("c1", "p1")] {
        KeyedChild::insert(
            db,
            CreateKeyedChildInput {
                id: id.to_string(),
                parent_id: parent_id.to_string(),
                body: "child".to_string(),
            },
        )
        .await?;
    }
    let event = next_event(&mut stream, parent_changed()).await;
    assert_eq!(id_of(&event), "p1");
    assert_eq!(event[field_name("changeKind", "ChangeKind")], "PROPAGATED");
    Ok(())
}

async fn composite_keys_match_on_the_complete_key(db: &Database) -> TestResult {
    for (site, name) in [("north", "a"), ("north", "b"), ("south", "a")] {
        KeyedSlot::insert(
            db,
            CreateKeyedSlotInput {
                site: site.to_string(),
                name: name.to_string(),
                owner: "alice".to_string(),
                label: "created".to_string(),
            },
        )
        .await?;
    }
    let changed = resolver_name("keyedSlotChanged", "KeyedSlotChanged");
    let keys_arg = argument_name("keys", "Keys");
    let key_field = field_name("key", "Key");
    let site_field = field_name("site", "Site");
    let name_field = field_name("name", "Name");
    // Each part of a composite key also appears in another, unlisted key.
    let mut stream = open(
        db,
        format!(
            r#"subscription {{ {changed}({keys_arg}: [{{ {site_field}: "north", {name_field}: "a" }}]) {{ {key_field} {{ {site_field} {name_field} }} }} }}"#
        ),
        "alice",
    );
    admitted(&mut stream).await;
    let relabel = |site: &str, name: &str| {
        let key = KeyedSlotKey {
            site: site.to_string(),
            name: name.to_string(),
        };
        async move {
            KeyedSlot::update_by_key(
                db,
                &key,
                UpdateKeyedSlotInput {
                    label: Some("changed".to_string()),
                    ..Default::default()
                },
            )
            .await
        }
    };
    relabel("north", "b").await?;
    relabel("south", "a").await?;
    relabel("north", "a").await?;
    let event = next_event(&mut stream, changed).await;
    assert_eq!(event[key_field][site_field], "north");
    assert_eq!(event[key_field][name_field], "a");
    Ok(())
}

async fn unkeyed_subscriptions_are_unchanged(db: &Database) -> TestResult {
    parent(db, "p1", "alice").await?;
    parent(db, "p2", "alice").await?;
    let mut stream = open(db, parent_subscription(""), "alice");
    admitted(&mut stream).await;
    touch(db, "p2", "one").await?;
    touch(db, "p1", "two").await?;
    assert_eq!(
        id_of(&next_event(&mut stream, parent_changed()).await),
        "p2"
    );
    assert_eq!(
        id_of(&next_event(&mut stream, parent_changed()).await),
        "p1"
    );

    let sdl = schema_builder(db.clone()).finish().sdl();
    let keys_arg = argument_name("keys", "Keys");
    // The argument block of `name(...)` in the subscription root, whatever its layout.
    let arguments = |name: &str| {
        let start = sdl
            .find(&format!("\t{name}("))
            .unwrap_or_else(|| panic!("{name} in SDL"));
        let block = &sdl[start..];
        block[..block.find("):").expect("argument list end")].to_string()
    };
    let single = arguments(parent_changed());
    assert!(
        single.contains(&format!("{keys_arg}: [String!]")),
        "{single}"
    );
    let composite = arguments(resolver_name("keyedSlotChanged", "KeyedSlotChanged"));
    assert!(
        composite.contains(&format!("{keys_arg}: [KeyedSlotKeyInput!]")),
        "{composite}"
    );
    Ok(())
}

macro_rules! scenarios {
    ($($scenario:ident),* $(,)?) => {
        #[cfg(feature = "sqlite")]
        mod sqlite_scenarios {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
                async fn $scenario() -> super::TestResult {
                    let fixture = super::fixture().await?;
                    super::$scenario(&fixture.db).await?;
                    fixture.finish()
                }
            )*
        }

        #[cfg(not(feature = "sqlite"))]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[ignore = "starts a test-owned database container"]
        async fn every_scenario_on_an_owned_database() -> TestResult {
            let fixture = fixture().await?;
            $(
                reset(&fixture.db).await?;
                $scenario(&fixture.db)
                    .await
                    .map_err(|error| format!("{}: {error}", stringify!($scenario)))?;
                println!("keyed changed scenario passed: {}", stringify!($scenario));
            )*
            fixture.finish()
        }
    };
}

scenarios![
    keys_deliver_only_listed_keys,
    one_key_matches_like_key,
    an_empty_list_matches_nothing,
    the_maximum_is_accepted_and_one_more_is_refused,
    key_and_keys_together_are_refused,
    an_unreadable_key_is_indistinguishable_from_a_missing_one,
    propagated_parent_events_match_the_parents_key,
    composite_keys_match_on_the_complete_key,
    unkeyed_subscriptions_are_unchanged,
];
