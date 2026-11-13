// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! One rule for a field without a policy key (#447): every generated path
//! asks the installed `FieldPolicy` for the field's read decision, with the
//! declared key or `None`, and a refusal is `FORBIDDEN` on each of them.

use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::StreamExt;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::any::Any;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

type Field = (&'static str, &'static str);

/// Field authority that decides by entity and field name alone, so it denies
/// fields that declare no policy key. It records each key-less question, and
/// for a GraphQL read the field the question's context is positioned on.
#[derive(Clone, Default)]
struct NamedFieldAuthority {
    denied: Arc<Mutex<HashSet<Field>>>,
    denied_writes: Arc<Mutex<HashSet<Field>>>,
    asked: Arc<Mutex<HashSet<Field>>>,
    asked_writes: Arc<Mutex<HashSet<Field>>>,
    contexts: Arc<Mutex<HashSet<(&'static str, String)>>>,
}
impl NamedFieldAuthority {
    fn deny(&self, field: Field) {
        self.denied.lock().unwrap().insert(field);
    }
    fn deny_write(&self, field: Field) {
        self.denied_writes.lock().unwrap().insert(field);
    }
    fn asked(&self, field: Field) -> bool {
        self.asked.lock().unwrap().contains(&field)
    }
    fn asked_write(&self, field: Field) -> bool {
        self.asked_writes.lock().unwrap().contains(&field)
    }
    /// Whether `field` was asked under a context positioned on `context`.
    fn asked_under(&self, field: &'static str, context: &str) -> bool {
        self.contexts
            .lock()
            .unwrap()
            .contains(&(field, context.to_owned()))
    }
    fn decide_write(&self, field: Field, key: Option<&'static str>) -> bool {
        assert_eq!(key, None, "{field:?} declares no policy key");
        self.asked_writes.lock().unwrap().insert(field);
        !self.denied_writes.lock().unwrap().contains(&field)
    }
    fn decide(&self, field: Field, key: Option<&'static str>) -> bool {
        assert_eq!(key, None, "{field:?} declares no policy key");
        self.asked.lock().unwrap().insert(field);
        !self.denied.lock().unwrap().contains(&field)
    }
}
impl FieldPolicy for NamedFieldAuthority {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        key: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        self.contexts
            .lock()
            .unwrap()
            .insert((field, ctx.field().name().to_owned()));
        Box::pin(async move { Ok(self.decide((entity, field), key)) })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        key: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide((entity, field), key)) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        key: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide_write((entity, field), key)) })
    }
    fn can_write_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        key: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide_write((entity, field), key)) })
    }
}

mod schema {
    use graphql_orm::async_graphql::SimpleObject;
    use graphql_orm::prelude::*;

    /// A generated GraphQL object: the ORM owns every field getter.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "keyless_parts",
        plural = "KeylessParts",
        auth = "none",
        default_sort = "id ASC",
        aggregate = true,
        node_id,
        federation_key
    )]
    #[graphql_orm(search(index = true, tokenizer = "unicode61", min_token_len = 2))]
    #[graphql_orm(projection(name = "KeylessPartLabel", fields = [id, label], private = true))]
    pub struct KeylessPart {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        pub item_id: String,
        #[graphql_orm(searchable(weight = "A"))]
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
        #[filterable(type = "number")]
        #[sortable]
        pub weight: i32,
    }
    impl BatchLoadEntity for KeylessPart {
        fn batch_column() -> &'static str {
            "item_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "item_id")
        }
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
    #[graphql_entity(
        backend = "sqlite",
        table = "keyless_items",
        plural = "KeylessItems",
        auth = "none"
    )]
    #[graphql_orm(search(index = true, tokenizer = "unicode61", min_token_len = 2))]
    pub struct KeylessItem {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        #[graphql(skip)]
        #[relation(
            target = "KeylessPart",
            from = "id",
            to = "item_id",
            multiple,
            emit_fk = false
        )]
        #[graphql_orm(search_relation(fields = ["label"], weight = "B"))]
        pub parts: Vec<KeylessPart>,
        #[graphql(skip)]
        #[relation(
            target = "KeylessTag",
            from = "id",
            to = "item_id",
            multiple,
            emit_fk = false
        )]
        pub tags: Vec<KeylessTag>,
    }

    /// A plain `SimpleObject` entity: async-graphql owns its scalar getters,
    /// so only the relation loader's pre-check can ask about its fields.
    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        SimpleObject,
        Clone,
        Debug,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql(complex)]
    #[graphql_entity(
        backend = "sqlite",
        table = "keyless_tags",
        plural = "KeylessTags",
        auth = "none",
        default_sort = "id ASC"
    )]
    pub struct KeylessTag {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        pub item_id: String,
        pub name: String,
    }
    #[graphql_orm::async_graphql::ComplexObject]
    impl KeylessTag {}
    impl BatchLoadEntity for KeylessTag {
        fn batch_column() -> &'static str {
            "item_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "item_id")
        }
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [KeylessItem, KeylessPart, KeylessTag],
    }
}
use schema::{KeylessItem, KeylessPart, KeylessPartLabel, KeylessTag};

