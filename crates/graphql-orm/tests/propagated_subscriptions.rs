use graphql_orm::async_graphql::{Request, Response, Schema};
use graphql_orm::futures::{StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::sync::OnceLock;
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "propagation_collections",
    plural = "PropagationCollections",
    default_sort = "name ASC"
)]
struct Collection {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "propagation_records",
    plural = "PropagationRecords",
    default_sort = "title ASC"
)]
struct Record {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub collection_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,

    #[graphql(skip)]
    #[relation(
        target = "Collection",
        from = "collection_id",
        to = "id",
        propagate_change = "up"
    )]
    pub collection: Option<Collection>,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "propagation_notes",
    plural = "PropagationNotes",
    default_sort = "id ASC"
)]
struct Note {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub record_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    pub body: String,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,

    #[graphql(skip)]
    #[relation(
        target = "Record",
        from = "record_id",
        to = "id",
        propagate_change = "up"
    )]
    pub record: Option<Record>,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "propagation_record_relationships",
    plural = "PropagationRecordRelationships",
    default_sort = "id ASC"
)]
struct RecordRelationship {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub source_record_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub target_record_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    pub kind: String,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,

    #[graphql(skip)]
    #[relation(
        target = "Record",
        from = "source_record_id",
        to = "id",
        propagate_change = "up"
    )]
    pub source_record: Option<Record>,

    #[graphql(skip)]
    #[relation(
        target = "Record",
        from = "target_record_id",
        to = "id",
        propagate_change = "up"
    )]
    pub target_record: Option<Record>,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "propagation_nodes",
    plural = "PropagationNodes",
    default_sort = "name ASC"
)]
struct Node {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "string")]
    pub parent_id: Option<String>,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,

    #[graphql(skip)]
    #[relation(
        target = "Node",
        from = "parent_id",
        to = "id",
        propagate_change = "up"
    )]
    pub parent_node: Option<Box<Node>>,
}

schema_roots! {
    query_custom_ops: [],
    entities: [Collection, Record, Note, RecordRelationship, Node],
}

