//! #677: the seven generated wrapper objects no longer derive `SimpleObject`.
//! Each hand-emitted type must register and resolve exactly as a
//! `SimpleObject` twin with the same GraphQL name, fields and doc comments.
//!
//! The twins spell out the default (camel case) field names, so the check runs
//! in the default naming lane only.
#![cfg(feature = "sqlite")]
#![cfg(not(any(feature = "postgres", feature = "mssql")))]
#![cfg(not(any(
    graphql_orm_field_case = "pascal",
    graphql_orm_field_case = "snake",
    graphql_orm_field_case = "screaming_snake",
    graphql_orm_field_case = "lower",
    graphql_orm_field_case = "upper",
)))]

use async_graphql::{EmptyMutation, EmptySubscription, Object, SDLExportOptions, Schema};
use graphql_orm::graphql::orm::{
    ChangeAction, ChangeKind, NotificationError, ScanPageInfo, ScanStatus,
};
use graphql_orm::graphql::pagination::PageInfo;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "parity_items", plural = "ParityItems", keyset = "id asc")]
struct ParityItem {
    #[primary_key]
    #[sortable]
    id: String,
    name: String,
}

mod twin {
    use super::*;
    use async_graphql::SimpleObject;

    /// Edge containing a node and cursor
    #[derive(SimpleObject)]
    #[graphql(name = "ParityItemEdge", rename_fields = "camelCase")]
    pub struct Edge {
        /// The item at the end of the edge
        pub node: ParityItem,
        /// A cursor for pagination
        pub cursor: String,
    }

    /// Connection containing edges and page info
    #[derive(SimpleObject)]
    #[graphql(name = "ParityItemConnection", rename_fields = "camelCase")]
    pub struct Connection {
        /// The edges in this connection
        pub edges: Vec<Edge>,
        /// Pagination information
        #[graphql(name = "pageInfo")]
        pub page_info: PageInfo,
    }

    /// Mutation result for this entity.
    #[derive(SimpleObject)]
    #[graphql(name = "ParityItemResult", rename_fields = "camelCase")]
    pub struct MutationResult {
        pub success: bool,
        pub error: Option<String>,
        #[graphql(name = "parityItem")]
        pub entity: Option<ParityItem>,
    }

    /// Subscription change event for this entity.
    #[derive(SimpleObject)]
    #[graphql(name = "ParityItemChangedEvent", rename_fields = "camelCase")]
    pub struct ChangedEvent {
        pub action: ChangeAction,
        #[graphql(name = "changeKind")]
        pub change_kind: ChangeKind,
        pub id: String,
        #[graphql(name = "sourceEntity")]
        pub source_entity: Option<String>,
        #[graphql(name = "sourceId")]
        pub source_id: Option<String>,
        pub path: Vec<String>,
        #[graphql(name = "parityItem")]
        pub entity: Option<ParityItem>,
    }

    /// Result of bulk delete by Where filter
    #[derive(SimpleObject)]
    #[graphql(name = "DeleteParityItemsResult", rename_fields = "camelCase")]
    pub struct DeleteMany {
        pub success: bool,
        /// Failures after commit; success and count still describe the committed write.
        pub notification_errors: Option<Vec<NotificationError>>,
        pub error: Option<String>,
        /// Application refusal code, or the safe database error category.
        pub error_code: Option<String>,
        #[graphql(name = "deletedCount")]
        pub deleted_count: i64,
    }

    /// Result of bulk update by Where filter
    #[derive(SimpleObject)]
    #[graphql(name = "UpdateParityItemsResult", rename_fields = "camelCase")]
    pub struct UpdateMany {
        pub success: bool,
        /// Failures after commit; success and count still describe the committed write.
        pub notification_errors: Option<Vec<NotificationError>>,
        pub error: Option<String>,
        /// Application refusal code, or the safe database error category.
        pub error_code: Option<String>,
        #[graphql(name = "affectedCount")]
        pub affected_count: i64,
    }

    #[derive(SimpleObject)]
    #[graphql(rename_fields = "camelCase")]
    pub struct ParityItemAuthorizedScanPage {
        pub nodes: Vec<ParityItem>,
        pub page_info: ScanPageInfo,
    }
}

fn item(id: &str) -> ParityItem {
    ParityItem {
        id: id.to_owned(),
        name: format!("name-{id}"),
    }
}

