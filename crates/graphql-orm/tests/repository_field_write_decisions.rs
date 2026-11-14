// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! Field write decisions on generated write paths (#447). Repository writes
//! ask `can_write_repository_field` about every field the host code writes,
//! with the declared key or `None`; GraphQL writes ask `can_write_field`
//! about the fields the client could and did supply, and never also ask the
//! repository decision.

use graphql_orm::async_graphql::Request;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::any::Any;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

type Field = (&'static str, &'static str);

/// Field authority that decides writes by entity and field name, and records
/// which decision each write path asked.
#[derive(Clone, Default)]
struct WriteAuthority {
    denied: Arc<Mutex<HashSet<Field>>>,
    repository: Arc<Mutex<HashSet<Field>>>,
    graphql: Arc<Mutex<HashSet<Field>>>,
    /// What the GraphQL decision received for `LedgerNote.remark`: the
    /// current row's `remark` (when a row was passed) and the value written.
    remark_seen: Arc<Mutex<Vec<RemarkDecision>>>,
}
type RemarkDecision = (Option<Option<String>>, Option<Option<String>>);
impl WriteAuthority {
    fn deny(&self, field: Field) {
        self.denied.lock().unwrap().insert(field);
    }
    fn allow_all(&self) {
        self.denied.lock().unwrap().clear();
    }
    fn repository_asked(&self) -> HashSet<Field> {
        std::mem::take(&mut self.repository.lock().unwrap())
    }
    fn graphql_asked(&self) -> HashSet<Field> {
        std::mem::take(&mut self.graphql.lock().unwrap())
    }
}
impl FieldPolicy for WriteAuthority {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        key: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
        value: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        assert_eq!(key, None);
        self.graphql.lock().unwrap().insert((entity, field));
        if (entity, field) == ("LedgerNote", "remark") {
            self.remark_seen.lock().unwrap().push((
                record
                    .and_then(|record| record.downcast_ref::<LedgerNote>())
                    .map(|note| note.remark.clone()),
                value
                    .and_then(|value| value.downcast_ref::<Option<String>>())
                    .cloned(),
            ));
        }
        let allowed = !self.denied.lock().unwrap().contains(&(entity, field));
        Box::pin(async move { Ok(allowed) })
    }
    fn can_write_repository_field<'a>(
        &'a self,
        access: Option<AccessContext<'a>>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        key: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        // Generated repository writes carry no access context.
        assert!(access.is_none());
        assert_eq!(key, None);
        self.repository.lock().unwrap().insert((entity, field));
        let allowed = !self.denied.lock().unwrap().contains(&(entity, field));
        Box::pin(async move { Ok(allowed) })
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
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "ledger_notes",
    plural = "LedgerNotes",
    auth = "none",
    default_sort = "id ASC",
    upsert = "slug"
)]
struct LedgerNote {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[unique]
    #[filterable(type = "string")]
    slug: String,
    #[filterable(type = "string")]
    body: String,
    /// Optional and part of the GraphQL input.
    remark: Option<String>,
    /// Not part of the GraphQL input: only host code can write it.
    #[graphql_orm(skip_input)]
    internal_tag: Option<String>,
    #[graphql_orm(version, default = "0")]
    #[filterable(type = "number")]
    version: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "ledger_grants",
    plural = "LedgerGrants",
    auth = "none",
    repository_mutations = true,
    default_sort = "subject ASC, name ASC"
)]
struct LedgerGrant {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    subject: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    name: String,
    #[filterable(type = "string")]
    value: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [LedgerNote, LedgerGrant],
}

async fn database() -> (Database, WriteAuthority) {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let target = SchemaModel::from_entities(&[LedgerNote::metadata(), LedgerGrant::metadata()]);
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target,
    );
    for statement in plan.statements.iter().map(String::as_str).chain([
        "INSERT INTO ledger_notes (id, slug, body, version) VALUES ('n1', 'first', 'before', 0)",
        "INSERT INTO ledger_grants (subject, name, value) VALUES ('s1', 'viewer', 'before')",
    ]) {
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let authority = WriteAuthority::default();
    db.set_field_policy(authority.clone());
    db.set_entity_policy(AllowEntities);
    (db, authority)
}