macro_rules! impl_noop_relation_loader {
    ($($entity:ty),* $(,)?) => {
        $(
            impl graphql_orm::graphql::orm::RelationLoader for $entity {
                async fn load_relations(
                    &mut self,
                    _pool: &graphql_orm::DbPool,
                    _selection: &[graphql_orm::async_graphql::context::SelectionField<'_>],
                ) -> Result<(), sqlx::Error> {
                    Ok(())
                }

                async fn bulk_load_relations(
                    _entities: &mut [Self],
                    _pool: &graphql_orm::DbPool,
                    _selection: &[graphql_orm::async_graphql::context::SelectionField<'_>],
                ) -> Result<(), sqlx::Error> {
                    Ok(())
                }
            }
        )*
    };
}

impl_noop_relation_loader!(Record, Note, RecordRelationship, Node);

type TestSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

fn test_mutex() -> &'static tokio::sync::Mutex<()> {
    static TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(feature = "sqlite")]
type TestPool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type TestPool = sqlx::PgPool;

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE propagation_collections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE propagation_records (
            id TEXT PRIMARY KEY,
            collection_id TEXT NOT NULL,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE propagation_notes (
            id TEXT PRIMARY KEY,
            record_id TEXT NOT NULL,
            body TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE propagation_record_relationships (
            id TEXT PRIMARY KEY,
            source_record_id TEXT NOT NULL,
            target_record_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE propagation_nodes (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            parent_id TEXT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;
    for table in [
        "propagation_record_relationships",
        "propagation_notes",
        "propagation_records",
        "propagation_collections",
        "propagation_nodes",
    ] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .execute(&pool)
            .await?;
    }
    sqlx::query(
        "CREATE TABLE propagation_collections (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE propagation_records (
            id UUID PRIMARY KEY,
            collection_id UUID NOT NULL,
            title TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE propagation_notes (
            id UUID PRIMARY KEY,
            record_id UUID NOT NULL,
            body TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE propagation_record_relationships (
            id UUID PRIMARY KEY,
            source_record_id UUID NOT NULL,
            target_record_id UUID NOT NULL,
            kind TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE propagation_nodes (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            parent_id TEXT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

fn schema(database: graphql_orm::db::Database) -> TestSchema {
    schema_builder(database)
        .data("test-user".to_string())
        .finish()
}

async fn wait_until_pending<S>(stream: &mut S)
where
    S: graphql_orm::futures::Stream<Item = Response> + Unpin,
{
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(Some(response)) => {
            panic!(
                "subscription yielded before mutation: {:?}",
                response.errors
            )
        }
        Poll::Ready(None) => panic!("subscription stream ended before mutation"),
    })
    .await;
}

async fn next_json<S>(stream: &mut S) -> serde_json::Value
where
    S: graphql_orm::futures::Stream<Item = Response> + Unpin,
{
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("subscription timed out")
        .expect("subscription stream ended unexpectedly");
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response
        .data
        .into_json()
        .expect("subscription data should be json")
}

fn graphql_uuid(id: graphql_orm::uuid::Uuid) -> String {
    id.to_string()
}

#[cfg(feature = "sqlite")]
async fn set_node_parent(
    pool: &sqlx::SqlitePool,
    id: &str,
    parent_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE propagation_nodes SET parent_id = ? WHERE id = ?")
        .bind(parent_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_node(
    pool: &sqlx::SqlitePool,
    id: &str,
    name: &str,
    parent_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO propagation_nodes (id, name, parent_id) VALUES (?, ?, ?)")
        .bind(id)
        .bind(name)
        .bind(parent_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn set_node_parent(
    pool: &sqlx::PgPool,
    id: &str,
    parent_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE propagation_nodes SET parent_id = $1 WHERE id = $2")
        .bind(parent_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_node(
    pool: &sqlx::PgPool,
    id: &str,
    name: &str,
    parent_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO propagation_nodes (id, name, parent_id) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(name)
        .bind(parent_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn direct_entity_mutation_emits_direct_change_event() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let schema = schema(graphql_orm::db::Database::new(pool));

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription {
                    collectionChanged {
                        action
                        changeKind
                        sourceEntity
                        sourceId
                        path
                        collection { id name }
                    }
                }",
            )
            .data("test-user".to_string()),
        ),
    );
    wait_until_pending(&mut stream).await;

    let created = schema
        .execute(
            Request::new(
                "mutation {
                    createCollection(input: { name: \"Main\" }) {
                        success
                        collection { id name }
                    }
                }",
            )
            .data("test-user".to_string()),
        )
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);

    let json = next_json(&mut stream).await;
    assert_eq!(
        json["collectionChanged"]["action"].as_str(),
        Some("CREATED")
    );
    assert_eq!(
        json["collectionChanged"]["changeKind"].as_str(),
        Some("DIRECT")
    );
    assert!(json["collectionChanged"]["sourceEntity"].is_null());
    assert!(json["collectionChanged"]["sourceId"].is_null());
    assert_eq!(json["collectionChanged"]["path"], serde_json::json!([]));
    assert_eq!(
        json["collectionChanged"]["collection"]["name"].as_str(),
        Some("Main")
    );

    Ok(())
}

#[tokio::test]
async fn child_mutation_emits_parent_propagated_event() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let schema = schema(graphql_orm::db::Database::new(pool));

    let created_collection = schema
        .execute(
            Request::new(
                "mutation {
                    createCollection(input: { name: \"Parent\" }) {
                        collection { id }
                    }
                }",
            )
            .data("test-user".to_string()),
        )
        .await;
    assert!(
        created_collection.errors.is_empty(),
        "{:?}",
        created_collection.errors
    );
    let collection_id =
        created_collection.data.into_json()?["createCollection"]["collection"]["id"]
            .as_str()
            .expect("collection id")
            .to_string();

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription {
                    collectionChanged {
                        action
                        changeKind
                        sourceEntity
                        sourceId
                        path
                        collection { id name }
                    }
                }",
            )
            .data("test-user".to_string()),
        ),
    );
    wait_until_pending(&mut stream).await;

    let created_record = schema
        .execute(
            Request::new(format!(
                "mutation {{
                    createRecord(input: {{ collectionId: \"{collection_id}\", title: \"Child\" }}) {{
                        success
                        record {{ id }}
                    }}
                }}"
            ))
            .data("test-user".to_string()),
        )
        .await;
    assert!(
        created_record.errors.is_empty(),
        "{:?}",
        created_record.errors
    );
    let record_id = created_record.data.into_json()?["createRecord"]["record"]["id"]
        .as_str()
        .expect("record id")
        .to_string();

    let json = next_json(&mut stream).await;
    assert_eq!(
        json["collectionChanged"]["action"].as_str(),
        Some("CREATED")
    );
    assert_eq!(
        json["collectionChanged"]["changeKind"].as_str(),
        Some("PROPAGATED")
    );
    assert_eq!(
        json["collectionChanged"]["sourceEntity"].as_str(),
        Some("Record")
    );
    assert_eq!(
        json["collectionChanged"]["sourceId"].as_str(),
        Some(record_id.as_str())
    );
    assert_eq!(
        json["collectionChanged"]["path"],
        serde_json::json!(["collection"])
    );
    assert_eq!(
        json["collectionChanged"]["collection"]["id"].as_str(),
        Some(collection_id.as_str())
    );

    Ok(())
}

#[tokio::test]
async fn multi_parent_propagation_reaches_both_parents_and_deduplicates_same_parent()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let schema = schema(graphql_orm::db::Database::new(pool));

    let collection = schema
        .execute(
            Request::new(
                "mutation {
                    createCollection(input: { name: \"Scope\" }) {
                        collection { id }
                    }
                }",
            )
            .data("test-user".to_string()),
        )
        .await;
    assert!(collection.errors.is_empty(), "{:?}", collection.errors);
    let collection_id = collection.data.into_json()?["createCollection"]["collection"]["id"]
        .as_str()
        .expect("collection id")
        .to_string();

    let first_record = schema
        .execute(
            Request::new(format!(
                "mutation {{
                    createRecord(input: {{ collectionId: \"{collection_id}\", title: \"One\" }}) {{
                        record {{ id }}
                    }}
                }}"
            ))
            .data("test-user".to_string()),
        )
        .await;
    assert!(first_record.errors.is_empty(), "{:?}", first_record.errors);
    let first_record_id = first_record.data.into_json()?["createRecord"]["record"]["id"]
        .as_str()
        .expect("first record id")
        .to_string();

    let second_record = schema
        .execute(
            Request::new(format!(
                "mutation {{
                    createRecord(input: {{ collectionId: \"{collection_id}\", title: \"Two\" }}) {{
                        record {{ id }}
                    }}
                }}"
            ))
            .data("test-user".to_string()),
        )
        .await;
    assert!(
        second_record.errors.is_empty(),
        "{:?}",
        second_record.errors
    );
    let second_record_id = second_record.data.into_json()?["createRecord"]["record"]["id"]
        .as_str()
        .expect("second record id")
        .to_string();

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription {
                    recordChanged {
                        action
                        changeKind
                        sourceEntity
                        sourceId
                        path
                        record { id title }
                    }
                }",
            )
            .data("test-user".to_string()),
        ),
    );
    wait_until_pending(&mut stream).await;

    let relationship = schema
        .execute(
            Request::new(format!(
                "mutation {{
                    createRecordRelationship(input: {{
                        sourceRecordId: \"{first_record_id}\"
                        targetRecordId: \"{second_record_id}\"
                        kind: \"related\"
                    }}) {{
                        recordRelationship {{ id }}
                    }}
                }}"
            ))
            .data("test-user".to_string()),
        )
        .await;
    assert!(relationship.errors.is_empty(), "{:?}", relationship.errors);
    let relationship_id =
        relationship.data.into_json()?["createRecordRelationship"]["recordRelationship"]["id"]
            .as_str()
            .expect("relationship id")
            .to_string();

    let first_event = next_json(&mut stream).await["recordChanged"].clone();
    let second_event = next_json(&mut stream).await["recordChanged"].clone();
    let mut received = vec![
        (
            first_event["record"]["id"].as_str().unwrap().to_string(),
            first_event["path"][0].as_str().unwrap().to_string(),
        ),
        (
            second_event["record"]["id"].as_str().unwrap().to_string(),
            second_event["path"][0].as_str().unwrap().to_string(),
        ),
    ];
    received.sort();
    let mut expected = vec![
        (first_record_id.clone(), "sourceRecord".to_string()),
        (second_record_id.clone(), "targetRecord".to_string()),
    ];
    expected.sort();
    assert_eq!(received, expected);
    assert_eq!(first_event["changeKind"].as_str(), Some("PROPAGATED"));
    assert_eq!(
        first_event["sourceEntity"].as_str(),
        Some("RecordRelationship")
    );
    assert_eq!(
        first_event["sourceId"].as_str(),
        Some(relationship_id.as_str())
    );
    assert_eq!(second_event["changeKind"].as_str(), Some("PROPAGATED"));
    assert_eq!(
        second_event["sourceId"].as_str(),
        Some(relationship_id.as_str())
    );

    let duplicate_stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription {
                    recordChanged {
                        changeKind
                        sourceId
                        path
                        record { id }
                    }
                }",
            )
            .data("test-user".to_string()),
        ),
    );
    tokio::pin!(duplicate_stream);
    wait_until_pending(&mut duplicate_stream).await;

    let duplicate_relationship = schema
        .execute(
            Request::new(format!(
                "mutation {{
                    createRecordRelationship(input: {{
                        sourceRecordId: \"{first_record_id}\"
                        targetRecordId: \"{first_record_id}\"
                        kind: \"duplicate\"
                    }}) {{
                        recordRelationship {{ id }}
                    }}
                }}"
            ))
            .data("test-user".to_string()),
        )
        .await;
    assert!(
        duplicate_relationship.errors.is_empty(),
        "{:?}",
        duplicate_relationship.errors
    );

    let duplicate_json = next_json(&mut duplicate_stream).await;
    assert_eq!(
        duplicate_json["recordChanged"]["record"]["id"].as_str(),
        Some(first_record_id.as_str())
    );
    assert_eq!(
        duplicate_json["recordChanged"]["changeKind"].as_str(),
        Some("PROPAGATED")
    );
    assert!(
        timeout(Duration::from_millis(250), duplicate_stream.next())
            .await
            .is_err(),
        "duplicate propagation emitted more than once for the same parent"
    );

    Ok(())
}

