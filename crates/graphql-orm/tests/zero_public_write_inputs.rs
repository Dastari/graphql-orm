// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! Entities whose create or update input would have no public member generate
//! no empty GraphQL input object and no create, upsert, update or update-many
//! root for that input. Reads, relations, `Changed` subscriptions, repository
//! inputs and repository or transaction writes are unaffected.

use graphql_orm::async_graphql::Request;
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::graphql::orm::{GraphqlOperationKind, GraphqlOperationMetadata};
use graphql_orm::prelude::TransactionMode;
use std::task::Poll;
use tokio::time::{Duration, timeout};

mod entities {
    use graphql_orm::prelude::*;

    /// Zero-create and zero-update: the generated primary key and a
    /// server-owned `skip_input` column are the only persisted fields.
    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        GraphQLRelations,
        graphql_orm::async_graphql::SimpleObject,
        Clone,
        Debug,
        PartialEq,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql(complex)]
    #[graphql_entity(table = "accounts", plural = "Accounts", default_sort = "id ASC")]
    pub struct Account {
        #[primary_key]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql_orm(skip_input)]
        #[sortable]
        pub refreshed_at: String,
        #[graphql(skip)]
        #[serde(default)]
        #[relation(
            target = "Device",
            from = "id",
            to = "account_id",
            multiple,
            emit_fk = false
        )]
        pub devices: Vec<Device>,
    }

    /// Zero-update only: the caller supplies the key, and every other column
    /// is server owned.
    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        Clone,
        Debug,
        PartialEq,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql_entity(table = "badges", plural = "Badges", default_sort = "code ASC")]
    pub struct Badge {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub code: String,
        #[graphql_orm(skip_input)]
        pub issued_at: String,
    }

    /// Append-only with no publicly writable create field: rows are appended
    /// only by repository or transaction writes.
    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        Clone,
        Debug,
        PartialEq,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql_entity(
        table = "heartbeats",
        plural = "Heartbeats",
        default_sort = "id ASC",
        append_only = true
    )]
    pub struct Heartbeat {
        #[primary_key]
        #[sortable]
        pub id: String,
        #[graphql_orm(skip_input)]
        pub observed_at: String,
    }

    /// An ordinary writable entity in the same schema.
    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        GraphQLRelations,
        graphql_orm::async_graphql::SimpleObject,
        Clone,
        Debug,
        PartialEq,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql(complex)]
    #[graphql_entity(table = "devices", plural = "Devices", default_sort = "name ASC")]
    pub struct Device {
        #[primary_key]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        pub account_id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub name: String,
        #[graphql(skip)]
        #[serde(default)]
        #[relation(target = "Account", from = "account_id", to = "id", emit_fk = false)]
        pub account: Option<Account>,
    }

    macro_rules! batch_by_id {
        ($($entity:ty),*) => {$(
            impl BatchLoadEntity for $entity {
                fn batch_column() -> &'static str {
                    "id"
                }
                fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
                    use graphql_orm::sqlx::Row;
                    row.try_get("id")
                }
            }
        )*};
    }
    batch_by_id!(Account, Device);

    schema_roots! {
        query_custom_ops: [],
        entities: [Account, Badge, Device, Heartbeat],
    }

    pub async fn database() -> Database<SqliteBackend> {
        let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap();
        for statement in [
            "CREATE TABLE accounts (id TEXT PRIMARY KEY, refreshed_at TEXT NOT NULL)",
            "CREATE TABLE badges (code TEXT PRIMARY KEY, issued_at TEXT NOT NULL)",
            "CREATE TABLE devices (id TEXT PRIMARY KEY, account_id TEXT NOT NULL, name TEXT NOT NULL)",
            "CREATE TABLE heartbeats (id TEXT PRIMARY KEY, observed_at TEXT NOT NULL)",
        ] {
            graphql_orm::sqlx::query(statement)
                .execute(database.pool())
                .await
                .unwrap();
        }
        database
    }
}

/// The reported configuration: no generated mutation roots at all.
mod read_only_roots {
    pub use super::entities::*;
    use graphql_orm::prelude::*;

    schema_roots! {
        generated_mutations: "none",
        query_custom_ops: [],
        entities: [Account, Badge, Device, Heartbeat],
    }
}

fn mutation_fields(sdl: &str) -> Vec<String> {
    use graphql_orm::async_graphql::parser::types::{TypeKind, TypeSystemDefinition};
    let document = graphql_orm::async_graphql::parser::parse_schema(sdl).unwrap();
    let mut fields = document
        .definitions
        .iter()
        .filter_map(|definition| match definition {
            TypeSystemDefinition::Type(definition) if definition.node.name.node == "Mutation" => {
                match &definition.node.kind {
                    TypeKind::Object(object) => Some(object.fields.clone()),
                    _ => None,
                }
            }
            _ => None,
        })
        .flatten()
        .map(|field| field.node.name.node.to_string())
        .collect::<Vec<_>>();
    fields.sort();
    fields
}

