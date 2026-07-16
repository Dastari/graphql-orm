#![cfg(feature = "sqlite")]

use graphql_orm::graphql::orm::{
    CollectionId, FieldId, RelationCardinality, RelationId, RelationKeyPair, RuntimeCollection,
    RuntimeField, RuntimeFieldState, RuntimeOrderDirection, RuntimeOrderTerm, RuntimePageRequest,
    RuntimeQueryLimits, RuntimeRelation, RuntimeRelationErrorCode, RuntimeRelationLimits,
    RuntimeRelationSelection, RuntimeRelationValue, RuntimeSchema, RuntimeValueKind,
};

fn cid(value: &str) -> CollectionId {
    CollectionId::new(value).unwrap()
}

fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}

fn field(id: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        id: fid(id),
        api_name: id.to_string(),
        physical_column: id.to_string(),
        value_kind: kind,
        nullable,
        unique: id.ends_with("_id") || id == "id",
        filterable: true,
        sortable: !matches!(kind, RuntimeValueKind::Json),
        generated: false,
        default: None,
    }
}

fn schema() -> graphql_orm::graphql::orm::ValidatedRuntimeSchema {
    let customer_id = field("customer_id", RuntimeValueKind::Integer, false);
    let contact_id = field("contact_id", RuntimeValueKind::Integer, false);
    let note_id = field("note_id", RuntimeValueKind::Integer, false);
    RuntimeSchema {
        format_version: 1,
        collections: vec![
            RuntimeCollection {
                id: cid("customers"),
                api_type_name: "Customer".into(),
                api_plural_name: "Customers".into(),
                physical_table: "runtime_relation_customers".into(),
                primary_key: vec![customer_id.id.clone()],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    customer_id.clone(),
                    field("status", RuntimeValueKind::String, false),
                    field("primary_contact_id", RuntimeValueKind::Integer, true),
                ],
                relations: vec![
                    RuntimeRelation {
                        id: RelationId::new("customer_contacts").unwrap(),
                        api_name: "contacts".into(),
                        target: cid("contacts"),
                        key_pairs: vec![RelationKeyPair {
                            source: customer_id.id.clone(),
                            target: fid("owner_id"),
                        }],
                        cardinality: RelationCardinality::Many,
                        enforce_foreign_key: false,
                        on_delete: None,
                    },
                    RuntimeRelation {
                        id: RelationId::new("customer_primary_contact").unwrap(),
                        api_name: "primaryContact".into(),
                        target: cid("contacts"),
                        key_pairs: vec![RelationKeyPair {
                            source: fid("primary_contact_id"),
                            target: contact_id.id.clone(),
                        }],
                        cardinality: RelationCardinality::One,
                        enforce_foreign_key: false,
                        on_delete: None,
                    },
                ],
                indexes: vec![],
                composite_unique: vec![],
                default_order: vec![RuntimeOrderTerm {
                    field: customer_id.id.clone(),
                    direction: RuntimeOrderDirection::Asc,
                }],
            },
            RuntimeCollection {
                id: cid("contacts"),
                api_type_name: "Contact".into(),
                api_plural_name: "Contacts".into(),
                physical_table: "runtime_relation_contacts".into(),
                primary_key: vec![contact_id.id.clone()],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    contact_id.clone(),
                    field("owner_id", RuntimeValueKind::Integer, false),
                    field("label", RuntimeValueKind::String, false),
                    field("secret", RuntimeValueKind::Bytes, false),
                ],
                relations: vec![RuntimeRelation {
                    id: RelationId::new("contact_notes").unwrap(),
                    api_name: "notes".into(),
                    target: cid("notes"),
                    key_pairs: vec![RelationKeyPair {
                        source: contact_id.id.clone(),
                        target: fid("note_contact_id"),
                    }],
                    cardinality: RelationCardinality::Many,
                    enforce_foreign_key: false,
                    on_delete: None,
                }],
                indexes: vec![],
                composite_unique: vec![],
                default_order: vec![RuntimeOrderTerm {
                    field: contact_id.id.clone(),
                    direction: RuntimeOrderDirection::Asc,
                }],
            },
            RuntimeCollection {
                id: cid("notes"),
                api_type_name: "Note".into(),
                api_plural_name: "Notes".into(),
                physical_table: "runtime_relation_notes".into(),
                primary_key: vec![note_id.id.clone()],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    note_id.clone(),
                    field("note_contact_id", RuntimeValueKind::Integer, false),
                    field("body", RuntimeValueKind::String, false),
                ],
                relations: vec![],
                indexes: vec![],
                composite_unique: vec![],
                default_order: vec![RuntimeOrderTerm {
                    field: note_id.id.clone(),
                    direction: RuntimeOrderDirection::Asc,
                }],
            },
        ],
    }
    .validate()
    .unwrap()
}