async fn database() -> (Database, NamedFieldAuthority) {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let target = SchemaModel::from_entities(&[
        KeylessItem::metadata(),
        KeylessPart::metadata(),
        KeylessTag::metadata(),
    ]);
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target,
    );
    for statement in plan.statements.iter().map(String::as_str).chain([
        "INSERT INTO keyless_items (id) VALUES ('i1')",
        "INSERT INTO keyless_parts (id, item_id, label, weight) VALUES ('p1', 'i1', 'widget', 4)",
        "INSERT INTO keyless_tags (id, item_id, name) VALUES ('t1', 'i1', 'widget')",
    ]) {
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await
            .unwrap();
    }
    KeylessPart::rebuild_search_index(&db).await.unwrap();
    KeylessItem::rebuild_search_index(&db).await.unwrap();
    let authority = NamedFieldAuthority::default();
    db.set_field_policy(authority.clone());
    (db, authority)
}

fn forbidden(response: &Response) -> bool {
    !response.errors.is_empty()
        && response.errors.iter().all(|error| {
            error
                .extensions
                .as_ref()
                .and_then(|extensions| extensions.get("code"))
                == Some(&async_graphql::Value::from("FORBIDDEN"))
        })
}

const PART_LABEL: Field = ("KeylessPart", "label");

/// Each generated query path that discloses a key-less field, with the field
/// whose denial must refuse it.
const QUERY_PATHS: [(&str, Field, &str); 16] = [
    (
        "getter on a get-by-key root",
        PART_LABEL,
        "{ keylessPart(id: \"p1\") { label } }",
    ),
    (
        "getter on list items",
        PART_LABEL,
        "{ keylessParts { edges { node { label } } } }",
    ),
    (
        "relation field",
        ("KeylessItem", "parts"),
        "{ keylessItem(id: \"i1\") { parts { edges { node { id } } } } }",
    ),
    (
        "field of a relation target",
        PART_LABEL,
        "{ keylessItem(id: \"i1\") { parts { edges { node { label } } } } }",
    ),
    (
        "getter on a Federation _entities result",
        PART_LABEL,
        "{ _entities(representations: [{__typename: \"KeylessPart\", id: \"p1\"}]) { ... on KeylessPart { label } } }",
    ),
    (
        "NodeId field",
        ("KeylessPart", "NodeId"),
        "{ keylessPart(id: \"p1\") { NodeId } }",
    ),
    (
        "key field disclosed by NodeId",
        ("KeylessPart", "id"),
        "{ keylessPart(id: \"p1\") { NodeId } }",
    ),
    (
        "aggregate group field",
        PART_LABEL,
        "{ keylessPartsAggregate(groupBy: [label], metrics: [{operator: COUNT}], groupLimit: 10) { groups { value } } }",
    ),
    (
        "aggregate metric field",
        ("KeylessPart", "weight"),
        "{ keylessPartsAggregate(groupBy: [], metrics: [{operator: MAX, field: weight}], groupLimit: 10) { metrics { value } } }",
    ),
    (
        "search source",
        PART_LABEL,
        "{ keylessPartsSearch(search: {query: \"widget\"}) { edges { node { id } } } }",
    ),
    (
        "getter on a search hit",
        ("KeylessPart", "weight"),
        "{ keylessPartsSearch(search: {query: \"widget\"}) { edges { node { weight } } } }",
    ),
    (
        "filter argument",
        PART_LABEL,
        "{ keylessParts(where: {label: {eq: \"widget\"}}) { edges { node { id } } } }",
    ),
    (
        "order argument",
        PART_LABEL,
        "{ keylessParts(orderBy: [{label: ASC}]) { edges { node { id } } } }",
    ),
    (
        "search_relation source",
        ("KeylessItem", "parts"),
        "{ keylessItemsSearch(search: {query: \"widget\"}) { edges { node { id } } } }",
    ),
    (
        // The target is a plain SimpleObject, so this refusal can only come
        // from the relation loader's pre-check.
        "relation loader pre-check of a target field",
        ("KeylessTag", "name"),
        "{ keylessItem(id: \"i1\") { tags { edges { node { name } } } } }",
    ),
    (
        "relation filter argument",
        PART_LABEL,
        "{ keylessItem(id: \"i1\") { parts(where: {label: {eq: \"widget\"}}) { edges { node { id } } } } }",
    ),
];