#[tokio::test]
async fn delete_propagation_uses_pre_delete_state() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool);
    let schema = schema(database.clone());

    let collection = Collection::insert(
        &database,
        CreateCollectionInput {
            name: "Owned".to_string(),
        },
    )
    .await?;
    let record = Record::insert(
        &database,
        CreateRecordInput {
            collection_id: collection.id,
            title: "Document".to_string(),
        },
    )
    .await?;
    let note = Note::insert(
        &database,
        CreateNoteInput {
            record_id: record.id,
            body: "Hello".to_string(),
        },
    )
    .await?;

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription {
                    recordChanged {
                        action
                        changeKind
                        sourceEntity
                        sourceId
                        path
                        record { id title }
                    }
                }",
            )
            .data("test-user".to_string()),
        ),
    );
    wait_until_pending(&mut stream).await;

    let deleted = schema
        .execute(
            Request::new(format!(
                "mutation {{
                    deleteNote(id: \"{}\") {{
                        success
                    }}
                }}",
                graphql_uuid(note.id)
            ))
            .data("test-user".to_string()),
        )
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);

    let json = next_json(&mut stream).await;
    assert_eq!(json["recordChanged"]["action"].as_str(), Some("DELETED"));
    assert_eq!(
        json["recordChanged"]["changeKind"].as_str(),
        Some("PROPAGATED")
    );
    assert_eq!(json["recordChanged"]["sourceEntity"].as_str(), Some("Note"));
    assert_eq!(
        json["recordChanged"]["sourceId"].as_str(),
        Some(graphql_uuid(note.id).as_str())
    );
    assert_eq!(json["recordChanged"]["path"], serde_json::json!(["record"]));
    assert_eq!(
        json["recordChanged"]["record"]["id"].as_str(),
        Some(graphql_uuid(record.id).as_str())
    );

    Ok(())
}

