// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

//! `mutation_shapes` narrows generated mutations per category: the GraphQL
//! root, the operation and semantic catalogues and the orphaned input and
//! result types follow, while repository writes, relations and `Changed`
//! subscriptions do not.
#![cfg(feature = "sqlite")]

use graphql_orm::graphql::orm::{GraphqlOperationKind, GraphqlOperationMetadata};
/// Emits a module holding the same two entities and schema roots with
/// `$shapes` applied (or not) to `Parcel`.
macro_rules! parcel_module {
    ($module:ident, $($attributes:tt)*) => {
        mod $module {
            use graphql_orm::prelude::*;

            #[derive(
                GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
            )]
            #[graphql_entity(table = "shelves", plural = "Shelves", default_sort = "label ASC")]
            pub struct Shelf {
                #[primary_key]
                #[filterable(type = "uuid")]
                pub id: graphql_orm::uuid::Uuid,
                #[filterable(type = "string")]
                #[sortable]
                pub label: String,
            }

            #[derive(
                GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
            )]
            #[graphql_entity(
                table = "parcels",
                plural = "Parcels",
                default_sort = "code ASC",
                upsert = "code",
                $($attributes)*
            )]
            pub struct Parcel {
                #[primary_key]
                #[filterable(type = "uuid")]
                pub id: graphql_orm::uuid::Uuid,
                #[filterable(type = "uuid")]
                pub shelf_id: graphql_orm::uuid::Uuid,
                #[unique]
                #[filterable(type = "string")]
                #[sortable]
                pub code: String,
                #[graphql(skip)]
                #[relation(target = "Shelf", from = "shelf_id", to = "id")]
                pub shelf: Option<Shelf>,
            }

            schema_roots! {
                query_custom_ops: [],
                entities: [Shelf, Parcel],
            }

            #[allow(dead_code)]
            pub async fn database() -> Database<SqliteBackend> {
                let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
                    .await
                    .unwrap();
                for statement in [
                    "CREATE TABLE shelves (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
                    "CREATE TABLE parcels (id TEXT PRIMARY KEY, shelf_id TEXT NOT NULL, code TEXT NOT NULL UNIQUE)",
                ] {
                    graphql_orm::sqlx::query(statement)
                        .execute(database.pool())
                        .await
                        .unwrap();
                }
                database
            }
        }
    };
}

parcel_module!(omitted,);
parcel_module!(
    all_selected,
    mutation_shapes = "create,upsert,update,update_many,delete,delete_many"
);
parcel_module!(subset, mutation_shapes = "update, create");
parcel_module!(bulk_only, mutation_shapes = "delete_many");

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

#[tokio::test]
async fn omission_and_full_selection_produce_byte_identical_sdl() {
    let omitted = omitted::schema_builder(omitted::database().await)
        .finish()
        .sdl();
    let all = all_selected::schema_builder(all_selected::database().await)
        .finish()
        .sdl();
    assert_eq!(omitted, all);
    let golden = format!(
        "{}/tests/goldens/mutation_shapes_omitted.graphql",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        std::fs::write(&golden, &omitted).unwrap();
    }
    assert_eq!(omitted, std::fs::read_to_string(&golden).unwrap());
    assert!(omitted.contains("deleteParcels("));
    assert!(omitted.contains("upsertParcel("));
}

#[tokio::test]
async fn disabled_mutations_leave_the_schema_and_orphaned_types_with_them() {
    let sdl = subset::schema_builder(subset::database().await)
        .finish()
        .sdl();
    let fields = mutation_fields(&sdl);
    assert_eq!(
        fields
            .iter()
            .filter(|field| field.contains("arcel"))
            .collect::<Vec<_>>(),
        ["createParcel", "updateParcel"]
    );
    // The unselected entity keeps everything.
    assert!(fields.contains(&"deleteShelves".to_string()));
    // Subscriptions, queries and relations are untouched.
    assert!(sdl.contains("parcelChanged("));
    assert!(sdl.contains("parcels("));
    // Input and result types only a disabled root referenced are gone; types
    // a selected root still uses remain.
    assert!(!sdl.contains("DeleteParcelsResult"));
    assert!(!sdl.contains("UpdateParcelsResult"));
    assert!(!sdl.contains("UpsertParcelResult"));
    // Shelf keeps its bulk results; Parcel keeps what create and update use.
    assert!(sdl.contains("DeleteShelvesResult"));
    assert!(sdl.contains("ParcelResult"));
    assert!(sdl.contains("UpdateParcelInput"));
    assert!(sdl.contains("CreateParcelInput"));
}

