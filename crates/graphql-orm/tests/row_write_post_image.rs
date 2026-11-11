//! Row read and write policy on the rows generated writes select, produce and return.
//!
//! - The written row (post-image) must satisfy `RowPolicy::can_write_row`, on every
//!   key shape, or the write rolls back.
//! - A generated mutation payload releases the committed row only when the caller
//!   may read it.
//! - Generated GraphQL writes treat a row the caller can neither read nor write
//!   exactly like a missing row.
#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "owned_notes",
    plural = "OwnedNotes",
    default_sort = "name ASC",
    upsert = "slug"
)]
struct OwnedNote {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
    #[unique]
    #[filterable(type = "string")]
    pub slug: String,
    // Deliberately client-writable: the row write policy alone must keep rows in scope.
    #[filterable(type = "string")]
    pub owner_user_id: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "owned_pairs",
    plural = "OwnedPairs",
    repository_mutations = true,
    unique_composite = "tenant_key,item_key",
    default_sort = "tenant_key ASC, item_key ASC",
    upsert = "tenant_key,item_key"
)]
struct OwnedPair {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    tenant_key: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    item_key: String,
    #[filterable(type = "string")]
    owner_user_id: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "owned_events", plural = "OwnedEvents", append_only = true)]
struct OwnedEvent {
    #[primary_key]
    #[sortable]
    id: i32,
    #[primary_key]
    sequence: i32,
    owner_user_id: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [OwnedNote, OwnedPair, OwnedEvent],
}

/// Repository calls carry no request context; they act as this fixed host principal.
const REPOSITORY_ACTOR: &str = "repo";

fn actor(ctx: Option<&async_graphql::Context<'_>>) -> Option<String> {
    match ctx {
        Some(ctx) => ctx.data_opt::<String>().cloned(),
        None => Some(REPOSITORY_ACTOR.to_string()),
    }
}

fn owner(row: &(dyn std::any::Any + Send + Sync)) -> Option<&str> {
    if let Some(note) = row.downcast_ref::<OwnedNote>() {
        Some(&note.owner_user_id)
    } else if let Some(pair) = row.downcast_ref::<OwnedPair>() {
        Some(&pair.owner_user_id)
    } else {
        row.downcast_ref::<OwnedEvent>()
            .map(|event| event.owner_user_id.as_str())
    }
}

/// Owners read and write their own rows. `writer` may write every row but read none;
/// `reader` may read every row but write none.
#[derive(Clone, Default)]
struct OwnerPolicy;

impl graphql_orm::graphql::orm::RowPolicy for OwnerPolicy {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a graphql_orm::db::Database,
        _: &'static str,
        _: Option<&'static str>,
        _: graphql_orm::graphql::orm::EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let actor = actor(ctx);
            Ok(actor.as_deref() == Some("reader") || actor.as_deref() == owner(row))
        })
    }

    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a graphql_orm::db::Database,
        _: &'static str,
        _: Option<&'static str>,
        _: graphql_orm::graphql::orm::EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let actor = actor(ctx);
            Ok(actor.as_deref() == Some("writer") || actor.as_deref() == owner(row))
        })
    }
}

#[derive(Clone)]
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

async fn database() -> (graphql_orm::db::Database, sqlx::SqlitePool) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    for ddl in [
        "CREATE TABLE owned_notes (id TEXT PRIMARY KEY, name TEXT NOT NULL, slug TEXT UNIQUE NOT NULL, owner_user_id TEXT NOT NULL)",
        "CREATE TABLE owned_pairs (tenant_key TEXT NOT NULL, item_key TEXT NOT NULL, owner_user_id TEXT NOT NULL, PRIMARY KEY (tenant_key, item_key))",
        "CREATE TABLE owned_events (id INTEGER NOT NULL, sequence INTEGER NOT NULL, owner_user_id TEXT NOT NULL, PRIMARY KEY (id, sequence))",
    ] {
        sqlx::query(ddl).execute(&pool).await.unwrap();
    }
    let mut db = graphql_orm::db::Database::new(pool.clone())
        .with_authorized_scan_config(AuthorizedScanConfig::new([5; 32], 8, 64));
    db.set_entity_policy(AllowEntities);
    db.set_row_policy(OwnerPolicy);
    (db, pool)
}

fn schema(db: &graphql_orm::db::Database, actor: &str) -> AppSchema {
    schema_builder(db.clone()).data(actor.to_string()).finish()
}