fn page_info() -> PageInfo {
    PageInfo {
        has_next_page: true,
        has_previous_page: false,
        start_cursor: Some("start".to_owned()),
        end_cursor: None,
        total_count: Some(2),
    }
}

fn notification_errors() -> Option<Vec<NotificationError>> {
    Some(vec![NotificationError {
        message: "after commit".to_owned(),
        code: Some("HOOK".to_owned()),
    }])
}

fn scan_page_info() -> ScanPageInfo {
    ScanPageInfo {
        status: ScanStatus::PageFull,
        continuation: Some("next".to_owned()),
        total_count: None,
    }
}

struct GeneratedQuery;

#[Object(name = "Query")]
impl GeneratedQuery {
    async fn edge(&self) -> ParityItemEdge {
        ParityItemEdge {
            node: item("a"),
            cursor: "c-a".to_owned(),
        }
    }
    async fn connection(&self) -> ParityItemConnection {
        ParityItemConnection {
            edges: vec![
                ParityItemEdge {
                    node: item("a"),
                    cursor: "c-a".to_owned(),
                },
                ParityItemEdge {
                    node: item("b"),
                    cursor: "c-b".to_owned(),
                },
            ],
            page_info: page_info(),
        }
    }
    async fn result(&self) -> ParityItemResult {
        ParityItemResult {
            success: false,
            error: Some("refused".to_owned()),
            entity: Some(item("r")),
        }
    }
    async fn changed(&self) -> ParityItemChangedEvent {
        ParityItemChangedEvent {
            action: ChangeAction::Updated,
            change_kind: ChangeKind::Propagated,
            id: "e".to_owned(),
            source_entity: Some("Source".to_owned()),
            source_id: None,
            path: vec!["one".to_owned(), "two".to_owned()],
            entity: None,
        }
    }
    async fn deleted(&self) -> DeleteParityItemsResult {
        DeleteParityItemsResult {
            success: true,
            notification_errors: notification_errors(),
            error: None,
            error_code: Some("CODE".to_owned()),
            deleted_count: 3,
        }
    }
    async fn updated(&self) -> UpdateParityItemsResult {
        UpdateParityItemsResult {
            success: true,
            notification_errors: None,
            error: None,
            error_code: None,
            affected_count: 4,
        }
    }
    async fn scan(&self) -> ParityItemAuthorizedScanPage {
        ParityItemAuthorizedScanPage {
            nodes: vec![item("s")],
            page_info: scan_page_info(),
        }
    }
}

struct TwinQuery;

#[Object(name = "Query")]
impl TwinQuery {
    async fn edge(&self) -> twin::Edge {
        twin::Edge {
            node: item("a"),
            cursor: "c-a".to_owned(),
        }
    }
    async fn connection(&self) -> twin::Connection {
        twin::Connection {
            edges: vec![
                twin::Edge {
                    node: item("a"),
                    cursor: "c-a".to_owned(),
                },
                twin::Edge {
                    node: item("b"),
                    cursor: "c-b".to_owned(),
                },
            ],
            page_info: page_info(),
        }
    }
    async fn result(&self) -> twin::MutationResult {
        twin::MutationResult {
            success: false,
            error: Some("refused".to_owned()),
            entity: Some(item("r")),
        }
    }
    async fn changed(&self) -> twin::ChangedEvent {
        twin::ChangedEvent {
            action: ChangeAction::Updated,
            change_kind: ChangeKind::Propagated,
            id: "e".to_owned(),
            source_entity: Some("Source".to_owned()),
            source_id: None,
            path: vec!["one".to_owned(), "two".to_owned()],
            entity: None,
        }
    }
    async fn deleted(&self) -> twin::DeleteMany {
        twin::DeleteMany {
            success: true,
            notification_errors: notification_errors(),
            error: None,
            error_code: Some("CODE".to_owned()),
            deleted_count: 3,
        }
    }
    async fn updated(&self) -> twin::UpdateMany {
        twin::UpdateMany {
            success: true,
            notification_errors: None,
            error: None,
            error_code: None,
            affected_count: 4,
        }
    }
    async fn scan(&self) -> twin::ParityItemAuthorizedScanPage {
        twin::ParityItemAuthorizedScanPage {
            nodes: vec![item("s")],
            page_info: scan_page_info(),
        }
    }
}