fn operations<T: GraphqlOperationMetadata>(kind: GraphqlOperationKind) -> Vec<&'static str> {
    T::generated_graphql_operations()
        .iter()
        .filter(|operation| operation.kind() == kind)
        .map(|operation| operation.field_name())
        .collect()
}

async fn registered<S: Stream + Unpin>(stream: &mut S)
where
    S::Item: std::fmt::Debug,
{
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected initial event {other:?}"),
    })
    .await;
}

#[tokio::test]
async fn zero_input_entities_omit_only_the_mutations_that_need_the_input() {
    let sdl = entities::schema_builder(entities::database().await)
        .finish()
        .sdl();
    // No empty input object is emitted, and no root names one.
    for absent in [
        "CreateAccountInput",
        "UpdateAccountInput",
        "UpdateBadgeInput",
        "UpsertAccountResult",
        "CreateHeartbeatInput",
        "createHeartbeat",
    ] {
        assert!(!sdl.contains(absent), "{absent} must not exist:\n{sdl}");
    }
    assert!(sdl.contains("input CreateBadgeInput"), "{sdl}");
    assert!(sdl.contains("input CreateDeviceInput"), "{sdl}");
    assert!(sdl.contains("input UpdateDeviceInput"), "{sdl}");

    let fields = mutation_fields(&sdl);
    let named = |needle: &str| {
        fields
            .iter()
            .filter(|field| field.contains(needle))
            .cloned()
            .collect::<Vec<_>>()
    };
    // Input-free deletes stay; input-bearing roots follow their input.
    assert_eq!(named("Account"), ["deleteAccount", "deleteAccounts"]);
    assert_eq!(
        named("Badge"),
        ["createBadge", "deleteBadge", "deleteBadges"]
    );
    assert_eq!(
        named("Device"),
        [
            "createDevice",
            "deleteDevice",
            "deleteDevices",
            "updateDevice",
            "updateDevices"
        ]
    );

    assert_eq!(
        operations::<entities::Account>(GraphqlOperationKind::Mutation),
        ["deleteAccount", "deleteAccounts"]
    );
    assert_eq!(
        operations::<entities::Badge>(GraphqlOperationKind::Mutation),
        ["createBadge", "deleteBadge", "deleteBadges"]
    );
    assert_eq!(
        operations::<entities::Account>(GraphqlOperationKind::Subscription),
        ["accountChanged"]
    );
    // An append-only entity without a public create field has no public
    // mutation; its root keeps only the hidden, described placeholder, which
    // no caller can be authorized for.
    assert_eq!(
        named("Heartbeat"),
        ["gomHeartbeatWritesUnavailable"],
        "{fields:?}"
    );
    assert_eq!(
        operations::<entities::Heartbeat>(GraphqlOperationKind::Mutation),
        ["gomHeartbeatWritesUnavailable"]
    );
    let placeholder = entities::Heartbeat::generated_graphql_operations()
        .iter()
        .find(|operation| operation.kind() == GraphqlOperationKind::Mutation)
        .expect("the placeholder is described");
    assert_eq!(
        placeholder.category(),
        graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::Unavailable
    );
    assert_eq!(
        placeholder.authorization(),
        &graphql_orm::graphql::orm::GraphqlAuthorizationRequirement::unsatisfiable()
    );
    let response = entities::schema_builder(entities::database().await)
        .finish()
        .execute("mutation { gomHeartbeatWritesUnavailable }")
        .await;
    assert_eq!(response.errors.len(), 1, "the placeholder is refused");
    let error = serde_json::to_value(&response.errors[0]).expect("error serializes");
    assert_eq!(error["extensions"]["code"], "FORBIDDEN", "{error}");
    assert_eq!(
        operations::<entities::Heartbeat>(GraphqlOperationKind::Subscription),
        ["heartbeatChanged"]
    );
    assert!(sdl.contains("heartbeats("), "{sdl}");
}