async fn count(pool: &sqlx::SqlitePool, table: &str, owner: &str) -> i64 {
    sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM {table} WHERE owner_user_id = ?"
    ))
    .bind(owner)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn note_name(pool: &sqlx::SqlitePool, id: &str) -> String {
    sqlx::query_scalar("SELECT name FROM owned_notes WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn run(schema: &AppSchema, query: impl Into<String>) -> serde_json::Value {
    serde_json::to_value(schema.execute(query.into()).await).unwrap()
}

fn error_code(response: &serde_json::Value) -> Option<&str> {
    response["errors"][0]["extensions"]["code"].as_str()
}

/// Create a note owned by `owner` through that owner's own schema.
async fn create_note(db: &graphql_orm::db::Database, owner: &str, slug: &str) -> String {
    let response = run(
        &schema(db, owner),
        format!(
            "mutation {{ createOwnedNote(input: {{ name: \"secret\", slug: \"{slug}\", ownerUserId: \"{owner}\" }}) {{ ownedNote {{ id }} }} }}"
        ),
    )
    .await;
    response["data"]["createOwnedNote"]["ownedNote"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("owner create failed: {response}"))
        .to_owned()
}

#[tokio::test]
async fn create_cannot_write_a_row_the_caller_may_not_write() {
    let (db, pool) = database().await;
    let response = run(
        &schema(&db, "actor-a"),
        "mutation { createOwnedNote(input: { name: \"x\", slug: \"x\", ownerUserId: \"actor-b\" }) { success } }",
    )
    .await;
    assert_eq!(error_code(&response), Some("FORBIDDEN"), "{response}");
    assert_eq!(
        count(&pool, "owned_notes", "actor-b").await,
        0,
        "created row fails can_write_row for its creator"
    );
}

#[tokio::test]
async fn update_cannot_move_a_row_out_of_the_callers_write_scope() {
    let (db, pool) = database().await;
    let id = create_note(&db, "actor-a", "a").await;
    let response = run(
        &schema(&db, "actor-a"),
        format!(
            "mutation {{ updateOwnedNote(id: \"{id}\", input: {{ ownerUserId: \"actor-b\" }}) {{ success }} }}"
        ),
    )
    .await;
    assert_eq!(error_code(&response), Some("FORBIDDEN"), "{response}");
    assert_eq!(
        count(&pool, "owned_notes", "actor-b").await,
        0,
        "post-image must satisfy can_write_row"
    );
    assert_eq!(count(&pool, "owned_notes", "actor-a").await, 1);
}

#[tokio::test]
async fn upsert_cannot_create_or_move_a_row_out_of_the_callers_write_scope() {
    let (db, pool) = database().await;
    let actor_a = schema(&db, "actor-a");
    let created = run(
        &actor_a,
        "mutation { upsertOwnedNote(input: { name: \"x\", slug: \"new\", ownerUserId: \"actor-b\" }) { success } }",
    )
    .await;
    assert_eq!(error_code(&created), Some("FORBIDDEN"), "{created}");

    create_note(&db, "actor-a", "mine").await;
    let moved = run(
        &actor_a,
        "mutation { upsertOwnedNote(input: { name: \"x\", slug: \"mine\", ownerUserId: \"actor-b\" }) { success } }",
    )
    .await;
    assert_eq!(error_code(&moved), Some("FORBIDDEN"), "{moved}");
    assert_eq!(count(&pool, "owned_notes", "actor-b").await, 0);
    assert_eq!(count(&pool, "owned_notes", "actor-a").await, 1);
}

#[tokio::test]
async fn update_many_cannot_move_rows_out_of_the_callers_write_scope() {
    let (db, pool) = database().await;
    create_note(&db, "actor-a", "one").await;
    create_note(&db, "actor-a", "two").await;
    let response = run(
        &schema(&db, "actor-a"),
        "mutation { updateOwnedNotes(where: { name: { eq: \"secret\" } }, input: { ownerUserId: \"actor-b\" }) { success errorCode affectedCount } }",
    )
    .await;
    let result = &response["data"]["updateOwnedNotes"];
    assert_eq!(result["success"], false, "{response}");
    assert_eq!(result["errorCode"], "FORBIDDEN", "{response}");
    assert_eq!(count(&pool, "owned_notes", "actor-b").await, 0);
    assert_eq!(count(&pool, "owned_notes", "actor-a").await, 2);
}

#[tokio::test]
async fn owners_still_receive_and_move_rows_within_their_scope() {
    let (db, pool) = database().await;
    let id = create_note(&db, "actor-a", "a").await;
    let response = run(
        &schema(&db, "actor-a"),
        format!(
            "mutation {{ updateOwnedNote(id: \"{id}\", input: {{ name: \"renamed\" }}) {{ success ownedNote {{ name }} }} }}"
        ),
    )
    .await;
    assert_eq!(
        response["data"]["updateOwnedNote"]["ownedNote"]["name"], "renamed",
        "{response}"
    );
    // A writer may place a row anywhere it may write.
    let moved = run(
        &schema(&db, "writer"),
        format!(
            "mutation {{ updateOwnedNote(id: \"{id}\", input: {{ ownerUserId: \"actor-b\" }}) {{ success }} }}"
        ),
    )
    .await;
    assert_eq!(moved["data"]["updateOwnedNote"]["success"], true, "{moved}");
    assert_eq!(count(&pool, "owned_notes", "actor-b").await, 1);
}

#[tokio::test]
async fn update_does_not_return_a_row_the_caller_cannot_read() {
    let (db, pool) = database().await;
    let id = create_note(&db, "actor-a", "a").await;
    let response = run(
        &schema(&db, "writer"),
        format!(
            "mutation {{ updateOwnedNote(id: \"{id}\", input: {{ name: \"renamed\" }}) {{ success ownedNote {{ id name ownerUserId }} }} }}"
        ),
    )
    .await;
    let result = &response["data"]["updateOwnedNote"];
    assert_eq!(result["success"], true, "{response}");
    assert!(
        result["ownedNote"].is_null(),
        "a write-only caller must not receive the row: {response}"
    );
    assert!(response["errors"].is_null(), "{response}");
    assert_eq!(note_name(&pool, &id).await, "renamed");
}

#[tokio::test]
async fn upsert_does_not_return_a_row_the_caller_cannot_read() {
    let (db, pool) = database().await;
    let id = create_note(&db, "actor-a", "a").await;
    let writer = schema(&db, "writer");
    let updated = run(
        &writer,
        "mutation { upsertOwnedNote(input: { name: \"renamed\", slug: \"a\", ownerUserId: \"actor-a\" }) { success action ownedNote { id name } } }",
    )
    .await;
    let result = &updated["data"]["upsertOwnedNote"];
    assert_eq!(result["success"], true, "{updated}");
    assert_eq!(result["action"], "UPDATED", "{updated}");
    assert!(result["ownedNote"].is_null(), "{updated}");
    assert_eq!(note_name(&pool, &id).await, "renamed");

    let created = run(
        &writer,
        "mutation { upsertOwnedNote(input: { name: \"fresh\", slug: \"b\", ownerUserId: \"actor-a\" }) { success action ownedNote { id } } }",
    )
    .await;
    let result = &created["data"]["upsertOwnedNote"];
    assert_eq!(result["action"], "CREATED", "{created}");
    assert!(result["ownedNote"].is_null(), "{created}");
}

#[tokio::test]
async fn single_writes_answer_hidden_rows_like_missing_rows() {
    let (db, pool) = database().await;
    let hidden = create_note(&db, "actor-a", "a").await;
    let missing = graphql_orm::uuid::Uuid::new_v4().to_string();
    let actor_b = schema(&db, "actor-b");
    for template in [
        "mutation { updateOwnedNote(id: \"ID\", input: { name: \"probe\" }) { success error ownedNote { id } } }",
        "mutation { deleteOwnedNote(id: \"ID\") { success error } }",
    ] {
        let hidden_response = run(&actor_b, template.replace("ID", &hidden)).await;
        let missing_response = run(&actor_b, template.replace("ID", &missing)).await;
        assert_eq!(hidden_response, missing_response, "{template}");
        assert_eq!(
            missing_response["data"]
                .as_object()
                .unwrap()
                .values()
                .next()
                .unwrap()["error"],
            "Entity not found"
        );
    }
    assert_eq!(note_name(&pool, &hidden).await, "secret");

    // A row the caller can read stays a refusal: its existence is already known.
    let reader = run(
        &schema(&db, "reader"),
        format!(
            "mutation {{ updateOwnedNote(id: \"{hidden}\", input: {{ name: \"probe\" }}) {{ success }} }}"
        ),
    )
    .await;
    assert_eq!(error_code(&reader), Some("FORBIDDEN"), "{reader}");
}

#[tokio::test]
async fn bulk_writes_answer_hidden_rows_like_missing_rows() {
    let (db, pool) = database().await;
    create_note(&db, "actor-a", "a").await;
    let own = create_note(&db, "actor-b", "b").await;
    let actor_b = schema(&db, "actor-b");
    for (field, selection) in [
        (
            "updateOwnedNotes",
            "input: { name: \"probe\" }) { success error errorCode affectedCount }",
        ),
        (
            "deleteOwnedNotes",
            ") { success error errorCode deletedCount }",
        ),
    ] {
        let probe = |slug: &str| {
            format!("mutation {{ {field}(where: {{ slug: {{ eq: \"{slug}\" }} }} {selection} }}")
        };
        let hidden_response = run(&actor_b, probe("a")).await;
        let missing_response = run(&actor_b, probe("absent")).await;
        assert_eq!(hidden_response, missing_response, "{field}");
        assert_eq!(
            missing_response["data"][field]["success"], true,
            "{missing_response}"
        );
    }
    assert_eq!(count(&pool, "owned_notes", "actor-a").await, 1);
    assert_eq!(note_name(&pool, &own).await, "secret");

    // A filter spanning hidden and own rows applies to the caller's rows only.
    let spanning = run(
        &actor_b,
        "mutation { updateOwnedNotes(where: { name: { eq: \"secret\" } }, input: { name: \"mine\" }) { success affectedCount } }",
    )
    .await;
    assert_eq!(
        spanning["data"]["updateOwnedNotes"]["affectedCount"], 1,
        "{spanning}"
    );
    assert_eq!(note_name(&pool, &own).await, "mine");
    assert_eq!(count(&pool, "owned_notes", "actor-a").await, 1);

    // Readable but unwritable rows still refuse the whole bulk write.
    let reader = run(
        &schema(&db, "reader"),
        "mutation { deleteOwnedNotes(where: { slug: { eq: \"a\" } }) { success errorCode deletedCount } }",
    )
    .await;
    assert_eq!(
        reader["data"]["deleteOwnedNotes"]["success"], false,
        "{reader}"
    );
    assert_eq!(
        reader["data"]["deleteOwnedNotes"]["errorCode"], "FORBIDDEN",
        "{reader}"
    );
    assert_eq!(count(&pool, "owned_notes", "actor-a").await, 1);
}

#[tokio::test]
async fn repository_writes_check_the_written_row() {
    let (db, pool) = database().await;
    let input = |slug: &str, owner: &str| CreateOwnedNoteInput {
        name: "repo".to_string(),
        slug: slug.to_string(),
        owner_user_id: owner.to_string(),
    };
    assert!(
        OwnedNote::insert(&db, input("other", "actor-b"))
            .await
            .is_err()
    );
    assert!(
        OwnedNote::insert_many(
            &db,
            [input("ok", REPOSITORY_ACTOR), input("bad", "actor-b")]
        )
        .await
        .is_err()
    );
    assert_eq!(count(&pool, "owned_notes", REPOSITORY_ACTOR).await, 0);

    let note = OwnedNote::insert(&db, input("mine", REPOSITORY_ACTOR))
        .await
        .unwrap();
    let moved = OwnedNote::update_by_id(
        &db,
        &note.id,
        UpdateOwnedNoteInput {
            owner_user_id: Some("actor-b".to_string()),
            ..Default::default()
        },
    )
    .await;
    assert!(moved.is_err(), "{moved:?}");
    assert!(
        OwnedNote::upsert(&db, input("mine", "actor-b"))
            .await
            .is_err()
    );
    assert!(
        OwnedNote::upsert(&db, input("fresh", "actor-b"))
            .await
            .is_err()
    );
    assert_eq!(count(&pool, "owned_notes", "actor-b").await, 0);
    assert_eq!(count(&pool, "owned_notes", REPOSITORY_ACTOR).await, 1);
}

#[tokio::test]
async fn composite_key_writes_check_the_written_row() {
    let (db, pool) = database().await;
    let pair = |item: &str, owner: &str| CreateOwnedPairInput {
        tenant_key: "t".to_string(),
        item_key: item.to_string(),
        owner_user_id: owner.to_string(),
    };
    assert!(
        OwnedPair::insert(&db, pair("other", "actor-b"))
            .await
            .is_err()
    );
    OwnedPair::insert(&db, pair("mine", REPOSITORY_ACTOR))
        .await
        .unwrap();
    let key = OwnedPairKey {
        tenant_key: "t".to_string(),
        item_key: "mine".to_string(),
    };
    let moved = OwnedPair::update_by_key(
        &db,
        &key,
        UpdateOwnedPairInput {
            owner_user_id: Some("actor-b".to_string()),
        },
    )
    .await;
    assert!(moved.is_err(), "{moved:?}");
    assert!(
        OwnedPair::upsert(&db, pair("mine", "actor-b"))
            .await
            .is_err()
    );
    assert!(
        OwnedPair::upsert(&db, pair("fresh", "actor-b"))
            .await
            .is_err()
    );
    assert_eq!(count(&pool, "owned_pairs", "actor-b").await, 0);
    assert_eq!(count(&pool, "owned_pairs", REPOSITORY_ACTOR).await, 1);

    let response = run(
        &schema(&db, "actor-a"),
        "mutation { createOwnedEvent(input: { id: 1, sequence: 1, ownerUserId: \"actor-b\" }) { success } }",
    )
    .await;
    assert_eq!(error_code(&response), Some("FORBIDDEN"), "{response}");
    assert_eq!(count(&pool, "owned_events", "actor-b").await, 0);
}
