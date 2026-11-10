//! GHSA-f7xj-6695-ww2q: `order_aggregate` count ordering must count only the
//! related rows the request may read, and fail closed when it cannot.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
))]

#[path = "support/graphql_case.rs"]
mod graphql_case;
#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_case::{argument_name as a, field_name as f, resolver_name as r};
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
#[cfg(not(feature = "mssql"))]
use graphql_orm::sqlx::Row;
use serde_json::Value;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "mssql")]
type Backend = MssqlBackend;

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(
    table = "count_groups",
    plural = "CountGroups",
    default_sort = "id ASC"
)]
struct CountGroup {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
    #[graphql(skip)]
    #[relation(target = "CountParent", from = "id", to = "group_id", multiple)]
    parents: Vec<CountParent>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "count_parents",
    plural = "CountParents",
    default_sort = "id ASC"
)]
struct CountParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    group_id: String,
    #[relation(
        target = "CountChild",
        from = "id",
        to = "parent_id",
        multiple,
        emit_fk = false,
        order_aggregate(name = "VisibleChildCount", aggregate = "count")
    )]
    children: Vec<CountChild>,
    #[relation(
        target = "CountSecret",
        from = "id",
        to = "parent_id",
        multiple,
        emit_fk = false,
        order_aggregate(name = "SecretCount", aggregate = "count")
    )]
    secrets: Vec<CountSecret>,
    #[relation(
        target = "CountBlocked",
        from = "id",
        to = "parent_id",
        multiple,
        emit_fk = false,
        order_aggregate(name = "BlockedCount", aggregate = "count")
    )]
    blocked: Vec<CountBlocked>,
}

/// SQL visibility: only `visible` children may be read.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "count_children",
    plural = "CountChildren",
    default_sort = "id ASC"
)]
struct CountChild {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    parent_id: String,
    #[filterable(type = "boolean")]
    visible: bool,
}

/// Callback-only row visibility: cannot be expressed in SQL.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "count_secrets",
    plural = "CountSecrets",
    default_sort = "id ASC"
)]
struct CountSecret {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    parent_id: String,
}

/// Entity access is denied outright.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "count_blocked",
    plural = "CountBlockeds",
    default_sort = "id ASC"
)]
struct CountBlocked {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    parent_id: String,
}

impl BatchLoadEntity for CountParent {
    fn batch_column() -> &'static str {
        "group_id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get("group_id")
    }
}

schema_roots! {
    query_custom_ops: [],
    entities: [CountGroup, CountParent, CountChild, CountSecret, CountBlocked],
}

struct Policy;
impl RowPolicy<Backend> for Policy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            Ok(match entity {
                "CountChild" => ReadVisibility::Complete(ReadPredicate::from_filter::<Backend, _>(
                    &CountChildWhereInput {
                        visible: Some(BoolFilter {
                            eq: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )?),
                "CountSecret" => ReadVisibility::CallbackOnly,
                _ => ReadVisibility::Unrestricted,
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(row.downcast_ref::<CountChild>().is_none_or(|c| c.visible)
                && row.downcast_ref::<CountSecret>().is_none())
        })
    }
    fn can_write_row<'a>(
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
}
impl EntityPolicy<Backend> for Policy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(entity != "CountBlocked") })
    }
}

/// Keeps a test-owned database container alive for the schema's lifetime.
#[cfg(feature = "postgres")]
type Owned = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type Owned = owned_mssql::OwnedSqlServer;
#[cfg(feature = "sqlite")]
type Owned = ();

#[cfg(feature = "mssql")]
const TEXT: &str = "NVARCHAR(64)";
#[cfg(not(feature = "mssql"))]
const TEXT: &str = "TEXT";
#[cfg(feature = "mssql")]
const BOOL: (&str, &str, &str) = ("BIT", "1", "0");
#[cfg(not(feature = "mssql"))]
const BOOL: (&str, &str, &str) = ("BOOLEAN", "TRUE", "FALSE");

fn seed_statements() -> Vec<String> {
    let (bool_type, yes, no) = BOOL;
    vec![
        format!(
            "CREATE TABLE count_groups (id {TEXT} NOT NULL PRIMARY KEY, label {TEXT} NOT NULL)"
        ),
        format!(
            "CREATE TABLE count_parents (id {TEXT} NOT NULL PRIMARY KEY, group_id {TEXT} NOT NULL)"
        ),
        format!(
            "CREATE TABLE count_children (id {TEXT} NOT NULL PRIMARY KEY, parent_id {TEXT} NOT NULL, visible {bool_type} NOT NULL)"
        ),
        format!(
            "CREATE TABLE count_secrets (id {TEXT} NOT NULL PRIMARY KEY, parent_id {TEXT} NOT NULL)"
        ),
        format!(
            "CREATE TABLE count_blocked (id {TEXT} NOT NULL PRIMARY KEY, parent_id {TEXT} NOT NULL)"
        ),
        "INSERT INTO count_groups (id, label) VALUES ('g', 'g')".to_owned(),
        "INSERT INTO count_parents (id, group_id) VALUES ('a', 'g'), ('b', 'g'), ('c', 'g')"
            .to_owned(),
        // `a` has the most children, but none of them are readable.
        format!(
            "INSERT INTO count_children (id, parent_id, visible) VALUES
            ('a1', 'a', {no}), ('a2', 'a', {no}), ('a3', 'a', {no}),
            ('b1', 'b', {yes}),
            ('c1', 'c', {yes}), ('c2', 'c', {yes})"
        ),
        "INSERT INTO count_secrets (id, parent_id) VALUES ('s1', 'a'), ('s2', 'a'), ('s3', 'b')"
            .to_owned(),
        "INSERT INTO count_blocked (id, parent_id) VALUES ('x1', 'b'), ('x2', 'b'), ('x3', 'c')"
            .to_owned(),
    ]
}

