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

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "versioned_notes",
    plural = "VersionedNotes",
    default_sort = "id ASC"
)]
struct VersionedNote {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    owner_user_id: String,
    #[graphql_orm(version, default = "0")]
    version: i64,
}

/// Declares an entity write policy, which is also its row write policy key.
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "guarded_notes",
    plural = "GuardedNotes",
    default_sort = "name ASC",
    write_policy = "notes.write"
)]
struct GuardedNote {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [OwnedNote, OwnedPair, OwnedEvent, VersionedNote, GuardedNote],
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
    } else if let Some(note) = row.downcast_ref::<VersionedNote>() {
        Some(&note.owner_user_id)
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
        "CREATE TABLE versioned_notes (id TEXT PRIMARY KEY, owner_user_id TEXT NOT NULL, version INTEGER NOT NULL DEFAULT 0)",
        "CREATE TABLE guarded_notes (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
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

/// The public code of a refused repository call.
fn refusal<T: std::fmt::Debug>(result: graphql_orm::Result<T>) -> &'static str {
    OrmPublicError::from(result.expect_err("write must be refused"))
        .code
        .as_str()
}

fn slug(value: &str) -> OwnedNoteWhereInput {
    OwnedNoteWhereInput {
        slug: Some(graphql_orm::graphql::filters::StringFilter {
            eq: Some(value.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn move_note_to(owner: &str) -> UpdateOwnedNoteInput {
    UpdateOwnedNoteInput {
        owner_user_id: Some(owner.to_string()),
        ..Default::default()
    }
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
    assert_eq!(
        refusal(OwnedNote::insert(&db, input("other", "actor-b")).await),
        "FORBIDDEN"
    );
    assert_eq!(
        refusal(
            OwnedNote::insert_many(
                &db,
                [input("ok", REPOSITORY_ACTOR), input("bad", "actor-b")]
            )
            .await
        ),
        "FORBIDDEN"
    );
    assert_eq!(count(&pool, "owned_notes", REPOSITORY_ACTOR).await, 0);

    let note = OwnedNote::insert(&db, input("mine", REPOSITORY_ACTOR))
        .await
        .unwrap();
    assert_eq!(
        refusal(OwnedNote::update_by_id(&db, &note.id, move_note_to("actor-b")).await),
        "FORBIDDEN"
    );
    assert_eq!(
        refusal(OwnedNote::update_where(&db, slug("mine"), move_note_to("actor-b")).await),
        "FORBIDDEN"
    );
    assert_eq!(
        refusal(OwnedNote::upsert(&db, input("mine", "actor-b")).await),
        "FORBIDDEN"
    );
    assert_eq!(
        refusal(OwnedNote::upsert(&db, input("fresh", "actor-b")).await),
        "FORBIDDEN"
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
    assert_eq!(
        refusal(OwnedPair::insert(&db, pair("other", "actor-b")).await),
        "FORBIDDEN"
    );
    OwnedPair::insert(&db, pair("mine", REPOSITORY_ACTOR))
        .await
        .unwrap();
    let key = OwnedPairKey {
        tenant_key: "t".to_string(),
        item_key: "mine".to_string(),
    };
    let to_b = || UpdateOwnedPairInput {
        owner_user_id: Some("actor-b".to_string()),
    };
    assert_eq!(
        refusal(OwnedPair::update_by_key(&db, &key, to_b()).await),
        "FORBIDDEN"
    );
    let expected = OwnedPairWhereInput {
        owner_user_id: Some(graphql_orm::graphql::filters::StringFilter {
            eq: Some(REPOSITORY_ACTOR.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        refusal(OwnedPair::update_if(&db, &key, expected, to_b()).await),
        "FORBIDDEN"
    );
    assert_eq!(
        refusal(OwnedPair::upsert(&db, pair("mine", "actor-b")).await),
        "FORBIDDEN"
    );
    assert_eq!(
        refusal(OwnedPair::upsert(&db, pair("fresh", "actor-b")).await),
        "FORBIDDEN"
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

#[tokio::test]
async fn compare_and_swap_checks_the_written_row_and_rolls_back_its_savepoint() {
    let (db, pool) = database().await;
    VersionedNote::insert(
        &db,
        CreateVersionedNoteInput {
            id: "v".to_string(),
            owner_user_id: REPOSITORY_ACTOR.to_string(),
        },
    )
    .await
    .unwrap();
    let swap = |owner: &str| UpdateVersionedNoteInput {
        owner_user_id: Some(owner.to_string()),
    };
    let refused = VersionedNote::compare_and_swap(
        &db,
        &"v".to_string(),
        0,
        VersionedNoteWhereInput::default(),
        swap("actor-b"),
    )
    .await;
    assert_eq!(refusal(refused), "FORBIDDEN");
    assert_eq!(count(&pool, "versioned_notes", "actor-b").await, 0);

    // Inside a caller-owned transaction the refused swap is undone at once, and
    // the transaction cannot commit even if the caller ignores the error.
    let outcome = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let ignored = tx
                    .compare_and_swap::<VersionedNote>(
                        &"v".to_string(),
                        0,
                        VersionedNoteWhereInput::default(),
                        swap("actor-b"),
                    )
                    .await;
                assert!(ignored.is_err());
                let current = tx
                    .find_by_id::<VersionedNote>(&"v".to_string())
                    .await
                    .map_err(OrmPublicError::from)?
                    .expect("row still present");
                assert_eq!(current.owner_user_id, REPOSITORY_ACTOR);
                assert_eq!(current.version, 0);
                Ok(())
            })
        })
        .await;
    assert!(
        matches!(
            &outcome,
            Err(TransactionError::Rejected(error)) if error.code == OrmErrorCode::Forbidden
        ),
        "{outcome:?}"
    );
    assert_eq!(count(&pool, "versioned_notes", REPOSITORY_ACTOR).await, 1);
}

#[tokio::test]
async fn a_refused_written_row_poisons_the_callers_transaction() {
    let (db, pool) = database().await;
    let input = |slug: &str, owner: &str| CreateOwnedNoteInput {
        name: "tx".to_string(),
        slug: slug.to_string(),
        owner_user_id: owner.to_string(),
    };
    let outcome = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.insert::<OwnedNote>(input("allowed", REPOSITORY_ACTOR))
                    .await
                    .map_err(OrmPublicError::from)?;
                // The host swallows the refusal and tries to commit anyway.
                let _ = tx.insert::<OwnedNote>(input("refused", "actor-b")).await;
                Ok(())
            })
        })
        .await;
    assert!(
        matches!(
            &outcome,
            Err(TransactionError::Rejected(error)) if error.code == OrmErrorCode::Forbidden
        ),
        "{outcome:?}"
    );
    assert_eq!(count(&pool, "owned_notes", "actor-b").await, 0);
    assert_eq!(count(&pool, "owned_notes", REPOSITORY_ACTOR).await, 0);

    let mut context =
        graphql_orm::graphql::orm::MutationContext::begin(&db, TransactionMode::StateMachine)
            .await
            .unwrap();
    let note = context
        .insert::<OwnedNote>(input("mine", REPOSITORY_ACTOR))
        .await
        .unwrap();
    let _ = context
        .update_by_id::<OwnedNote>(&note.id, move_note_to("actor-b"))
        .await;
    assert_eq!(refusal(context.commit_and_emit().await), "FORBIDDEN");
    assert_eq!(count(&pool, "owned_notes", "actor-b").await, 0);
    assert_eq!(count(&pool, "owned_notes", REPOSITORY_ACTOR).await, 0);
}

#[tokio::test]
async fn graphql_refusals_for_readable_rows_and_hidden_upsert_targets() {
    let (db, pool) = database().await;
    let hidden = create_note(&db, "actor-a", "a").await;
    let reader = run(
        &schema(&db, "reader"),
        format!("mutation {{ deleteOwnedNote(id: \"{hidden}\") {{ success }} }}"),
    )
    .await;
    assert_eq!(error_code(&reader), Some("FORBIDDEN"), "{reader}");

    // The upsert key is unique, so a hidden conflicting row cannot look absent.
    let upsert = run(
        &schema(&db, "actor-b"),
        "mutation { upsertOwnedNote(input: { name: \"probe\", slug: \"a\", ownerUserId: \"actor-b\" }) { success } }",
    )
    .await;
    assert_eq!(error_code(&upsert), Some("FORBIDDEN"), "{upsert}");
    assert_eq!(note_name(&pool, &hidden).await, "secret");
    assert_eq!(count(&pool, "owned_notes", "actor-a").await, 1);
}

#[tokio::test]
async fn owners_receive_their_upserted_row() {
    let (db, _pool) = database().await;
    create_note(&db, "actor-a", "a").await;
    let response = run(
        &schema(&db, "actor-a"),
        "mutation { upsertOwnedNote(input: { name: \"renamed\", slug: \"a\", ownerUserId: \"actor-a\" }) { action ownedNote { name } } }",
    )
    .await;
    assert_eq!(
        response["data"]["upsertOwnedNote"]["action"], "UPDATED",
        "{response}"
    );
    assert_eq!(
        response["data"]["upsertOwnedNote"]["ownedNote"]["name"], "renamed",
        "{response}"
    );
}

#[tokio::test]
async fn delete_many_spanning_hidden_and_own_rows_deletes_only_the_callers() {
    let (db, pool) = database().await;
    create_note(&db, "actor-a", "a").await;
    create_note(&db, "actor-b", "b").await;
    let response = run(
        &schema(&db, "actor-b"),
        "mutation { deleteOwnedNotes(where: { name: { eq: \"secret\" } }) { success deletedCount } }",
    )
    .await;
    assert_eq!(
        response["data"]["deleteOwnedNotes"]["success"], true,
        "{response}"
    );
    assert_eq!(
        response["data"]["deleteOwnedNotes"]["deletedCount"], 1,
        "{response}"
    );
    assert_eq!(count(&pool, "owned_notes", "actor-a").await, 1);
    assert_eq!(count(&pool, "owned_notes", "actor-b").await, 0);
}

#[tokio::test]
async fn a_declared_write_policy_without_a_row_provider_is_misconfigured_in_every_mode() {
    use graphql_orm::graphql::auth::AuthorizationMode;
    for mode in [
        AuthorizationMode::LegacyPermissive,
        AuthorizationMode::DeclaredPoliciesRequired,
        AuthorizationMode::ExplicitPolicyForAllExposedOperations,
    ] {
        let (_, pool) = database().await;
        let mut db = graphql_orm::db::Database::new(pool.clone())
            .with_authorized_scan_config(AuthorizedScanConfig::new([5; 32], 8, 64))
            .with_authorization_mode(mode);
        db.set_entity_policy(AllowEntities);
        let schema = schema(&db, "actor-a");
        // Refused before any lookup: an absent id gets the same answer.
        let missing = graphql_orm::uuid::Uuid::new_v4();
        for query in [
            "mutation { createGuardedNote(input: { name: \"x\" }) { success } }".to_string(),
            format!(
                "mutation {{ updateGuardedNote(id: \"{missing}\", input: {{ name: \"x\" }}) {{ success }} }}"
            ),
            format!("mutation {{ deleteGuardedNote(id: \"{missing}\") {{ success }} }}"),
        ] {
            let response = run(&schema, query).await;
            assert_eq!(
                error_code(&response),
                Some("AUTHORIZATION_MISCONFIGURED"),
                "{mode:?}: {response}"
            );
        }
        for (field, selection) in [
            (
                "updateGuardedNotes",
                "input: { name: \"x\" }) { success errorCode }",
            ),
            ("deleteGuardedNotes", ") { success errorCode }"),
        ] {
            let response = run(
                &schema,
                format!("mutation {{ {field}(where: {{ name: {{ eq: \"x\" }} }} {selection} }}"),
            )
            .await;
            assert_eq!(
                response["data"][field]["errorCode"], "AUTHORIZATION_MISCONFIGURED",
                "{mode:?}: {response}"
            );
        }
        assert_eq!(
            refusal(
                GuardedNote::insert(
                    &db,
                    CreateGuardedNoteInput {
                        name: "x".to_string()
                    }
                )
                .await
            ),
            "AUTHORIZATION_MISCONFIGURED",
            "{mode:?}"
        );
        assert_eq!(
            refusal(GuardedNote::delete_by_id(&db, &missing).await),
            "AUTHORIZATION_MISCONFIGURED",
            "{mode:?}"
        );
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM guarded_notes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }
}

/// Refuses the After hook of a note named `after-refuses`, and the Before hook of
/// a versioned-note update, which compare-and-swap runs after its `UPDATE`.
struct RefusingHook;
impl graphql_orm::graphql::orm::MutationHook for RefusingHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _context: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        use graphql_orm::graphql::orm::MutationPhase;
        Box::pin(async move {
            let named = |name: &str| {
                event
                    .after::<OwnedNote>()
                    .ok()
                    .flatten()
                    .is_some_and(|note| note.name == name)
            };
            if (event.entity_name == "OwnedNote"
                && event.phase == MutationPhase::After
                && named("after-refuses"))
                || (event.entity_name == "VersionedNote" && event.phase == MutationPhase::Before)
            {
                return Err(async_graphql::Error::new("hook refused"));
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn a_failure_after_a_write_poisons_the_callers_transaction() {
    let (mut db, pool) = database().await;
    db.set_mutation_hook(RefusingHook);
    // The After hook fails after the row is inserted; the caller ignores it.
    let outcome = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let ignored = tx
                    .insert::<OwnedNote>(CreateOwnedNoteInput {
                        name: "after-refuses".to_string(),
                        slug: "after".to_string(),
                        owner_user_id: REPOSITORY_ACTOR.to_string(),
                    })
                    .await;
                assert!(ignored.is_err());
                Ok(())
            })
        })
        .await;
    assert!(
        matches!(&outcome, Err(TransactionError::Rejected(_))),
        "{outcome:?}"
    );
    assert_eq!(count(&pool, "owned_notes", REPOSITORY_ACTOR).await, 0);

    sqlx::query("INSERT INTO versioned_notes (id, owner_user_id, version) VALUES ('v', ?, 0)")
        .bind(REPOSITORY_ACTOR)
        .execute(&pool)
        .await
        .unwrap();
    // Compare-and-swap runs its Before hook after the UPDATE: the savepoint undoes
    // the swap and the transaction still cannot commit.
    let outcome = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let ignored = tx
                    .compare_and_swap::<VersionedNote>(
                        &"v".to_string(),
                        0,
                        VersionedNoteWhereInput::default(),
                        UpdateVersionedNoteInput {
                            owner_user_id: Some(REPOSITORY_ACTOR.to_string()),
                        },
                    )
                    .await;
                assert!(ignored.is_err());
                let current = tx
                    .find_by_id::<VersionedNote>(&"v".to_string())
                    .await
                    .map_err(OrmPublicError::from)?
                    .expect("row still present");
                assert_eq!(current.version, 0);
                Ok(())
            })
        })
        .await;
    assert!(
        matches!(&outcome, Err(TransactionError::Rejected(_))),
        "{outcome:?}"
    );
    assert_eq!(count(&pool, "versioned_notes", REPOSITORY_ACTOR).await, 1);
}

#[tokio::test]
async fn a_refusal_before_any_write_leaves_the_transaction_usable() {
    let (db, pool) = database().await;
    sqlx::query(
        "INSERT INTO versioned_notes (id, owner_user_id, version) VALUES ('theirs', 'actor-b', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            // The existing row is not writable: CAS refuses before its UPDATE and
            // releases its savepoint.
            let refused = tx
                .compare_and_swap::<VersionedNote>(
                    &"theirs".to_string(),
                    0,
                    VersionedNoteWhereInput::default(),
                    UpdateVersionedNoteInput {
                        owner_user_id: Some(REPOSITORY_ACTOR.to_string()),
                    },
                )
                .await;
            assert_eq!(refusal(refused), "FORBIDDEN");
            tx.insert::<OwnedNote>(CreateOwnedNoteInput {
                name: "after".to_string(),
                slug: "after".to_string(),
                owner_user_id: REPOSITORY_ACTOR.to_string(),
            })
            .await
            .map_err(OrmPublicError::from)?;
            Ok(())
        })
    })
    .await
    .unwrap();
    assert_eq!(count(&pool, "owned_notes", REPOSITORY_ACTOR).await, 1);
    assert_eq!(count(&pool, "versioned_notes", "actor-b").await, 1);
}
