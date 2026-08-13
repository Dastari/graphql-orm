#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::prelude::*;

fn operation(
    operations: &[GeneratedGraphqlOperationDescriptor],
    category: GeneratedGraphqlOperationCategory,
) -> &GeneratedGraphqlOperationDescriptor {
    operations
        .iter()
        .find(|operation| operation.category() == category)
        .expect("generated operation category")
}

fn resolver_name(camel: &'static str, pascal: &'static str) -> &'static str {
    if cfg!(feature = "resolver-case-pascal") {
        pascal
    } else {
        camel
    }
}

fn argument_name(camel: &'static str, pascal: &'static str) -> &'static str {
    if cfg!(feature = "argument-case-pascal") {
        pascal
    } else {
        camel
    }
}

fn root_field_has(sdl: &str, name: &str, expected: &str) -> bool {
    sdl.match_indices(&format!("\n\t{name}")).any(|(start, _)| {
        let tail = &sdl[start..];
        let next_description = tail
            .get(2..)
            .and_then(|tail| tail.find("\n\t\"\"\""))
            .map(|offset| offset + 2);
        let root_end = tail.find("\n}");
        tail[..next_description.or(root_end).unwrap_or(tail.len())].contains(expected)
    })
}

mod rich_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "resolver_metadata_records",
        plural = "ResolverMetadataRecords",
        keyset = "created_at desc, id asc",
        upsert = "slug"
    )]
    #[graphql_orm(projection(
        name = "ResolverMetadataProjection",
        fields = [id, slug],
        private = true
    ))]
    pub struct ResolverMetadataRecord {
        #[primary_key]
        #[sortable]
        pub id: String,

        #[unique]
        #[filterable(type = "string")]
        #[graphql_orm(searchable(weight = "A"))]
        pub slug: String,

        #[filterable(type = "string")]
        #[sortable]
        pub title: String,

        #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
        pub private_payload: serde_json::Value,

        #[sortable]
        pub created_at: i64,
    }

    schema_roots! {
        entities: [ResolverMetadataRecord],
    }
}

mod exposure_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(table = "resolver_metadata_visible", plural = "VisibleMetadataRecords")]
    pub struct VisibleMetadataRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(table = "resolver_metadata_hidden", plural = "HiddenMetadataRecords")]
    pub struct HiddenMetadataRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    schema_roots! {
        generated_mutations: "allowlist",
        generated_mutation_allowlist: [VisibleMetadataRecord],
        entities: [VisibleMetadataRecord, HiddenMetadataRecord],
    }
}

mod read_only_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "resolver_metadata_read_only",
        plural = "ReadOnlyMetadataRecords",
        schema_policy = "external_read_only"
    )]
    pub struct ReadOnlyMetadataRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    schema_roots! {
        schema_policy: "external_read_only",
        entities: [ReadOnlyMetadataRecord],
    }
}

mod root_read_only_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "resolver_metadata_root_read_only",
        plural = "RootReadOnlyMetadataRecords"
    )]
    pub struct RootReadOnlyMetadataRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    schema_roots! {
        schema_policy: "external_read_only",
        entities: [RootReadOnlyMetadataRecord],
    }
}

mod composite_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "resolver_metadata_composite",
        plural = "CompositeMetadataRecords"
    )]
    pub struct CompositeMetadataRecord {
        #[primary_key]
        #[graphql(name = "TenantKey")]
        pub tenant_key: i32,

        #[primary_key]
        #[graphql(name = "RecordKey")]
        pub record_key: i32,

        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    schema_roots! {
        entities: [CompositeMetadataRecord],
    }
}

mod append_only_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "resolver_metadata_events",
        plural = "ResolverMetadataEvents",
        append_only = true
    )]
    #[graphql_orm(
        operation_authorization(
            categories = ["create"],
            all_scopes = ["events.append"]
        ),
        operation_authorization(
            categories = ["subscription"],
            all_scopes = ["events.read"]
        )
    )]
    pub struct ResolverMetadataEvent {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub message: String,
    }

    schema_roots! {
        entities: [ResolverMetadataEvent],
    }
}

