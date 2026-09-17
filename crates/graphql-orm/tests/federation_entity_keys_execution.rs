#![cfg(feature = "sqlite")]
//! `_entities` execution behaviour for generated Federation entity resolvers.
//!
//! The generated resolver reproduces the single-row read authorization chain,
//! so these tests pin which denials become `null` for one representation and
//! which fail the whole fetch. `_entities` resolves representations under
//! `try_join_all`, so any error aborts the entire fetch.

use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

mod zones {
    use super::*;

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "zones",
        plural = "Zones",
        default_sort = "name ASC",
        read_policy = "zone.read",
        federation_key
    )]
    pub struct Zone {
        #[primary_key]
        pub id: String,

        #[filterable(type = "string")]
        #[sortable]
        pub name: String,

        #[graphql_orm(read_policy = "zone.note.read")]
        pub note: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [Zone],
    }
}

mod guarded {
    use super::*;

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "guarded_zones",
        plural = "GuardedZones",
        default_sort = "name ASC",
        auth = "required",
        federation_key
    )]
    pub struct GuardedZone {
        #[primary_key]
        pub id: String,

        #[filterable(type = "string")]
        #[sortable]
        pub name: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [GuardedZone],
    }
}

use zones::Zone;

/// Entity policy that answers from an explicit allow list.
#[derive(Clone, Default)]
struct AllowListEntityPolicy {
    allowed: Arc<Mutex<Vec<String>>>,
}

impl AllowListEntityPolicy {
    fn allow(&self, key: &str) -> &Self {
        self.allowed.lock().expect("lock").push(key.to_string());
        self
    }
}

impl graphql_orm::graphql::orm::EntityPolicy for AllowListEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        policy_key: Option<&'static str>,
        _kind: graphql_orm::graphql::orm::EntityAccessKind,
        _surface: graphql_orm::graphql::orm::EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(policy_key.is_none_or(|key| {
                self.allowed
                    .lock()
                    .expect("lock")
                    .iter()
                    .any(|allowed| allowed == key)
            }))
        })
    }
}

/// Row policy that hides one named zone.
#[derive(Clone, Default)]
struct HideNamedZone;

impl graphql_orm::graphql::orm::RowPolicy for HideNamedZone {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: graphql_orm::graphql::orm::EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let Some(zone) = row.downcast_ref::<Zone>() else {
                return Ok(true);
            };
            Ok(zone.name != "restricted")
        })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: graphql_orm::graphql::orm::EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(true) })
    }
}

/// Field policy that refuses one field outright.
#[derive(Clone, Default)]
struct DenyNoteField;

impl graphql_orm::graphql::orm::FieldPolicy for DenyNoteField {
    fn can_read_field<'a>(
        &'a self,
        _ctx: &'a async_graphql::Context<'_>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        _field_name: &'static str,
        policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(policy_key != Some("zone.note.read")) })
    }

    fn can_write_field<'a>(
        &'a self,
        _ctx: &'a async_graphql::Context<'_>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        _field_name: &'static str,
        _policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(true) })
    }
}

async fn zone_pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    sqlx::query("CREATE TABLE zones (id TEXT PRIMARY KEY, name TEXT NOT NULL, note TEXT NOT NULL)")
        .execute(&pool)
        .await
        .expect("create zones");
    for (id, name) in [
        ("zone-1", "north"),
        ("zone-2", "south"),
        ("zone-3", "restricted"),
    ] {
        sqlx::query("INSERT INTO zones (id, name, note) VALUES (?1, ?2, 'note')")
            .bind(id)
            .bind(name)
            .execute(&pool)
            .await
            .expect("seed zone");
    }
    pool
}

