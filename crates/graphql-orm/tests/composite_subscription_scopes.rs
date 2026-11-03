//! Scope declarations must follow the generated subscription surface, not mutations.
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
use graphql_orm::async_graphql::{Request, Schema};
use graphql_orm::futures::StreamExt;
#[cfg(feature = "sqlite")]
use graphql_orm::futures::future::poll_fn;
use graphql_orm::prelude::*;
#[cfg(feature = "sqlite")]
use std::task::Poll;
#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{field_name, resolver_name};
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "scope_scopedcomposite",
        plural = "ScopedComposites"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "scope_scopedcomposite",
        plural = "ScopedComposites"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "scope_scopedcomposite",
        plural = "ScopedComposites",
        schema_policy = "external_writable"
    )
)]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["events.read"]))]
struct ScopedComposite {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[primary_key]
    code: String,
    label: String,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "scope_scopedprojection",
        plural = "ScopedProjections",
        schema_policy = "external_read_only"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "scope_scopedprojection",
        plural = "ScopedProjections",
        schema_policy = "external_read_only"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "scope_scopedprojection",
        plural = "ScopedProjections",
        schema_policy = "external_read_only"
    )
)]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["events.read"]))]
struct ScopedProjection {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[primary_key]
    code: String,
    label: String,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "scope_descriptionrow",
        plural = "DescriptionRows"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "scope_descriptionrow",
        plural = "DescriptionRows"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "scope_descriptionrow",
        plural = "DescriptionRows",
        schema_policy = "external_writable"
    )
)]
struct DescriptionRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
}
schema_roots! { entities: [ScopedComposite, ScopedProjection, DescriptionRow] }
fn operations() -> Vec<&'static GeneratedGraphqlOperationDescriptor> {
    [
        ScopedComposite::generated_graphql_operations(),
        ScopedProjection::generated_graphql_operations(),
        DescriptionRow::generated_graphql_operations(),
    ]
    .into_iter()
    .flatten()
    .collect()
}
fn schema() -> AppSchema {
    Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .enable_subscription_in_federation()
    .finish()
}
#[tokio::test]
async fn composite_and_readonly_roots_deny_a_subject_without_the_scope() {
    let schema = schema();
    for field in [
        resolver_name("scopedCompositeChanged"),
        resolver_name("scopedProjectionChanged"),
    ] {
        let mut events = Box::pin(
            schema.execute_stream(
                Request::new(format!(
                    "subscription {{ {field} {{ {} }} }}",
                    field_name("id")
                ))
                .data(AuthSubject::new("reader")),
            ),
        );
        let response = events.next().await.expect("scope refusal");
        assert_eq!(response.errors.len(), 1, "{response:?}");
        assert_eq!(response.errors[0].message, "forbidden");
    }
}
#[test]
fn scope_is_exported_and_generated_descriptions_have_no_interpolation_markers() {
    let schema = schema();
    let sdl =
        schema.sdl_with_options(graphql_orm::async_graphql::SDLExportOptions::new().federation());
    fn assert_no_marker(text: &str) {
        assert!(
            !text
                .as_bytes()
                .windows(2)
                .any(|pair| pair[0] == b'#' && (pair[1].is_ascii_alphabetic() || pair[1] == b'_')),
            "unexpanded interpolation in {text}"
        );
    }
    assert_no_marker(&sdl);
    let catalog = graphql_orm_operation_catalog();
    for operation in operations() {
        assert_no_marker(operation.description());
        for argument in operation.arguments() {
            assert_no_marker(argument.description());
        }
        if operation.kind() != GraphqlOperationKind::Subscription
            || operation.entity_name() == "DescriptionRow"
        {
            continue;
        }
        assert!(
            matches!(operation.authorization(), GraphqlAuthorizationRequirement::AllScopes { scopes } if scopes == &["events.read"])
        );
        let resolved = catalog
            .operations()
            .iter()
            .find(|op| op.field_name() == operation.field_name())
            .unwrap();
        assert_eq!(resolved.authorization(), operation.authorization());
        assert!(
            sdl.contains("@federation__requiresScopes(scopes: [[\"events.read\"]])"),
            "{sdl}"
        );
        #[cfg(feature = "router-protocol")]
        {
            let exported = catalog.router_protocol_operations().unwrap();
            let descriptor = exported
                .iter()
                .find(|op| op.field_name == operation.field_name())
                .unwrap();
            assert!(
                matches!(&descriptor.authorization, graphql_orm_router_protocol::AuthorizationRequirement::AllScopes { scopes } if scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>() == ["events.read"])
            );
        }
    }
}
#[cfg(feature = "sqlite")]
struct Allow;
#[cfg(feature = "sqlite")]
impl EntityPolicy for Allow {
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
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn the_declared_scope_admits_both_composite_roots() {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    db.set_entity_policy(Allow);
    let schema = schema_builder(db).finish();
    let mut subject = AuthSubject::new("reader");
    subject.scopes.push("events.read".into());
    for field in [
        resolver_name("scopedCompositeChanged"),
        resolver_name("scopedProjectionChanged"),
    ] {
        let mut events = Box::pin(
            schema.execute_stream(
                Request::new(format!(
                    "subscription {{ {field} {{ {} }} }}",
                    field_name("id")
                ))
                .data(subject.clone()),
            ),
        );
        poll_fn(|cx| match events.poll_next_unpin(cx) {
            Poll::Pending => Poll::Ready(()),
            item => panic!("scope-authorized stream was not admitted: {item:?}"),
        })
        .await;
    }
}