mod scoped_write_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "resolver_metadata_scoped_writes",
        plural = "ScopedWriteRecords",
        auth = "none",
        upsert = "slug"
    )]
    #[graphql_orm(
        operation_authorization(
            categories = [
                "create",
                "upsert",
                "update",
                "update_many",
                "delete",
                "delete_many"
            ],
            all_scopes = ["records.write"]
        ),
        operation_authorization(
            categories = ["subscription"],
            any_scopes = [["records.events"], ["records.admin"]]
        )
    )]
    pub struct ScopedWriteRecord {
        #[primary_key]
        #[filterable(type = "string")]
        pub id: String,
        #[unique]
        #[filterable(type = "string")]
        pub slug: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    schema_roots! {
        entities: [ScopedWriteRecord],
    }
}

mod scoped_read_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "resolver_metadata_scoped_records",
        plural = "ScopedMetadataRecords",
        auth = "none",
        keyset = "id asc"
    )]
    #[graphql_orm(
        operation_authorization(
            categories = ["single_read"],
            any_scopes = [["records.read"], ["records.admin", "records.audit"]]
        ),
        operation_authorization(
            categories = ["list"],
            all_scopes = ["records.list", "tenant.active"]
        ),
        operation_authorization(
            categories = ["search"],
            any_scopes = [["records.search"], ["records.admin"]]
        ),
        operation_authorization(
            categories = ["keyset_list"],
            all_scopes = ["records.page", "tenant.active"]
        )
    )]
    pub struct ScopedMetadataRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        #[graphql_orm(searchable(weight = "A"))]
        pub label: String,
    }

    schema_roots! {
        entities: [ScopedMetadataRecord],
    }
}

mod templated_read_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "resolver_metadata_templated_records",
        plural = "TemplatedMetadataRecords",
        auth = "none"
    )]
    #[graphql_orm(operation_authorization(
        categories = ["single_read"],
        any_scope_templates = [["records.{id}.read"], ["records.admin"]]
    ))]
    pub struct TemplatedMetadataRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    schema_roots! {
        entities: [TemplatedMetadataRecord],
    }
}

mod authenticated_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "resolver_metadata_authenticated_records",
        plural = "AuthenticatedMetadataRecords",
        auth = "required"
    )]
    pub struct AuthenticatedMetadataRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    schema_roots! {
        entities: [AuthenticatedMetadataRecord],
    }
}