const QUERY: &str = r#"
{
  edge { __typename node { id name } cursor again: cursor }
  connection {
    __typename
    edges { __typename cursor node { id } }
    pageInfo { hasNextPage hasPreviousPage startCursor endCursor totalCount }
  }
  result { __typename success error parityItem { id name } }
  changed {
    __typename action changeKind id sourceEntity sourceId path parityItem { id }
  }
  deleted {
    __typename success notificationErrors { message code } error errorCode deletedCount
  }
  updated { __typename success notificationErrors { message } error errorCode affectedCount }
  scan { __typename nodes { id name } pageInfo { status continuation totalCount } }
  ...Fragmented
}
fragment Fragmented on Query {
  first: edge { ... on ParityItemEdge { cursor } }
}
"#;

#[test]
fn wrapper_objects_register_like_simple_object() {
    let generated = Schema::new(GeneratedQuery, EmptyMutation, EmptySubscription);
    let twin = Schema::new(TwinQuery, EmptyMutation, EmptySubscription);
    assert_eq!(generated.sdl(), twin.sdl());
    let federation = SDLExportOptions::new().federation();
    assert_eq!(
        generated.sdl_with_options(federation),
        twin.sdl_with_options(federation)
    );
    // Descriptions are part of the comparison: the twin docs must reach the SDL.
    assert!(
        generated
            .sdl()
            .contains("Edge containing a node and cursor")
    );
    assert!(
        generated
            .sdl()
            .contains("Application refusal code, or the safe database error category.")
    );
}

type GeneratedSchema = Schema<GeneratedQuery, EmptyMutation, EmptySubscription>;
type TwinSchema = Schema<TwinQuery, EmptyMutation, EmptySubscription>;

/// Entity fields read through the ORM, so both schemas carry a database.
async fn schemas() -> (GeneratedSchema, TwinSchema) {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    (
        Schema::build(GeneratedQuery, EmptyMutation, EmptySubscription)
            .data(database.clone())
            .finish(),
        Schema::build(TwinQuery, EmptyMutation, EmptySubscription)
            .data(database)
            .finish(),
    )
}

#[tokio::test]
async fn wrapper_objects_resolve_like_simple_object() {
    let (generated, twin) = schemas().await;
    let generated_response = generated.execute(QUERY).await;
    let twin_response = twin.execute(QUERY).await;
    assert!(
        generated_response.errors.is_empty(),
        "{:?}",
        generated_response.errors
    );
    assert_eq!(
        serde_json::to_value(&generated_response).unwrap(),
        serde_json::to_value(&twin_response).unwrap()
    );
    let data = generated_response.data.into_json().unwrap();
    assert_eq!(data["edge"]["__typename"], "ParityItemEdge");
    assert_eq!(data["connection"]["edges"][1]["node"]["id"], "b");
    assert_eq!(data["deleted"]["errorCode"], "CODE");
    assert_eq!(data["scan"]["pageInfo"]["status"], "PAGE_FULL");

    // Introspection sees the same types too.
    let introspection = r#"{ __schema { types { name description fields { name description
        type { name kind ofType { name kind ofType { name kind ofType { name kind } } } } } } } }"#;
    let generated_response = generated.execute(introspection).await;
    let twin_response = twin.execute(introspection).await;
    assert!(
        generated_response.errors.is_empty(),
        "{:?}",
        generated_response.errors
    );
    assert_eq!(
        serde_json::to_value(&generated_response).unwrap(),
        serde_json::to_value(&twin_response).unwrap()
    );
}

#[tokio::test]
async fn derive_getters_remain_public() {
    // The SimpleObject derive's `async fn field(&self, ctx)` getters are part
    // of the generated Rust surface; exercise them from a resolver context.
    struct Probe;
    #[Object]
    impl Probe {
        async fn probe(&self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<String> {
            let edge = ParityItemEdge {
                node: item("p"),
                cursor: "c-p".to_owned(),
            };
            let deleted = DeleteParityItemsResult::ok(7);
            Ok(format!(
                "{}:{}:{}",
                edge.cursor(ctx).await?,
                edge.node(ctx).await?.id,
                deleted.deleted_count(ctx).await?
            ))
        }
    }
    let response = Schema::new(Probe, EmptyMutation, EmptySubscription)
        .execute("{ probe }")
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        serde_json::json!({ "probe": "c-p:p:7" })
    );
}