#[tokio::test]
async fn every_generated_read_path_asks_the_keyless_field_decision() {
    for (path, field, query) in QUERY_PATHS {
        let (db, authority) = database().await;
        let schema = schema::schema_builder(db).finish();
        let allowed = schema.execute(Request::new(query)).await;
        assert!(allowed.errors.is_empty(), "{path}: {:?}", allowed.errors);
        assert!(authority.asked(field), "{path}: {field:?} was never asked");

        authority.deny(field);
        let denied = schema.execute(Request::new(query)).await;
        assert!(forbidden(&denied), "{path}: {:?}", denied.errors);
        let rendered = serde_json::to_string(&denied.data).unwrap();
        assert!(!rendered.contains("widget"), "{path}: {rendered}");
    }
}

#[tokio::test]
async fn mutation_outputs_ask_the_keyless_field_decision() {
    let mutation = "mutation { createKeylessPart(input: {id: \"p2\", itemId: \"i1\", label: \"gasket\", weight: 1}) { success keylessPart { label } } }";
    let (db, authority) = database().await;
    authority.deny(PART_LABEL);
    let schema = schema::schema_builder(db.clone()).finish();
    let response = schema.execute(Request::new(mutation)).await;
    assert!(forbidden(&response), "{:?}", response.errors);
    assert!(
        !serde_json::to_string(&response.data)
            .unwrap()
            .contains("gasket")
    );
    // The write itself committed: only the returned field was refused.
    let rows: (i64,) = graphql_orm::sqlx::query_as("SELECT COUNT(*) FROM keyless_parts")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(rows.0, 2);
}

#[tokio::test]
async fn subscription_events_ask_the_keyless_field_decision() {
    // A `Changed` event whose selected field is denied is dropped without a
    // frame, as every per-event denial is; it is never delivered.
    for deny in [false, true] {
        let (db, authority) = database().await;
        if deny {
            authority.deny(PART_LABEL);
        }
        let schema = schema::schema_builder(db).finish();
        let mut stream = Box::pin(schema.execute_stream(Request::new(
            "subscription { keylessPartChanged { action keylessPart { label } } }",
        )));
        graphql_orm::futures::future::poll_fn(|cx| match stream.poll_next_unpin(cx) {
            std::task::Poll::Pending => std::task::Poll::Ready(()),
            std::task::Poll::Ready(response) => panic!("yielded before the write: {response:?}"),
        })
        .await;
        let created = schema
            .execute(Request::new(
                "mutation { createKeylessPart(input: {id: \"p3\", itemId: \"i1\", label: \"gasket\", weight: 1}) { success } }",
            ))
            .await;
        assert!(created.errors.is_empty(), "{:?}", created.errors);
        let event =
            tokio::time::timeout(std::time::Duration::from_millis(750), stream.next()).await;
        assert!(authority.asked(PART_LABEL));
        if deny {
            assert!(event.is_err(), "a denied event was delivered: {event:?}");
        } else {
            let event = event.expect("an event").expect("the stream stays open");
            assert!(event.errors.is_empty(), "{:?}", event.errors);
            assert_eq!(
                event.data.into_json().unwrap()["keylessPartChanged"]["keylessPart"]["label"],
                "gasket"
            );
        }
    }
}

#[tokio::test]
async fn repository_projections_and_reads_ask_the_keyless_field_decision() {
    let (db, authority) = database().await;
    assert!(
        KeylessPartLabel::query(&db)
            .fetch_first()
            .await
            .unwrap()
            .is_some()
    );
    authority.deny(PART_LABEL);
    for error in [
        KeylessPartLabel::query(&db)
            .fetch_first()
            .await
            .map(drop)
            .expect_err("a projection of a denied field is refused"),
        KeylessPart::find_by_id(&db, &"p1".to_owned())
            .await
            .map(drop)
            .expect_err("a repository read of a denied field is refused"),
    ] {
        assert_eq!(
            OrmPublicError::from_sqlx(&error).code,
            OrmErrorCode::Forbidden
        );
    }
}