#[test]
fn derive_metadata_covers_every_rich_generated_category_and_exact_names() {
    use rich_surface::ResolverMetadataRecord;

    let operations = ResolverMetadataRecord::generated_graphql_operations();
    assert_eq!(operations.len(), 11);
    assert!(operations.iter().all(|operation| {
        operation.fingerprint().len() == 64
            && operation
                .entity_rust_type()
                .ends_with("::ResolverMetadataRecord")
            && operation.entity_name() == "ResolverMetadataRecord"
            && operation.table_name() == "resolver_metadata_records"
            && operation.backend()
                == if cfg!(feature = "postgres") {
                    "postgres"
                } else {
                    "sqlite"
                }
    }));

    let list = operation(operations, GeneratedGraphqlOperationCategory::List);
    assert_eq!(
        list.field_name(),
        resolver_name("resolverMetadataRecords", "ResolverMetadataRecords")
    );
    assert_eq!(list.root_type(), "Query");
    assert_eq!(
        list.graphql_result_type(),
        "ResolverMetadataRecordConnection!"
    );
    assert_eq!(
        list.arguments()
            .iter()
            .map(GraphqlOperationArgumentDescriptor::graphql_name)
            .collect::<Vec<_>>(),
        vec![
            argument_name("where", "Where"),
            argument_name("orderBy", "OrderBy"),
            argument_name("page", "Page"),
        ]
    );
    assert_eq!(
        list.arguments()
            .iter()
            .map(GraphqlOperationArgumentDescriptor::graphql_type)
            .collect::<Vec<_>>(),
        vec![
            "ResolverMetadataRecordWhereInput",
            "[ResolverMetadataRecordOrderByInput!]",
            "PageInput",
        ]
    );

    assert_eq!(
        operation(operations, GeneratedGraphqlOperationCategory::Search).field_name(),
        resolver_name(
            "resolverMetadataRecordsSearch",
            "ResolverMetadataRecordsSearch"
        )
    );
    assert_eq!(
        operation(operations, GeneratedGraphqlOperationCategory::KeysetList).field_name(),
        resolver_name(
            "resolverMetadataRecordsKeyset",
            "ResolverMetadataRecordsKeyset"
        )
    );
    assert_eq!(
        operation(operations, GeneratedGraphqlOperationCategory::SingleRead).field_name(),
        resolver_name("resolverMetadataRecord", "ResolverMetadataRecord")
    );
    assert_eq!(
        operation(operations, GeneratedGraphqlOperationCategory::Create).field_name(),
        resolver_name(
            "createResolverMetadataRecord",
            "CreateResolverMetadataRecord"
        )
    );
    assert_eq!(
        operation(operations, GeneratedGraphqlOperationCategory::Upsert).field_name(),
        resolver_name(
            "upsertResolverMetadataRecord",
            "UpsertResolverMetadataRecord"
        )
    );
    let update = operation(operations, GeneratedGraphqlOperationCategory::Update);
    assert_eq!(
        update.field_name(),
        resolver_name(
            "updateResolverMetadataRecord",
            "UpdateResolverMetadataRecord"
        )
    );
    assert_eq!(
        update
            .arguments()
            .iter()
            .map(GraphqlOperationArgumentDescriptor::graphql_type)
            .collect::<Vec<_>>(),
        vec!["String!", "UpdateResolverMetadataRecordInput!"]
    );
    assert_eq!(
        operation(operations, GeneratedGraphqlOperationCategory::UpdateMany).field_name(),
        resolver_name(
            "updateResolverMetadataRecords",
            "UpdateResolverMetadataRecords"
        )
    );
    assert_eq!(
        operation(operations, GeneratedGraphqlOperationCategory::Delete).field_name(),
        resolver_name(
            "deleteResolverMetadataRecord",
            "DeleteResolverMetadataRecord"
        )
    );
    assert_eq!(
        operation(operations, GeneratedGraphqlOperationCategory::DeleteMany).field_name(),
        resolver_name(
            "deleteResolverMetadataRecords",
            "DeleteResolverMetadataRecords"
        )
    );
    assert_eq!(
        operation(operations, GeneratedGraphqlOperationCategory::Subscription).field_name(),
        resolver_name(
            "resolverMetadataRecordChanged",
            "ResolverMetadataRecordChanged"
        )
    );
    let private_payload_name = if cfg!(feature = "field-case-pascal") {
        "graphql=\"PrivatePayload\""
    } else {
        "graphql=\"privatePayload\""
    };
    assert!(list.schema_signature().contains(private_payload_name));
    assert!(list.schema_signature().contains("output=false"));
    assert!(list.schema_signature().contains("json=true"));
    assert!(
        !list
            .schema_signature()
            .contains("ResolverMetadataProjection")
    );

    let catalog = rich_surface::graphql_orm_operation_catalog();
    assert_eq!(catalog.operations().len(), operations.len());
    assert_eq!(catalog.exposed_operations().count(), operations.len());
    assert_eq!(catalog.fingerprint().len(), 64);
    assert!(
        catalog
            .resolve(GraphqlOperationKind::Query, list.field_name())
            .is_some()
    );
}

#[test]
fn schema_catalog_resolves_mutation_allowlist_without_hiding_queries_or_subscriptions() {
    let catalog = exposure_surface::graphql_orm_operation_catalog();
    let hidden_create = resolver_name("createHiddenMetadataRecord", "CreateHiddenMetadataRecord");
    let visible_create =
        resolver_name("createVisibleMetadataRecord", "CreateVisibleMetadataRecord");

    let hidden = catalog
        .operations()
        .iter()
        .find(|operation| operation.field_name() == hidden_create)
        .expect("hidden generated mutation remains discoverable");
    assert!(!hidden.is_exposed());
    assert!(
        catalog
            .resolve(GraphqlOperationKind::Mutation, hidden_create)
            .is_none()
    );
    assert!(
        catalog
            .resolve(GraphqlOperationKind::Mutation, visible_create)
            .is_some()
    );

    let hidden_query = resolver_name("hiddenMetadataRecords", "HiddenMetadataRecords");
    let hidden_subscription =
        resolver_name("hiddenMetadataRecordChanged", "HiddenMetadataRecordChanged");
    assert!(
        catalog
            .resolve(GraphqlOperationKind::Query, hidden_query)
            .is_some()
    );
    assert!(
        catalog
            .resolve(GraphqlOperationKind::Subscription, hidden_subscription)
            .is_some()
    );
}