async fn database() -> (Database<Backend>, Owned) {
    #[cfg(feature = "sqlite")]
    let (db, owned) = (
        Database::<Backend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap(),
        (),
    );
    #[cfg(feature = "postgres")]
    let (db, owned) = {
        let owned = owned_postgres::OwnedPostgres::start("order-aggregate").unwrap();
        let db = Database::<Backend>::connect_postgres(&owned.url)
            .await
            .unwrap();
        (db, owned)
    };
    #[cfg(feature = "mssql")]
    let (db, owned) = {
        let owned = owned_mssql::OwnedSqlServer::start().await.unwrap();
        let db = Database::<Backend>::connect_ado(&owned.connection_string)
            .await
            .unwrap();
        (db, owned)
    };
    #[cfg(not(feature = "mssql"))]
    for statement in seed_statements() {
        graphql_orm::sqlx::query(&statement)
            .execute(db.pool())
            .await
            .unwrap();
    }
    #[cfg(feature = "mssql")]
    owned_mssql::execute_batch(&owned.connection_string, &seed_statements().join(";\n"))
        .await
        .unwrap();
    (db, owned)
}

async fn schema() -> async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot> {
    let (mut db, owned) = database().await;
    db.set_row_policy(Policy);
    db.set_entity_policy(Policy);
    async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .data(db)
    .data(owned)
    .data(AuthSubject::builder("reader").build())
    .finish()
}

fn ids(connection: &Value) -> Vec<String> {
    connection["edges"]
        .as_array()
        .expect("connection edges")
        .iter()
        .map(|edge| edge["node"][f("id")].as_str().unwrap().to_owned())
        .collect()
}

fn list_query(order: &str) -> String {
    format!(
        "query {{ rows: {}({}: [{{ {order}: DESC }}]) {{ edges {{ node {{ {} }} }} }} }}",
        r("countParents"),
        a("orderBy"),
        f("id"),
    )
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn list_count_ordering_counts_only_readable_children() {
    let schema = schema().await;
    let response = schema.execute(list_query("VisibleChildCount")).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    // Unfiltered counts would rank `a` (three hidden children) first.
    assert_eq!(ids(&data["rows"]), ["c", "b", "a"]);
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn relation_connection_count_ordering_counts_only_readable_children() {
    let schema = schema().await;
    let query = format!(
        "query {{ groups: {} {{ edges {{ node {{ {}({}: {{ VisibleChildCount: DESC }}) {{ edges {{ node {{ {} }} }} }} }} }} }} }}",
        r("countGroups"),
        f("parents"),
        a("orderBy"),
        f("id"),
    );
    let response = schema.execute(query).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let parents = &data["groups"]["edges"][0]["node"][f("parents")];
    assert_eq!(ids(parents), ["c", "b", "a"]);
}

fn error_codes(response: &async_graphql::Response) -> Vec<String> {
    response
        .errors
        .iter()
        .map(|error| {
            error
                .extensions
                .as_ref()
                .and_then(|extensions| extensions.get("code"))
                .map(|code| code.to_string())
                .unwrap_or_default()
        })
        .collect()
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn callback_only_target_visibility_fails_closed() {
    let schema = schema().await;
    let response = schema.execute(list_query("SecretCount")).await;
    assert_eq!(response.data, async_graphql::Value::Null);
    let codes = error_codes(&response);
    assert_eq!(codes.len(), 1, "{:?}", response.errors);
    assert!(
        codes[0].contains("AUTHORIZATION_MISCONFIGURED"),
        "{:?}",
        response.errors
    );
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn denied_target_entity_access_fails_closed() {
    let schema = schema().await;
    let response = schema.execute(list_query("BlockedCount")).await;
    assert_eq!(response.data, async_graphql::Value::Null);
    let codes = error_codes(&response);
    assert_eq!(codes.len(), 1, "{:?}", response.errors);
    assert!(codes[0].contains("FORBIDDEN"), "{:?}", response.errors);
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn unauthorized_count_order_is_refused_by_query_builders() {
    let (db, _owned) = database().await;
    let order = CountParentOrderByInput {
        visible_child_count: Some(OrderDirection::Desc),
        ..Default::default()
    };
    assert!(order.validate_order().is_err());
    // Even the rendered fallback counts nothing.
    assert!(order.to_sql_order().unwrap().contains("AND 1 = 0"));
    let result = EntityQuery::<CountParent, Backend>::new()
        .order_by(&order)
        .fetch_all(&db)
        .await;
    assert!(result.is_err());
}