#[tokio::test]
async fn append_only_rows_without_public_create_fields_are_appended_privately() {
    let database = entities::database().await;
    let schema = entities::schema_builder(database.clone())
        .data("test-user".to_string())
        .finish();
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { heartbeatChanged { action heartbeat { observedAt } } }")
                .data("test-user".to_string()),
        ),
    );
    registered(&mut stream).await;
    entities::Heartbeat::insert(
        &database,
        entities::CreateHeartbeatInput {
            observed_at: "t1".into(),
        },
    )
    .await
    .unwrap();
    let event = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("Changed event")
        .expect("stream item");
    assert!(event.errors.is_empty(), "{:?}", event.errors);
    assert_eq!(
        event.data.into_json().unwrap()["heartbeatChanged"]["heartbeat"]["observedAt"],
        "t1"
    );
    let response = schema
        .execute(
            Request::new("{ heartbeats { edges { node { observedAt } } } }")
                .data("test-user".to_string()),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["heartbeats"]["edges"][0]["node"]["observedAt"],
        "t1"
    );
}

#[tokio::test]
async fn generated_mutations_none_keeps_reads_relations_and_changed() {
    let database = entities::database().await;
    let schema = read_only_roots::schema_builder(database.clone())
        .data("test-user".to_string())
        .finish();
    let sdl = schema.sdl();
    assert!(mutation_fields(&sdl).is_empty(), "{sdl}");
    assert!(!sdl.contains("UpdateAccountInput"), "{sdl}");
    assert!(!sdl.contains("CreateAccountInput"), "{sdl}");
    assert!(sdl.contains("accountChanged("), "{sdl}");

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { accountChanged { action id account { refreshedAt } } }")
                .data("test-user".to_string()),
        ),
    );
    registered(&mut stream).await;

    // Repository writes keep the server-owned column in the Rust inputs.
    let account = read_only_roots::Account::insert(
        &database,
        read_only_roots::CreateAccountInput {
            refreshed_at: "t1".into(),
        },
    )
    .await
    .unwrap();
    let created = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("Changed event")
        .expect("stream item");
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let created = created.data.into_json().unwrap();
    assert_eq!(created["accountChanged"]["id"], account.id);
    assert_eq!(created["accountChanged"]["account"]["refreshedAt"], "t1");

    let updated = read_only_roots::Account::update_by_id(
        &database,
        &account.id,
        read_only_roots::UpdateAccountInput {
            refreshed_at: Some("t2".into()),
        },
    )
    .await
    .unwrap()
    .expect("updated account");
    assert_eq!(updated.refreshed_at, "t2");
    let changed = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("Changed event")
        .expect("stream item");
    assert!(changed.errors.is_empty(), "{:?}", changed.errors);
    assert_eq!(
        changed.data.into_json().unwrap()["accountChanged"]["account"]["refreshedAt"],
        "t2"
    );

    // Transaction writes use the same repository inputs.
    let account_id = account.id.clone();
    database
        .transaction(TransactionMode::Default, |tx| {
            let account_id = account_id.clone();
            Box::pin(async move {
                tx.update_by_id::<read_only_roots::Account>(
                    &account_id,
                    read_only_roots::UpdateAccountInput {
                        refreshed_at: Some("t3".into()),
                    },
                )
                .await?;
                tx.insert::<read_only_roots::Badge>(read_only_roots::CreateBadgeInput {
                    code: "gold".into(),
                    issued_at: "t3".into(),
                })
                .await?;
                tx.insert::<read_only_roots::Device>(read_only_roots::CreateDeviceInput {
                    account_id: account_id.clone(),
                    name: "laptop".into(),
                })
                .await?;
                Ok(())
            })
        })
        .await
        .unwrap();
    let badge = read_only_roots::Badge::update_by_id(
        &database,
        &"gold".to_string(),
        read_only_roots::UpdateBadgeInput {
            issued_at: Some("t4".into()),
        },
    )
    .await
    .unwrap()
    .expect("updated badge");
    assert_eq!(badge.issued_at, "t4");

    // Reads and relations in both directions.
    let response = schema
        .execute(
            Request::new(
                "{ accounts { edges { node { id refreshedAt devices { edges { node { name account { refreshedAt } } } } } } } \
                 badges { edges { node { code issuedAt } } } }",
            )
            .data("test-user".to_string()),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let node = &data["accounts"]["edges"][0]["node"];
    assert_eq!(node["id"], account.id);
    assert_eq!(node["refreshedAt"], "t3");
    let device = &node["devices"]["edges"][0]["node"];
    assert_eq!(device["name"], "laptop");
    assert_eq!(device["account"]["refreshedAt"], "t3");
    assert_eq!(data["badges"]["edges"][0]["node"]["issuedAt"], "t4");

    // The schema itself refuses the absent roots.
    let refused = schema
        .execute(Request::new(
            "mutation { deleteAccount(id: \"x\") { success } }",
        ))
        .await;
    assert!(!refused.errors.is_empty());
}