#[test]
fn read_only_composite_and_append_only_profiles_report_only_actual_resolvers() {
    use append_only_surface::ResolverMetadataEvent;
    use composite_surface::CompositeMetadataRecord;
    use read_only_surface::ReadOnlyMetadataRecord;
    use root_read_only_surface::RootReadOnlyMetadataRecord;

    let read_only = ReadOnlyMetadataRecord::generated_graphql_operations();
    assert_eq!(read_only.len(), 2);
    assert!(
        read_only
            .iter()
            .all(|operation| operation.kind() == GraphqlOperationKind::Query)
    );
    assert_eq!(
        read_only_surface::graphql_orm_operation_catalog()
            .exposed_operations()
            .count(),
        2
    );

    let root_read_only = RootReadOnlyMetadataRecord::generated_graphql_operations();
    assert!(
        root_read_only
            .iter()
            .any(|operation| { operation.kind() == GraphqlOperationKind::Subscription })
    );
    let root_read_only_catalog = root_read_only_surface::graphql_orm_operation_catalog();
    assert_eq!(root_read_only_catalog.exposed_operations().count(), 2);
    assert!(root_read_only_catalog.operations().iter().all(|operation| {
        operation.kind() == GraphqlOperationKind::Query || !operation.is_exposed()
    }));

    let composite = CompositeMetadataRecord::generated_graphql_operations();
    assert_eq!(composite.len(), 2);
    let single = operation(composite, GeneratedGraphqlOperationCategory::SingleRead);
    assert_eq!(
        single
            .arguments()
            .iter()
            .map(GraphqlOperationArgumentDescriptor::graphql_name)
            .collect::<Vec<_>>(),
        vec![
            argument_name("tenantKey", "TenantKey"),
            argument_name("recordKey", "RecordKey"),
        ]
    );

    let append_only = ResolverMetadataEvent::generated_graphql_operations();
    assert_eq!(append_only.len(), 4);
    assert!(
        append_only
            .iter()
            .any(|operation| operation.category() == GeneratedGraphqlOperationCategory::Create)
    );
    assert!(!append_only.iter().any(|operation| matches!(
        operation.category(),
        GeneratedGraphqlOperationCategory::Update
            | GeneratedGraphqlOperationCategory::UpdateMany
            | GeneratedGraphqlOperationCategory::Delete
            | GeneratedGraphqlOperationCategory::DeleteMany
            | GeneratedGraphqlOperationCategory::Upsert
    )));
}

#[tokio::test]
async fn append_only_subscription_enforces_auth_before_database_runtime_lookup() {
    use append_only_surface::{MutationRoot, QueryRoot, ResolverMetadataEvent, SubscriptionRoot};
    use graphql_orm::futures::StreamExt as _;

    let field_name = operation(
        ResolverMetadataEvent::generated_graphql_operations(),
        GeneratedGraphqlOperationCategory::Subscription,
    )
    .field_name();
    let schema = graphql_orm::async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .data(AuthSubject::new("event-reader-without-scope"))
    .finish();
    let mut stream =
        Box::pin(schema.execute_stream(format!("subscription {{ {field_name} {{ action }} }}")));
    let response = stream.next().await.expect("subscription response");

    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(response.errors[0].message, "forbidden");
}