#[tokio::test]
async fn batches_hidden_parent_keys_and_bounded_nested_keysets() {
    let schema = schema();
    let customers = schema.resolve_collection(&cid("customers")).unwrap();
    let contacts = schema.resolve_collection(&cid("contacts")).unwrap();
    let notes = schema.resolve_collection(&cid("notes")).unwrap();
    let status = schema.resolve_field(&customers, &fid("status")).unwrap();
    let customer_id = schema
        .resolve_field(&customers, &fid("customer_id"))
        .unwrap();
    let label = schema.resolve_field(&contacts, &fid("label")).unwrap();
    let owner_id = schema.resolve_field(&contacts, &fid("owner_id")).unwrap();
    let contact_id = schema.resolve_field(&contacts, &fid("contact_id")).unwrap();
    let body = schema.resolve_field(&notes, &fid("body")).unwrap();
    let relation = schema
        .resolve_relation(&customers, &RelationId::new("customer_contacts").unwrap())
        .unwrap();
    let primary_relation = schema
        .resolve_relation(
            &customers,
            &RelationId::new("customer_primary_contact").unwrap(),
        )
        .unwrap();
    let note_relation = schema
        .resolve_relation(&contacts, &RelationId::new("contact_notes").unwrap())
        .unwrap();
    let customer_projection = schema
        .resolve_projection(&customers, std::slice::from_ref(&status))
        .unwrap();
    let contact_projection = schema
        .resolve_projection(&contacts, std::slice::from_ref(&label))
        .unwrap();
    let note_projection = schema
        .resolve_projection(&notes, std::slice::from_ref(&body))
        .unwrap();
    let query_limits = RuntimeQueryLimits::default();
    let parent_request = schema
        .runtime_read_request_with_relation_keys(
            &customers,
            &customer_projection,
            None,
            schema
                .runtime_order(&customers, None, query_limits)
                .unwrap(),
            RuntimePageRequest::first(10, None),
            false,
            &[relation.clone(), primary_relation.clone()],
            query_limits,
        )
        .unwrap();

    let database = graphql_orm::db::Database::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    graphql_orm::sqlx::query(
        "CREATE TABLE runtime_relation_customers (
            customer_id INTEGER PRIMARY KEY, status TEXT NOT NULL,
            primary_contact_id INTEGER NULL
         )",
    )
    .execute(database.pool())
    .await
    .unwrap();
    graphql_orm::sqlx::query(
        "CREATE TABLE runtime_relation_notes (
            note_id INTEGER PRIMARY KEY, note_contact_id INTEGER NOT NULL,
            body TEXT NOT NULL
         )",
    )
    .execute(database.pool())
    .await
    .unwrap();
    graphql_orm::sqlx::query(
        "CREATE TABLE runtime_relation_contacts (
            contact_id INTEGER PRIMARY KEY, owner_id INTEGER NOT NULL,
            label TEXT NOT NULL, secret BLOB NOT NULL
         )",
    )
    .execute(database.pool())
    .await
    .unwrap();
    for (id, status, primary_contact_id) in [
        (1_i64, "active", Some(10_i64)),
        (2, "active", None),
        (3, "empty", None),
    ] {
        graphql_orm::sqlx::query(
            "INSERT INTO runtime_relation_customers
             (customer_id, status, primary_contact_id) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(status)
        .bind(primary_contact_id)
        .execute(database.pool())
        .await
        .unwrap();
    }
    for (id, owner, label) in [(10_i64, 1_i64, "a"), (11, 1, "b"), (20, 2, "c")] {
        graphql_orm::sqlx::query(
            "INSERT INTO runtime_relation_contacts (contact_id, owner_id, label, secret)
             VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(owner)
        .bind(label)
        .bind(vec![0xff_u8, id as u8])
        .execute(database.pool())
        .await
        .unwrap();
    }
    for (id, contact, body) in [(100_i64, 10_i64, "first"), (200, 20, "second")] {
        graphql_orm::sqlx::query(
            "INSERT INTO runtime_relation_notes (note_id, note_contact_id, body) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(contact)
        .bind(body)
        .execute(database.pool())
        .await
        .unwrap();
    }

    let parents = database
        .execute_runtime_anchored_read(&parent_request, None)
        .await
        .unwrap();
    assert_eq!(parents.edges.len(), 3);
    assert_eq!(
        parents.edges[0].node.state(&customer_id).unwrap(),
        RuntimeFieldState::Unloaded
    );
    let anchors = parents.relation_parents(&relation).unwrap();
    assert!(format!("{:?}", anchors[0]).contains("[redacted]"));
    assert!(!format!("{:?}", anchors[0]).contains("Integer(1)"));
    assert_eq!(
        schema
            .runtime_relation_batch_request(
                &relation,
                vec![anchors[0].clone(), anchors[0].clone()],
                &contact_projection,
                None,
                schema.runtime_order(&contacts, None, query_limits).unwrap(),
                RuntimeRelationSelection::ToMany {
                    pages: vec![RuntimePageRequest::first(1, None); 2],
                    include_count: false,
                },
                RuntimeRelationLimits::default(),
            )
            .unwrap_err()
            .code(),
        RuntimeRelationErrorCode::InvalidParent
    );
    assert_eq!(
        schema
            .runtime_relation_batch_request(
                &relation,
                anchors.clone(),
                &contact_projection,
                None,
                schema.runtime_order(&contacts, None, query_limits).unwrap(),
                RuntimeRelationSelection::ToMany {
                    pages: vec![RuntimePageRequest::first(1, None); anchors.len()],
                    include_count: false,
                },
                RuntimeRelationLimits {
                    max_parents: 1,
                    ..Default::default()
                },
            )
            .unwrap_err()
            .code(),
        RuntimeRelationErrorCode::ResourceLimit
    );
    let primary_anchors = parents.relation_parents(&primary_relation).unwrap();
    assert!(!primary_anchors[0].is_null_key());
    assert!(primary_anchors[1].is_null_key());
    let primary_request = schema
        .runtime_relation_batch_request(
            &primary_relation,
            primary_anchors,
            &contact_projection,
            None,
            schema.runtime_order(&contacts, None, query_limits).unwrap(),
            RuntimeRelationSelection::ToOne,
            RuntimeRelationLimits::default(),
        )
        .unwrap();
    let primary = database
        .execute_runtime_relation_batch(&primary_request, None)
        .await
        .unwrap();
    assert!(matches!(
        &primary.results[0].value,
        RuntimeRelationValue::ToOne(Some(_))
    ));
    assert!(matches!(
        &primary.results[1].value,
        RuntimeRelationValue::ToOne(None)
    ));

    let order = schema.runtime_order(&contacts, None, query_limits).unwrap();
    assert_eq!(
        schema
            .runtime_relation_batch_request_with_relation_keys(
                &relation,
                anchors.clone(),
                &contact_projection,
                None,
                order.clone(),
                RuntimeRelationSelection::ToMany {
                    pages: vec![RuntimePageRequest::first(1, None); anchors.len()],
                    include_count: false,
                },
                std::slice::from_ref(&relation),
                RuntimeRelationLimits::default(),
            )
            .unwrap_err()
            .code(),
        RuntimeRelationErrorCode::InvalidRelation
    );
    assert_eq!(
        primary.relation_parents(&note_relation).unwrap_err().code(),
        RuntimeRelationErrorCode::InvalidRelation
    );
    let batch_request = schema
        .runtime_relation_batch_request_with_relation_keys(
            &relation,
            anchors.clone(),
            &contact_projection,
            None,
            order.clone(),
            RuntimeRelationSelection::ToMany {
                pages: vec![RuntimePageRequest::first(1, None); anchors.len()],
                include_count: true,
            },
            std::slice::from_ref(&note_relation),
            RuntimeRelationLimits::default(),
        )
        .unwrap();
    let batch = database
        .execute_runtime_relation_batch(&batch_request, None)
        .await
        .unwrap();
    assert_eq!(batch.results.len(), 3);
    let RuntimeRelationValue::ToMany(first) = &batch.results[0].value else {
        panic!("expected to-many result")
    };
    assert_eq!(first.edges[0].node.string(&label).unwrap(), "a");
    assert_eq!(first.total_count, Some(2));
    assert!(first.page_info.has_next_page);
    assert_eq!(
        first.edges[0].node.state(&owner_id).unwrap(),
        RuntimeFieldState::Unloaded
    );
    assert_eq!(
        first.edges[0].node.state(&contact_id).unwrap(),
        RuntimeFieldState::Unloaded
    );
    let RuntimeRelationValue::ToMany(empty) = &batch.results[2].value else {
        panic!("expected to-many result")
    };
    assert!(empty.edges.is_empty());
    assert_eq!(empty.total_count, Some(0));

    // One next-layer batch call resolves notes for both nonempty contact
    // parents. Hidden contact IDs remain available only through the anchors.
    let note_anchors = batch.relation_parents(&note_relation).unwrap();
    assert_eq!(note_anchors.len(), 2);
    assert_eq!(note_anchors[0].parent_index(), 0);
    assert_eq!(note_anchors[1].parent_index(), 1);
    let note_request = schema
        .runtime_relation_batch_request(
            &note_relation,
            note_anchors.clone(),
            &note_projection,
            None,
            schema.runtime_order(&notes, None, query_limits).unwrap(),
            RuntimeRelationSelection::ToMany {
                pages: vec![RuntimePageRequest::first(10, None); note_anchors.len()],
                include_count: false,
            },
            RuntimeRelationLimits::default(),
        )
        .unwrap();
    let note_batch = database
        .execute_runtime_relation_batch(&note_request, None)
        .await
        .unwrap();
    let RuntimeRelationValue::ToMany(first_notes) = &note_batch.results[0].value else {
        panic!("expected nested to-many result")
    };
    let RuntimeRelationValue::ToMany(second_notes) = &note_batch.results[1].value else {
        panic!("expected nested to-many result")
    };
    assert_eq!(first_notes.edges[0].node.string(&body).unwrap(), "first");
    assert_eq!(second_notes.edges[0].node.string(&body).unwrap(), "second");

    let cursor = first.page_info.end_cursor.clone();
    let wrong_parent_cursor = schema
        .runtime_relation_batch_request(
            &relation,
            anchors.clone(),
            &contact_projection,
            None,
            order.clone(),
            RuntimeRelationSelection::ToMany {
                pages: vec![
                    RuntimePageRequest::first(1, None),
                    RuntimePageRequest::first(1, cursor.clone()),
                    RuntimePageRequest::first(1, None),
                ],
                include_count: false,
            },
            RuntimeRelationLimits::default(),
        )
        .unwrap();
    assert_eq!(
        database
            .execute_runtime_relation_batch(&wrong_parent_cursor, None)
            .await
            .unwrap_err()
            .code(),
        RuntimeRelationErrorCode::CursorMismatch
    );
    let next_request = schema
        .runtime_relation_batch_request(
            &relation,
            anchors.clone(),
            &contact_projection,
            None,
            order,
            RuntimeRelationSelection::ToMany {
                pages: vec![
                    RuntimePageRequest::first(1, cursor),
                    RuntimePageRequest::first(1, None),
                    RuntimePageRequest::first(1, None),
                ],
                include_count: false,
            },
            RuntimeRelationLimits::default(),
        )
        .unwrap();
    let next = database
        .execute_runtime_relation_batch(&next_request, None)
        .await
        .unwrap();
    let RuntimeRelationValue::ToMany(second) = &next.results[0].value else {
        panic!("expected to-many result")
    };
    assert_eq!(second.edges[0].node.string(&label).unwrap(), "b");
    assert!(!second.page_info.has_next_page);
}