#[tokio::test]
async fn without_a_field_provider_keyless_fields_are_readable_in_every_mode() {
    for mode in [
        AuthorizationMode::LegacyPermissive,
        AuthorizationMode::DeclaredPoliciesRequired,
    ] {
        let (db, _) = database().await;
        let db = Database::new(db.pool().clone()).with_authorization_mode(mode);
        let schema = schema::schema_builder(db).finish();
        for (path, _, query) in QUERY_PATHS {
            let response = schema.execute(Request::new(query)).await;
            assert!(
                response.errors.is_empty(),
                "{mode:?} {path}: {:?}",
                response.errors
            );
        }
    }
}

#[tokio::test]
async fn node_id_asks_each_key_field_under_a_context_of_its_own_name() {
    let (db, authority) = database().await;
    let schema = schema::schema_builder(db).finish();
    let response = schema
        .execute(Request::new("{ keylessPart(id: \"p1\") { NodeId } }"))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    // The policy's field name and `ctx.field()` agree for both questions.
    assert!(authority.asked_under("NodeId", "NodeId"));
    assert!(authority.asked_under("id", "id"));
    assert!(!authority.asked_under("id", "NodeId"));
}

#[tokio::test]
async fn plain_simple_object_getters_are_the_documented_exception() {
    // async-graphql owns a plain SimpleObject's scalar getters, so a root
    // read of such an entity cannot ask the field decision. This pins the
    // exception the reference documents; `graphql_policy_object` closes it.
    let (db, authority) = database().await;
    authority.deny(("KeylessTag", "name"));
    let schema = schema::schema_builder(db).finish();
    let response = schema
        .execute(Request::new("{ keylessTags { edges { node { name } } } }"))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert!(!authority.asked(("KeylessTag", "name")));
}

#[tokio::test]
async fn repository_writes_ask_the_keyless_field_write_decision() {
    let part = |id: &str| schema::CreateKeylessPartInput {
        id: id.to_owned(),
        item_id: "i1".to_owned(),
        label: "gasket".to_owned(),
        weight: 1,
    };
    let relabel = || schema::UpdateKeylessPartInput {
        label: Some("renamed".to_owned()),
        ..Default::default()
    };
    let (db, authority) = database().await;
    KeylessPart::insert(&db, part("p2")).await.unwrap();
    KeylessPart::update_by_id(&db, &"p2".to_owned(), relabel())
        .await
        .unwrap();
    assert!(authority.asked_write(PART_LABEL));

    authority.deny_write(PART_LABEL);
    for error in [
        KeylessPart::insert(&db, part("p3"))
            .await
            .map(drop)
            .expect_err("a repository insert of a denied field is refused"),
        KeylessPart::update_by_id(&db, &"p1".to_owned(), relabel())
            .await
            .map(drop)
            .expect_err("a repository update of a denied field is refused"),
    ] {
        assert_eq!(
            OrmPublicError::from_sqlx(&error).code,
            OrmErrorCode::Forbidden
        );
    }
    // An update that does not supply the denied field is not refused.
    KeylessPart::update_by_id(
        &db,
        &"p1".to_owned(),
        schema::UpdateKeylessPartInput {
            weight: Some(9),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let labels: Vec<(String, String)> =
        graphql_orm::sqlx::query_as("SELECT id, label FROM keyless_parts ORDER BY id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        labels,
        [
            ("p1".to_owned(), "widget".to_owned()),
            ("p2".to_owned(), "renamed".to_owned())
        ]
    );
}

#[tokio::test]
async fn graphql_writes_ask_the_keyless_field_write_decision() {
    let create = "mutation { createKeylessPart(input: {id: \"p4\", itemId: \"i1\", label: \"gasket\", weight: 1}) { success } }";
    let update =
        "mutation { updateKeylessPart(id: \"p1\", input: {label: \"renamed\"}) { success } }";
    let bulk = "mutation { updateKeylessParts(where: {id: {eq: \"p1\"}}, input: {label: \"renamed\"}) { success errorCode } }";
    for mutation in [create, update, bulk] {
        let (db, authority) = database().await;
        authority.deny_write(PART_LABEL);
        let schema = schema::schema_builder(db.clone()).finish();
        let response = schema.execute(Request::new(mutation)).await;
        assert!(authority.asked_write(PART_LABEL), "{mutation}");
        let rendered = serde_json::to_string(&response).unwrap();
        assert!(rendered.contains("FORBIDDEN"), "{mutation}: {rendered}");
        let labels: Vec<(String,)> =
            graphql_orm::sqlx::query_as("SELECT label FROM keyless_parts ORDER BY id")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert_eq!(labels, [("widget".to_owned(),)], "{mutation}");
    }
}