fn note(id: &str, slug: &str) -> CreateLedgerNoteInput {
    CreateLedgerNoteInput {
        id: id.to_owned(),
        slug: slug.to_owned(),
        body: "after".to_owned(),
        remark: None,
        internal_tag: None,
    }
}
fn rebody() -> UpdateLedgerNoteInput {
    UpdateLedgerNoteInput {
        body: Some("after".to_owned()),
        ..Default::default()
    }
}
fn regrant() -> UpdateLedgerGrantInput {
    UpdateLedgerGrantInput {
        value: Some("after".to_owned()),
    }
}
fn viewer() -> LedgerGrantWhereInput {
    LedgerGrantWhereInput {
        name: Some(StringFilter {
            eq: Some("viewer".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn grant_key() -> LedgerGrantKey {
    LedgerGrantKey {
        subject: "s1".to_owned(),
        name: "viewer".to_owned(),
    }
}

/// A refused repository write is `FORBIDDEN` and names the public field.
fn assert_denied(error: graphql_orm::sqlx::Error, field: &str, path: &str) {
    let public = OrmPublicError::from_sqlx(&error);
    assert_eq!(public.code, OrmErrorCode::Forbidden, "{path}");
    assert!(public.message.contains(field), "{path}: {}", public.message);
    let graphql = public.into_graphql_error();
    assert_eq!(
        graphql.extensions.unwrap().get("deniedWriteField"),
        Some(&async_graphql::Value::from(field)),
        "{path}"
    );
}

async fn unchanged(db: &Database) {
    let notes: Vec<(String, String)> =
        graphql_orm::sqlx::query_as("SELECT id, body FROM ledger_notes ORDER BY id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(notes, [("n1".to_owned(), "before".to_owned())]);
    let grants: Vec<(String, String)> =
        graphql_orm::sqlx::query_as("SELECT name, value FROM ledger_grants ORDER BY name")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(grants, [("viewer".to_owned(), "before".to_owned())]);
}

#[tokio::test]
async fn every_single_key_repository_write_asks_the_field_write_decision() {
    let (db, authority) = database().await;
    authority.deny(("LedgerNote", "body"));
    let by_slug = || LedgerNoteWhereInput {
        slug: Some(StringFilter {
            eq: Some("first".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let error = LedgerNote::insert(&db, note("n2", "second"))
        .await
        .unwrap_err();
    assert_denied(error, "body", "insert");
    let error = LedgerNote::insert_many(&db, [note("n2", "second"), note("n3", "third")])
        .await
        .unwrap_err();
    assert_denied(error, "body", "insert_many");
    // A new row and an existing row both refuse the denied field.
    let error = LedgerNote::upsert(&db, note("n2", "second"))
        .await
        .unwrap_err();
    assert_denied(error, "body", "upsert (new)");
    let error = LedgerNote::upsert(&db, note("n1", "first"))
        .await
        .unwrap_err();
    assert_denied(error, "body", "upsert (existing)");
    let error = LedgerNote::update_by_id(&db, &"n1".to_owned(), rebody())
        .await
        .unwrap_err();
    assert_denied(error, "body", "update_by_id");
    let error = LedgerNote::update_where(&db, by_slug(), rebody())
        .await
        .unwrap_err();
    assert_denied(error, "body", "update_where");
    let error = LedgerNote::compare_and_swap(
        &db,
        &"n1".to_owned(),
        0,
        LedgerNoteWhereInput::default(),
        rebody(),
    )
    .await
    .unwrap_err();
    assert_denied(error, "body", "compare_and_swap");
    unchanged(&db).await;

    // No repository write asked the GraphQL decision.
    authority.repository_asked();
    assert!(authority.graphql_asked().is_empty());

    // Once allowed, the same writes succeed. An insert asks about every
    // field the host code writes, with `None`, including the one a GraphQL
    // client cannot supply, and not about the ORM-managed version column.
    authority.allow_all();
    LedgerNote::insert(&db, note("n2", "second")).await.unwrap();
    assert_eq!(
        authority.repository_asked(),
        ["id", "slug", "body", "remark", "internalTag"]
            .into_iter()
            .map(|field| ("LedgerNote", field))
            .collect::<HashSet<_>>()
    );
    LedgerNote::insert_many(&db, [note("n3", "third")])
        .await
        .unwrap();
    LedgerNote::upsert(&db, note("n4", "fourth")).await.unwrap();
    LedgerNote::update_by_id(&db, &"n2".to_owned(), rebody())
        .await
        .unwrap();
    assert_eq!(
        LedgerNote::update_where(&db, by_slug(), rebody())
            .await
            .unwrap(),
        1
    );
    assert!(matches!(
        LedgerNote::compare_and_swap(
            &db,
            &"n3".to_owned(),
            0,
            LedgerNoteWhereInput::default(),
            rebody(),
        )
        .await
        .unwrap(),
        ConditionalUpdateOutcome::Updated(_)
    ));
}

#[tokio::test]
async fn every_composite_key_repository_write_asks_the_field_write_decision() {
    let (db, authority) = database().await;
    authority.deny(("LedgerGrant", "value"));
    let editor = || CreateLedgerGrantInput {
        subject: "s1".to_owned(),
        name: "editor".to_owned(),
        value: "after".to_owned(),
    };

    let error = LedgerGrant::insert_if_absent(&db, editor())
        .await
        .unwrap_err();
    assert_denied(error, "value", "insert_if_absent");
    let error = LedgerGrant::update_by_key(&db, &grant_key(), regrant())
        .await
        .unwrap_err();
    assert_denied(error, "value", "update_by_key");
    let error = LedgerGrant::update_if(&db, &grant_key(), viewer(), regrant())
        .await
        .unwrap_err();
    assert_denied(error, "value", "update_if");
    unchanged(&db).await;
    assert!(
        authority
            .repository_asked()
            .contains(&("LedgerGrant", "value"))
    );

    authority.allow_all();
    assert!(matches!(
        LedgerGrant::insert_if_absent(&db, editor()).await.unwrap(),
        InsertIfAbsentOutcome::Inserted(_)
    ));
    assert!(
        LedgerGrant::update_by_key(&db, &grant_key(), regrant())
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        LedgerGrant::update_if(&db, &grant_key(), viewer(), regrant(),)
            .await
            .unwrap(),
        PredicateUpdateOutcome::Updated(_)
    ));
}

#[tokio::test]
async fn graphql_writes_ask_only_the_graphql_decision_and_only_supplied_input_fields() {
    let (db, authority) = database().await;
    let schema = schema_builder(db.clone()).finish();
    let run = |mutation: &'static str| {
        let schema = schema.clone();
        async move {
            let response = schema.execute(Request::new(mutation)).await;
            assert!(
                response.errors.is_empty(),
                "{mutation}: {:?}",
                response.errors
            );
            response.data.into_json().unwrap()
        }
    };

    // A create asks about the required inputs and about `remark` only when
    // the client supplies it; never about the `skip_input` field.
    run("mutation { createLedgerNote(input: {id: \"n2\", slug: \"second\", body: \"b\"}) { success } }").await;
    let asked = authority.graphql_asked();
    for field in ["id", "slug", "body"] {
        assert!(asked.contains(&("LedgerNote", field)), "{field}");
    }
    assert!(!asked.contains(&("LedgerNote", "remark")));
    assert!(!asked.contains(&("LedgerNote", "internalTag")));
    assert!(!asked.contains(&("LedgerNote", "version")));
    run("mutation { createLedgerNote(input: {id: \"n3\", slug: \"third\", body: \"b\", remark: \"r\"}) { success } }").await;
    assert!(
        authority
            .graphql_asked()
            .contains(&("LedgerNote", "remark"))
    );

    // An update and a bulk update ask only about the supplied field.
    run("mutation { updateLedgerNote(id: \"n1\", input: {body: \"b\"}) { success } }").await;
    assert_eq!(
        authority.graphql_asked(),
        HashSet::from([("LedgerNote", "body")])
    );
    run("mutation { updateLedgerNotes(where: {id: {eq: \"n1\"}}, input: {remark: \"r\"}) { success } }").await;
    assert_eq!(
        authority.graphql_asked(),
        HashSet::from([("LedgerNote", "remark")])
    );
    run("mutation { upsertLedgerNote(input: {id: \"n9\", slug: \"first\", body: \"b\"}) { success } }").await;
    // An upsert onto an existing row asks about every field its update
    // sets: the omitted optional `remark` is written as NULL and the
    // `skip_input` field as its default.
    let asked = authority.graphql_asked();
    for field in ["body", "remark", "internalTag"] {
        assert!(asked.contains(&("LedgerNote", field)), "{field}");
    }
    assert!(!asked.contains(&("LedgerNote", "id")));
    assert!(!asked.contains(&("LedgerNote", "slug")));
    // An upsert that inserts asks only about what the client supplied.
    run("mutation { upsertLedgerNote(input: {id: \"n8\", slug: \"eighth\", body: \"b\"}) { success } }").await;
    let asked = authority.graphql_asked();
    assert!(asked.contains(&("LedgerNote", "body")));
    assert!(!asked.contains(&("LedgerNote", "remark")));
    assert!(!asked.contains(&("LedgerNote", "internalTag")));

    // No GraphQL write also asked the repository decision.
    assert!(authority.repository_asked().is_empty());
}

/// The existing-row upsert asks about an omitted optional field with the
/// current row and `None`, the value its update writes.
#[tokio::test]
async fn an_existing_row_upsert_passes_the_current_row_and_none_for_an_omitted_optional_field() {
    let (db, authority) = database().await;
    graphql_orm::sqlx::query("UPDATE ledger_notes SET remark = 'kept' WHERE slug = 'first'")
        .execute(db.pool())
        .await
        .unwrap();
    let schema = schema_builder(db.clone()).finish();
    let response = schema
        .execute(Request::new(
            "mutation { upsertLedgerNote(input: {id: \"n9\", slug: \"first\", body: \"b\"}) { success } }",
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        *authority.remark_seen.lock().unwrap(),
        [(Some(Some("kept".to_owned())), Some(None))]
    );
    // An upsert that inserts does not ask about the omitted field at all.
    authority.remark_seen.lock().unwrap().clear();
    let response = schema
        .execute(Request::new(
            "mutation { upsertLedgerNote(input: {id: \"n8\", slug: \"eighth\", body: \"b\"}) { success } }",
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert!(authority.remark_seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_deny_list_on_a_field_the_client_cannot_supply_blocks_only_writes_that_set_it() {
    let (db, authority) = database().await;
    // The host denies GraphQL writes to fields clients cannot send anyway.
    authority.deny(("LedgerNote", "internalTag"));
    authority.deny(("LedgerNote", "version"));
    let schema = schema_builder(db.clone()).finish();
    for mutation in [
        "mutation { createLedgerNote(input: {id: \"n2\", slug: \"second\", body: \"b\"}) { success } }",
        "mutation { upsertLedgerNote(input: {id: \"n3\", slug: \"third\", body: \"b\"}) { success } }",
        "mutation { updateLedgerNote(id: \"n1\", input: {body: \"b\"}) { success } }",
    ] {
        let response = schema.execute(Request::new(mutation)).await;
        assert!(
            response.errors.is_empty(),
            "{mutation}: {:?}",
            response.errors
        );
        let data = response.data.into_json().unwrap();
        assert!(
            data.as_object()
                .unwrap()
                .values()
                .all(|payload| payload["success"] == true),
            "{mutation}: {data}"
        );
    }
    // An upsert onto an existing row resets the field to its default, so
    // there the deny-list is obeyed.
    let response = schema
        .execute(Request::new(
            "mutation { upsertLedgerNote(input: {id: \"n9\", slug: \"first\", body: \"refused\"}) { success } }",
        ))
        .await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code")),
        Some(&async_graphql::Value::from("FORBIDDEN"))
    );
    let refusal = format!("{:?}", response.errors);
    assert!(refusal.contains("internalTag"), "{refusal}");
    // The refused upsert wrote nothing: the body is still the update's.
    let bodies: Vec<String> =
        graphql_orm::sqlx::query_scalar("SELECT body FROM ledger_notes WHERE slug = 'first'")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(bodies, ["b".to_owned()]);
    // Host code that writes the field through the repository is still asked.
    let error = LedgerNote::insert(
        &db,
        CreateLedgerNoteInput {
            internal_tag: Some("tag".to_owned()),
            ..note("n4", "fourth")
        },
    )
    .await
    .unwrap_err();
    assert_denied(error, "internalTag", "insert");
}