fn entities_query(ids: &[&str], selection: &str) -> String {
    let representations = ids
        .iter()
        .map(|id| format!("{{ __typename: \"Zone\", id: \"{id}\" }}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "query {{ _entities(representations: [{representations}]) {{ ... on Zone {{ {selection} }} }} }}"
    )
}

/// Build the zones schema with a caller identity present.
///
/// Generated resolvers default to requiring an authenticated subject when no
/// resolver auth mode is configured, exactly as the single-row read does.
fn zone_schema(database: graphql_orm::db::Database) -> zones::AppSchema {
    zones::schema_builder(database)
        .data(AuthSubject::new("user-1"))
        .finish()
}

fn allowing_policy() -> AllowListEntityPolicy {
    let policy = AllowListEntityPolicy::default();
    policy.allow("zone.read");
    policy
}

#[tokio::test]
async fn representation_hit_resolves_the_owned_entity() {
    let database =
        graphql_orm::db::Database::with_entity_policy(zone_pool().await, allowing_policy());
    let schema = zone_schema(database);

    let response = schema.execute(entities_query(&["zone-1"], "id name")).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json().expect("json");
    assert_eq!(json["_entities"][0]["name"], "north");
}

#[tokio::test]
async fn missing_row_yields_null_for_that_representation() {
    let database =
        graphql_orm::db::Database::with_entity_policy(zone_pool().await, allowing_policy());
    let schema = zone_schema(database);

    let response = schema
        .execute(entities_query(&["zone-1", "absent", "zone-2"], "id"))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json().expect("json");
    assert_eq!(json["_entities"][0]["id"], "zone-1");
    assert!(json["_entities"][1].is_null(), "a miss resolves to null");
    assert_eq!(json["_entities"][2]["id"], "zone-2");
}

#[tokio::test]
async fn row_policy_denial_yields_null_rather_than_an_error() {
    let mut database =
        graphql_orm::db::Database::with_entity_policy(zone_pool().await, allowing_policy());
    database.set_row_policy(HideNamedZone);
    let schema = zone_schema(database);

    let response = schema
        .execute(entities_query(&["zone-1", "zone-3"], "id"))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json().expect("json");
    assert_eq!(json["_entities"][0]["id"], "zone-1");
    assert!(
        json["_entities"][1].is_null(),
        "a row-policy denial is indistinguishable from a miss"
    );
}

#[tokio::test]
async fn entity_policy_denial_fails_the_whole_fetch() {
    let database = graphql_orm::db::Database::with_entity_policy(
        zone_pool().await,
        AllowListEntityPolicy::default(),
    );
    let schema = zone_schema(database);

    let response = schema
        .execute(entities_query(&["zone-1", "zone-2"], "id"))
        .await;
    assert!(
        !response.errors.is_empty(),
        "an entity-policy denial is an error, not a null"
    );
}

#[tokio::test]
async fn field_policy_denial_on_a_selected_key_neighbour_errors() {
    let mut database =
        graphql_orm::db::Database::with_entity_policy(zone_pool().await, allowing_policy());
    database.set_field_policy(DenyNoteField);
    let schema = zone_schema(database);

    let denied = schema.execute(entities_query(&["zone-1"], "id note")).await;
    assert!(
        !denied.errors.is_empty(),
        "a field-level policy still runs inside the entity's own field resolvers"
    );

    let allowed = schema.execute(entities_query(&["zone-1"], "id name")).await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
}

#[tokio::test]
async fn unauthenticated_callers_cannot_resolve_an_auth_required_entity() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    sqlx::query("CREATE TABLE guarded_zones (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .expect("create guarded_zones");
    sqlx::query("INSERT INTO guarded_zones (id, name) VALUES ('zone-1', 'north')")
        .execute(&pool)
        .await
        .expect("seed guarded zone");

    let query = "query { _entities(representations: [{ __typename: \"GuardedZone\", id: \"zone-1\" }]) \
                 { ... on GuardedZone { id name } } }";

    let anonymous = guarded::schema_builder(graphql_orm::db::Database::new(pool.clone()))
        .finish()
        .execute(query)
        .await;
    assert_eq!(
        anonymous.errors.first().map(|error| error.message.as_str()),
        Some("unauthenticated"),
        "{:?}",
        anonymous.errors
    );

    let authenticated = guarded::schema_builder(graphql_orm::db::Database::new(pool))
        .data(AuthSubject::new("user-1"))
        .finish()
        .execute(query)
        .await;
    assert!(
        authenticated.errors.is_empty(),
        "{:?}",
        authenticated.errors
    );
}
