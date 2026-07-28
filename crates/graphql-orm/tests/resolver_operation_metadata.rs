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