#[tokio::test]
async fn catalogues_follow_the_selection_and_other_kinds_are_unchanged() {
    let mutations = operations::<subset::Parcel>(GraphqlOperationKind::Mutation);
    assert_eq!(mutations, ["createParcel", "updateParcel"]);
    let bulk = operations::<bulk_only::Parcel>(GraphqlOperationKind::Mutation);
    assert_eq!(bulk, ["deleteParcels"]);
    assert_eq!(
        operations::<subset::Parcel>(GraphqlOperationKind::Query),
        operations::<omitted::Parcel>(GraphqlOperationKind::Query)
    );
    assert_eq!(
        operations::<subset::Parcel>(GraphqlOperationKind::Subscription),
        ["parcelChanged"]
    );

    let catalog = subset::graphql_orm_operation_catalog();
    let public = catalog
        .operations()
        .iter()
        .filter(|operation| operation.entity_name() == "Parcel")
        .filter(|operation| operation.kind() == GraphqlOperationKind::Mutation)
        .map(|operation| operation.field_name().to_string())
        .collect::<Vec<_>>();
    assert_eq!(public, ["createParcel", "updateParcel"]);

    let semantic = subset::graphql_orm_semantic_catalog();
    let semantic_mutations = semantic
        .operations
        .iter()
        .filter(|operation| operation.kind == GraphqlOperationKind::Mutation)
        .filter(|operation| operation.generated_entity_name.as_deref() == Some("Parcel"))
        .map(|operation| operation.field_name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        semantic_mutations
            .iter()
            .collect::<std::collections::BTreeSet<_>>(),
        ["createParcel".to_string(), "updateParcel".to_string()]
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
    );
}

#[tokio::test]
async fn repository_writes_relations_and_changed_events_are_unaffected() {
    use graphql_orm::futures::{Stream, StreamExt};
    let database = subset::database().await;
    let schema = subset::schema_builder(database.clone())
        .data(graphql_orm::graphql::auth::AuthSubject::new("test-user"))
        .finish();
    let shelf = subset::Shelf::insert(&database, subset::CreateShelfInput { label: "A".into() })
        .await
        .unwrap();
    let parcel = subset::Parcel::insert(
        &database,
        subset::CreateParcelInput {
            shelf_id: shelf.id,
            code: "p1".into(),
        },
    )
    .await
    .unwrap();
    // Repository delete and upsert remain available even though the GraphQL
    // roots are not.
    subset::Parcel::delete_by_id(&database, &parcel.id)
        .await
        .unwrap();
    let upserted = subset::Parcel::upsert(
        &database,
        subset::CreateParcelInput {
            shelf_id: shelf.id,
            code: "p1".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(upserted.entity.code, "p1");

    // A disabled root is refused by the schema, not by an application check.
    let refused = schema
        .execute(
            "mutation { deleteParcel(id: \"00000000-0000-0000-0000-000000000000\") { success } }",
        )
        .await;
    assert!(!refused.errors.is_empty());

    // A selected mutation still works and still feeds `Changed`.
    let mut stream = Box::pin(
        schema.execute_stream(
            graphql_orm::async_graphql::Request::new("subscription { parcelChanged { action } }")
                .data(graphql_orm::graphql::auth::AuthSubject::new("test-user")),
        ),
    );
    // A stream registers only once polled; nothing is pending yet.
    graphql_orm::futures::future::poll_fn(|cx| {
        use std::task::Poll;
        match stream.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(item) => panic!("unexpected early item: {item:?}"),
        }
    })
    .await;
    let response = schema
        .execute(format!(
            "mutation {{ createParcel(input: {{ shelfId: \"{}\", code: \"p2\" }}) {{ success }} }}",
            shelf.id
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .expect("Changed event")
        .expect("stream item");
    assert!(event.errors.is_empty(), "{:?}", event.errors);
}

mod append_only_create {
    use graphql_orm::prelude::*;

    /// `append_only` already limits an entity to `create`; stating it is
    /// accepted and changes nothing.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        table = "shape_audit_events",
        plural = "ShapeAuditEvents",
        append_only = true,
        mutation_shapes = "create"
    )]
    pub struct ShapeAuditEvent {
        #[primary_key]
        pub id: graphql_orm::uuid::Uuid,
        #[filterable(type = "string")]
        #[sortable]
        pub kind: String,
    }

    schema_roots! {
        query_custom_ops: [],
        entities: [ShapeAuditEvent],
    }
}

#[tokio::test]
async fn append_only_accepts_only_the_create_it_already_has() {
    use graphql_orm::prelude::*;
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let sdl = append_only_create::schema_builder(database).finish().sdl();
    assert!(sdl.contains("createShapeAuditEvent("));
    assert!(!sdl.contains("deleteShapeAuditEvent"));
    assert_eq!(
        operations::<append_only_create::ShapeAuditEvent>(GraphqlOperationKind::Mutation),
        ["createShapeAuditEvent"]
    );
}