#[test]
fn fixed_read_scope_declarations_drive_metadata_and_sdl_without_schema_fingerprint_drift() {
    use scoped_read_surface::{QueryRoot, ScopedMetadataRecord};

    let single = operation(
        ScopedMetadataRecord::generated_graphql_operations(),
        GeneratedGraphqlOperationCategory::SingleRead,
    );
    assert!(matches!(
        single.authorization(),
        GraphqlAuthorizationRequirement::AnyScopes { alternatives }
            if alternatives
                .iter()
                .map(|alternative| {
                    alternative.scopes.iter().map(String::as_str).collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                == vec![vec!["records.read"], vec!["records.admin", "records.audit"]]
    ));
    let list = operation(
        ScopedMetadataRecord::generated_graphql_operations(),
        GeneratedGraphqlOperationCategory::List,
    );
    assert!(matches!(
        list.authorization(),
        GraphqlAuthorizationRequirement::AllScopes { scopes }
            if scopes == &["records.list".to_string(), "tenant.active".to_string()]
    ));
    let search = operation(
        ScopedMetadataRecord::generated_graphql_operations(),
        GeneratedGraphqlOperationCategory::Search,
    );
    assert!(matches!(
        search.authorization(),
        GraphqlAuthorizationRequirement::AnyScopes { alternatives }
            if alternatives
                .iter()
                .map(|alternative| {
                    alternative.scopes.iter().map(String::as_str).collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                == vec![vec!["records.search"], vec!["records.admin"]]
    ));
    let keyset = operation(
        ScopedMetadataRecord::generated_graphql_operations(),
        GeneratedGraphqlOperationCategory::KeysetList,
    );
    assert!(matches!(
        keyset.authorization(),
        GraphqlAuthorizationRequirement::AllScopes { scopes }
            if scopes == &["records.page".to_string(), "tenant.active".to_string()]
    ));
    assert!(
        !single
            .schema_signature()
            .contains("operation_authorization"),
        "authorization policy must not change the legacy schema signature"
    );

    let schema = graphql_orm::async_graphql::Schema::build(
        QueryRoot::default(),
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .finish();
    let sdl =
        schema.sdl_with_options(graphql_orm::async_graphql::SDLExportOptions::new().federation());
    assert!(
        sdl.contains(
            r#"@requiresScopes(scopes: [["records.read"], ["records.admin", "records.audit"]])"#
        ),
        "{sdl}"
    );
    assert!(
        sdl.contains(r#"@requiresScopes(scopes: [["records.list", "tenant.active"]])"#),
        "{sdl}"
    );
    assert!(
        sdl.contains(r#"@requiresScopes(scopes: [["records.search"], ["records.admin"]])"#),
        "{sdl}"
    );
    assert!(
        sdl.contains(r#"@requiresScopes(scopes: [["records.page", "tenant.active"]])"#),
        "{sdl}"
    );
}

#[test]
fn argument_scope_templates_drive_metadata_without_literal_federation_scopes() {
    use templated_read_surface::{QueryRoot, TemplatedMetadataRecord};

    let single = operation(
        TemplatedMetadataRecord::generated_graphql_operations(),
        GeneratedGraphqlOperationCategory::SingleRead,
    );
    assert!(matches!(
        single.authorization(),
        GraphqlAuthorizationRequirement::AnyScopes { alternatives }
            if alternatives
                .iter()
                .map(|alternative| {
                    alternative.scopes.iter().map(String::as_str).collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                == vec![vec!["records.{id}.read"], vec!["records.admin"]]
    ));
    assert_eq!(
        GRAPHQL_AUTHORIZATION_FINGERPRINT_ALGORITHM,
        "graphql-orm-authorization-sha256-len-v2"
    );

    let schema = graphql_orm::async_graphql::Schema::build(
        QueryRoot::default(),
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .finish();
    let sdl =
        schema.sdl_with_options(graphql_orm::async_graphql::SDLExportOptions::new().federation());
    let field_name = single.field_name();
    let field_line = sdl
        .lines()
        .find(|line| line.trim_start().starts_with(field_name))
        .expect("templated single-read field in SDL");
    assert!(
        !field_line.contains("requiresScopes"),
        "argument templates must not be emitted as literal Federation scopes: {field_line}"
    );
}

#[test]
fn fixed_write_scope_declarations_drive_metadata_and_sdl() {
    use scoped_write_surface::{MutationRoot, QueryRoot, ScopedWriteRecord, SubscriptionRoot};

    let operations = ScopedWriteRecord::generated_graphql_operations();
    let mutation_categories = [
        GeneratedGraphqlOperationCategory::Create,
        GeneratedGraphqlOperationCategory::Upsert,
        GeneratedGraphqlOperationCategory::Update,
        GeneratedGraphqlOperationCategory::UpdateMany,
        GeneratedGraphqlOperationCategory::Delete,
        GeneratedGraphqlOperationCategory::DeleteMany,
    ];
    for category in mutation_categories {
        assert!(matches!(
            operation(operations, category).authorization(),
            GraphqlAuthorizationRequirement::AllScopes { scopes }
                if scopes == &["records.write".to_string()]
        ));
    }
    assert!(matches!(
        operation(
            operations,
            GeneratedGraphqlOperationCategory::Subscription
        )
        .authorization(),
        GraphqlAuthorizationRequirement::AnyScopes { alternatives }
            if alternatives
                .iter()
                .map(|alternative| {
                    alternative.scopes.iter().map(String::as_str).collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                == vec![vec!["records.events"], vec!["records.admin"]]
    ));

    let schema = graphql_orm::async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .enable_subscription_in_federation()
    .finish();
    let sdl =
        schema.sdl_with_options(graphql_orm::async_graphql::SDLExportOptions::new().federation());

    for category in mutation_categories {
        let field_name = operation(operations, category).field_name();
        assert!(
            root_field_has(
                &sdl,
                field_name,
                r#"@requiresScopes(scopes: [["records.write"]])"#,
            ),
            "generated field `{field_name}` is missing its fixed write scopes in:\n{sdl}"
        );
    }
    let subscription =
        operation(operations, GeneratedGraphqlOperationCategory::Subscription).field_name();
    assert!(
        root_field_has(
            &sdl,
            subscription,
            r#"@federation__requiresScopes(scopes: [["records.events"], ["records.admin"]])"#,
        ),
        "generated field `{subscription}` is missing its event scopes in:\n{sdl}"
    );
}

#[test]
fn required_entity_auth_emits_the_standard_namespaced_federation_directive() {
    use authenticated_surface::{
        AuthenticatedMetadataRecord, MutationRoot, QueryRoot, SubscriptionRoot,
    };

    let operations = AuthenticatedMetadataRecord::generated_graphql_operations();
    assert!(operations.iter().all(|operation| matches!(
        operation.authorization(),
        GraphqlAuthorizationRequirement::Authenticated
    )));

    let schema = graphql_orm::async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .enable_subscription_in_federation()
    .finish();
    let sdl =
        schema.sdl_with_options(graphql_orm::async_graphql::SDLExportOptions::new().federation());

    assert!(
        sdl.contains("directive @federation__authenticated on FIELD_DEFINITION"),
        "{sdl}"
    );
    for operation in operations {
        let name = operation.field_name();
        let protected = root_field_has(&sdl, name, "@federation__authenticated");
        assert!(
            protected,
            "generated field `{name}` is missing standard authentication metadata in:\n{sdl}"
        );
    }
}

#[cfg(feature = "router-protocol")]
#[test]
fn fixed_scope_declarations_export_through_the_optional_protocol_adapter() {
    use graphql_orm_router_protocol::AuthorizationRequirement;
    use scoped_read_surface::graphql_orm_operation_catalog;

    let operations = graphql_orm_operation_catalog()
        .router_protocol_operations()
        .expect("valid fixed-scope protocol export");
    let list = operations
        .iter()
        .find(|operation| operation.field_name == "scopedMetadataRecords")
        .expect("generated list operation");
    assert!(matches!(
        &list.authorization,
        AuthorizationRequirement::AllScopes { scopes }
            if scopes.iter().map(|scope| scope.as_str()).collect::<Vec<_>>()
                == vec!["records.list", "tenant.active"]
    ));
    let search = operations
        .iter()
        .find(|operation| operation.field_name == "scopedMetadataRecordsSearch")
        .expect("generated search operation");
    assert!(matches!(
        &search.authorization,
        AuthorizationRequirement::AnyScopes { alternatives }
            if alternatives
                .iter()
                .map(|alternative| {
                    alternative
                        .scopes
                        .iter()
                        .map(|scope| scope.as_str())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                == vec![vec!["records.search"], vec!["records.admin"]]
    ));
    let keyset = operations
        .iter()
        .find(|operation| operation.field_name == "scopedMetadataRecordsKeyset")
        .expect("generated keyset operation");
    assert!(matches!(
        &keyset.authorization,
        AuthorizationRequirement::AllScopes { scopes }
            if scopes.iter().map(|scope| scope.as_str()).collect::<Vec<_>>()
                == vec!["records.page", "tenant.active"]
    ));
}

#[cfg(feature = "router-protocol")]
#[test]
fn fixed_write_scope_declarations_export_through_the_optional_protocol_adapter() {
    use graphql_orm_router_protocol::AuthorizationRequirement;
    use scoped_write_surface::graphql_orm_operation_catalog;

    let operations = graphql_orm_operation_catalog()
        .router_protocol_operations()
        .expect("valid fixed-scope write protocol export");
    for field_name in [
        "createScopedWriteRecord",
        "upsertScopedWriteRecord",
        "updateScopedWriteRecord",
        "updateScopedWriteRecords",
        "deleteScopedWriteRecord",
        "deleteScopedWriteRecords",
    ] {
        let operation = operations
            .iter()
            .find(|operation| operation.field_name == field_name)
            .unwrap_or_else(|| panic!("generated operation `{field_name}`"));
        assert!(matches!(
            &operation.authorization,
            AuthorizationRequirement::AllScopes { scopes }
                if scopes.iter().map(|scope| scope.as_str()).collect::<Vec<_>>()
                    == vec!["records.write"]
        ));
    }
    let subscription = operations
        .iter()
        .find(|operation| operation.field_name == "scopedWriteRecordChanged")
        .expect("generated subscription operation");
    assert!(matches!(
        &subscription.authorization,
        AuthorizationRequirement::AnyScopes { alternatives }
            if alternatives
                .iter()
                .map(|alternative| {
                    alternative
                        .scopes
                        .iter()
                        .map(|scope| scope.as_str())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                == vec![vec!["records.events"], vec!["records.admin"]]
    ));
}

#[cfg(feature = "router-protocol")]
#[test]
fn argument_scope_templates_export_with_the_referenced_argument_contract() {
    use graphql_orm_router_protocol::AuthorizationRequirement;
    use templated_read_surface::graphql_orm_operation_catalog;

    let operations = graphql_orm_operation_catalog()
        .router_protocol_operations()
        .expect("valid templated protocol export");
    let single = operations
        .iter()
        .find(|operation| operation.field_name == "templatedMetadataRecord")
        .expect("templated single-read operation");
    assert_eq!(single.arguments.len(), 1);
    assert_eq!(single.arguments[0].name, "id");
    assert_eq!(single.arguments[0].graphql_type, "String!");
    assert!(single.arguments[0].required);
    assert!(matches!(
        &single.authorization,
        AuthorizationRequirement::AnyScopes { alternatives }
            if alternatives
                .iter()
                .map(|alternative| {
                    alternative
                        .scopes
                        .iter()
                        .map(|scope| scope.as_str())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                == vec![vec!["records.{id}.read"], vec!["records.admin"]]
    ));
}

#[cfg(feature = "router-protocol")]
#[test]
fn authenticated_declarations_export_through_the_optional_protocol_adapter() {
    use authenticated_surface::graphql_orm_operation_catalog;
    use graphql_orm_router_protocol::AuthorizationRequirement;

    let operations = graphql_orm_operation_catalog()
        .router_protocol_operations()
        .expect("valid authenticated protocol export");
    assert!(!operations.is_empty());
    assert!(operations.iter().all(|operation| matches!(
        operation.authorization,
        AuthorizationRequirement::Authenticated
    )));
}

#[tokio::test]
async fn fixed_write_scope_declarations_are_enforced_before_runtime_access() {
    use graphql_orm::futures::StreamExt as _;
    use scoped_write_surface::{MutationRoot, QueryRoot, SubscriptionRoot};

    let schema = graphql_orm::async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .data(AuthSubject::new("writer-without-scopes"))
    .finish();

    for mutation in [
        r#"mutation { createScopedWriteRecord(input: { slug: "created", label: "Created" }) { success } }"#,
        r#"mutation { upsertScopedWriteRecord(input: { slug: "upserted", label: "Upserted" }) { success } }"#,
        r#"mutation { updateScopedWriteRecord(id: "record-1", input: { label: "Updated" }) { success } }"#,
        r#"mutation { updateScopedWriteRecords(where: { id: { eq: "record-1" } }, input: { label: "Updated" }) { success } }"#,
        r#"mutation { deleteScopedWriteRecord(id: "record-1") { success } }"#,
        r#"mutation { deleteScopedWriteRecords(where: { id: { eq: "record-1" } }) { success } }"#,
    ] {
        let response = schema.execute(mutation).await;
        assert_eq!(response.errors.len(), 1, "{mutation}: {response:?}");
        assert_eq!(response.errors[0].message, "forbidden", "{mutation}");
    }

    let mut stream =
        Box::pin(schema.execute_stream("subscription { scopedWriteRecordChanged { action } }"));
    let response = stream.next().await.expect("subscription response");
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(response.errors[0].message, "forbidden");
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn argument_scope_templates_are_resolved_from_coerced_scalar_arguments() {
    use templated_read_surface::schema_builder;

    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    sqlx::query(
        "CREATE TABLE resolver_metadata_templated_records (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .expect("templated records table");
    sqlx::query(
        "INSERT INTO resolver_metadata_templated_records (id, label) VALUES ('record-1', 'Visible')",
    )
    .execute(&pool)
    .await
    .expect("templated record");

    let scoped_schema = schema_builder(Database::new(pool.clone()))
        .data(AuthSubject::from_parts(
            "record-reader",
            Vec::new(),
            vec!["records.record-1.read".to_string()],
            None,
        ))
        .finish();
    let allowed = scoped_schema
        .execute("{ templatedMetadataRecord(id: \"record-1\") { id label } }")
        .await;
    assert!(allowed.errors.is_empty(), "{allowed:?}");

    let denied = scoped_schema
        .execute("{ templatedMetadataRecord(id: \"record-2\") { id } }")
        .await;
    assert_eq!(denied.errors.len(), 1, "{denied:?}");
    assert_eq!(denied.errors[0].message, "forbidden");

    let admin_schema = schema_builder(Database::new(pool))
        .data(AuthSubject::from_parts(
            "record-admin",
            Vec::new(),
            vec!["records.admin".to_string()],
            None,
        ))
        .finish();
    let admin = admin_schema
        .execute("{ templatedMetadataRecord(id: \"record-1\") { id } }")
        .await;
    assert!(admin.errors.is_empty(), "{admin:?}");
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn fixed_read_scope_declarations_are_enforced_before_database_access() {
    use scoped_read_surface::schema_builder;

    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    sqlx::query(
        "CREATE TABLE resolver_metadata_scoped_records (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .expect("scoped records table");
    sqlx::query(
        "INSERT INTO resolver_metadata_scoped_records (id, label) VALUES ('record-1', 'visible')",
    )
    .execute(&pool)
    .await
    .expect("scoped record");

    let schema = schema_builder(Database::new(pool.clone()))
        .data(AuthSubject::new("scope-less-user"))
        .finish();
    let response = schema
        .execute("{ scopedMetadataRecord(id: \"missing\") { id } }")
        .await;

    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(response.errors[0].message, "forbidden");

    let search = schema
        .execute(
            "{ scopedMetadataRecordsSearch(search: { query: \"visible\" }) { edges { node { id } } } }",
        )
        .await;
    assert_eq!(search.errors.len(), 1, "{search:?}");
    assert_eq!(search.errors[0].message, "forbidden");

    let keyset = schema
        .execute("{ scopedMetadataRecordsKeyset(page: { limit: 1 }) { edges { node { id } } } }")
        .await;
    assert_eq!(keyset.errors.len(), 1, "{keyset:?}");
    assert_eq!(keyset.errors[0].message, "forbidden");

    let partial_list_schema = schema_builder(Database::new(pool.clone()))
        .data(AuthSubject::from_parts(
            "partial-list-user",
            Vec::new(),
            vec!["records.list".to_string()],
            None,
        ))
        .finish();
    let partial_list = partial_list_schema
        .execute("{ scopedMetadataRecords { edges { node { id } } } }")
        .await;
    assert_eq!(partial_list.errors.len(), 1, "{partial_list:?}");
    assert_eq!(partial_list.errors[0].message, "forbidden");

    let permitted_schema = schema_builder(Database::new(pool))
        .data(AuthSubject::from_parts(
            "scoped-user",
            Vec::new(),
            vec![
                "records.admin".to_string(),
                "records.audit".to_string(),
                "records.list".to_string(),
                "records.page".to_string(),
                "tenant.active".to_string(),
            ],
            None,
        ))
        .finish();
    let permitted = permitted_schema
        .execute("{ scopedMetadataRecord(id: \"record-1\") { id label } }")
        .await;
    assert!(permitted.errors.is_empty(), "{permitted:?}");
    assert_eq!(
        permitted.data.into_json().expect("response JSON"),
        serde_json::json!({"scopedMetadataRecord": {"id": "record-1", "label": "visible"}})
    );
    let permitted_list = permitted_schema
        .execute("{ scopedMetadataRecords { edges { node { id } } } }")
        .await;
    assert!(permitted_list.errors.is_empty(), "{permitted_list:?}");
    let permitted_keyset = permitted_schema
        .execute("{ scopedMetadataRecordsKeyset(page: { limit: 1 }) { edges { node { id } } } }")
        .await;
    assert!(permitted_keyset.errors.is_empty(), "{permitted_keyset:?}");
}