#[tokio::test]
async fn propagation_cycles_do_not_reemit_source_indefinitely()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let schema = schema(database.clone());

    insert_node(&pool, "node-a", "A", None).await?;
    insert_node(&pool, "node-b", "B", Some("node-a")).await?;
    let a = Node::get(&pool, &"node-a".to_string())
        .await?
        .expect("node a should exist");
    let b = Node::get(&pool, &"node-b".to_string())
        .await?
        .expect("node b should exist");
    set_node_parent(&pool, &a.id, &b.id).await?;

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription {
                    nodeChanged {
                        action
                        changeKind
                        path
                        node { id name }
                    }
                }",
            )
            .data("test-user".to_string()),
        ),
    );
    wait_until_pending(&mut stream).await;

    let deleted = schema
        .execute(
            Request::new(format!(
                "mutation {{
                    deleteNode(id: \"{}\") {{
                        success
                    }}
                }}",
                a.id
            ))
            .data("test-user".to_string()),
        )
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);

    let direct = next_json(&mut stream).await;
    let propagated = next_json(&mut stream).await;
    assert_eq!(direct["nodeChanged"]["action"].as_str(), Some("DELETED"));
    assert_eq!(direct["nodeChanged"]["changeKind"].as_str(), Some("DIRECT"));
    assert_eq!(
        direct["nodeChanged"]["node"]["id"].as_str(),
        Some(a.id.as_str())
    );
    assert_eq!(
        propagated["nodeChanged"]["changeKind"].as_str(),
        Some("PROPAGATED")
    );
    assert_eq!(
        propagated["nodeChanged"]["node"]["id"].as_str(),
        Some(b.id.as_str())
    );
    assert_eq!(
        propagated["nodeChanged"]["path"],
        serde_json::json!(["parentNode"])
    );
    assert!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .is_err(),
        "cycle propagation emitted an extra event after the direct and parent events"
    );

    Ok(())
}
