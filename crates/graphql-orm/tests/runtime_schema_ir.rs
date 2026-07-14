//! Acceptance tests for the owned runtime schema IR.
//!
//! The representative sample schema exercises every scalar value kind, defaults, unique and
//! composite-unique constraints, secondary indexes, a composite primary key, to-one/to-many
//! relations, a composite-key relation, and cascade/restrict delete behavior. One static derive
//! graph and one hand-built catalog-style runtime graph must agree on the ID-free structural
//! fingerprint.

use graphql_orm::graphql::orm::{
    CollectionId, DeletePolicy, Entity, FieldId, IndexId, RelationCardinality, RelationId,
    RelationKeyPair, RuntimeCollection, RuntimeDefault, RuntimeField, RuntimeIndex,
    RuntimeOrderDirection, RuntimeOrderTerm, RuntimeRelation, RuntimeSchema,
    RuntimeSchemaDiagnosticCode, RuntimeValueKind,
};
use graphql_orm_macros::GraphQLSchemaEntity;

// ---------------------------------------------------------------------------
// Static side: derive graph for the representative sample schema.
// ---------------------------------------------------------------------------

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "customers",
    plural = "Customers",
    default_sort = "created_at ASC",
    index = "name"
)]
pub struct Customer {
    #[primary_key]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[unique]
    #[filterable(type = "string")]
    pub email: String,

    #[filterable(type = "number")]
    #[sortable]
    #[graphql_orm(default = "0")]
    pub loyalty_points: i64,

    pub balance: Option<f64>,

    #[filterable(type = "boolean")]
    #[graphql_orm(default = "true")]
    pub is_active: bool,

    #[json_field]
    pub preferences: Option<serde_json::Value>,

    pub avatar: Option<Vec<u8>>,

    #[date_field]
    #[sortable]
    pub created_at: String,

    #[graphql(skip)]
    #[relation(target = "ContactDetail", from = "id", to = "customer_id", multiple)]
    pub contact_details: Vec<ContactDetail>,

    #[graphql(skip)]
    #[relation(target = "CustomerNote", from = "id", to = "customer_id", multiple)]
    pub notes: Vec<CustomerNote>,
}

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "contact_details",
    plural = "ContactDetails",
    unique_composite = "customer_id,kind",
    index = "customer_id"
)]
pub struct ContactDetail {
    #[primary_key]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub customer_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    pub kind: String,

    pub value: String,

    #[graphql(skip)]
    #[relation(
        target = "Customer",
        from = "customer_id",
        to = "id",
        on_delete = "cascade"
    )]
    pub customer: Option<Customer>,
}

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "customer_notes",
    plural = "CustomerNotes",
    default_sort = "seq ASC"
)]
pub struct CustomerNote {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub customer_id: graphql_orm::uuid::Uuid,

    #[primary_key]
    #[sortable]
    pub seq: i64,

    pub body: String,

    #[graphql(skip)]
    #[relation(
        target = "Customer",
        from = "customer_id",
        to = "id",
        on_delete = "cascade"
    )]
    pub customer: Option<Customer>,

    #[graphql(skip)]
    #[relation(
        target = "NoteAttachment",
        from = ["customer_id", "seq"],
        to = ["customer_id", "note_seq"],
        multiple
    )]
    pub attachments: Vec<NoteAttachment>,
}

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "note_attachments",
    plural = "NoteAttachments",
    index = "customer_id,note_seq"
)]
pub struct NoteAttachment {
    #[primary_key]
    pub id: graphql_orm::uuid::Uuid,

    pub customer_id: graphql_orm::uuid::Uuid,

    pub note_seq: i64,

    pub file_name: String,

    #[graphql(skip)]
    #[relation(
        target = "CustomerNote",
        from = ["customer_id", "note_seq"],
        to = ["customer_id", "seq"],
        on_delete = "restrict"
    )]
    pub note: Option<CustomerNote>,
}

fn static_metadata() -> Vec<&'static graphql_orm::graphql::orm::EntityMetadata> {
    vec![
        Customer::metadata(),
        ContactDetail::metadata(),
        CustomerNote::metadata(),
        NoteAttachment::metadata(),
    ]
}

// ---------------------------------------------------------------------------
// Runtime side: hand-built owned IR with catalog-style stable IDs.
// ---------------------------------------------------------------------------

fn cid(id: &str) -> CollectionId {
    CollectionId::new(id).expect("valid collection id")
}

fn fid(id: &str) -> FieldId {
    FieldId::new(id).expect("valid field id")
}

fn field(id: &str, api: &str, column: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        id: fid(id),
        api_name: api.to_string(),
        physical_column: column.to_string(),
        value_kind: kind,
        nullable: false,
        unique: false,
        filterable: false,
        sortable: false,
        generated: false,
        default: None,
    }
}

fn relation(
    id: &str,
    api: &str,
    target: &str,
    pairs: &[(&str, &str)],
    cardinality: RelationCardinality,
    on_delete: Option<DeletePolicy>,
) -> RuntimeRelation {
    RuntimeRelation {
        id: RelationId::new(id).expect("valid relation id"),
        api_name: api.to_string(),
        target: cid(target),
        key_pairs: pairs
            .iter()
            .map(|(source, target)| RelationKeyPair {
                source: fid(source),
                target: fid(target),
            })
            .collect(),
        cardinality,
        enforce_foreign_key: on_delete.is_some(),
        on_delete,
    }
}

fn index(id: &str, name: &str, fields: &[&str], unique: bool) -> RuntimeIndex {
    RuntimeIndex {
        id: IndexId::new(id).expect("valid index id"),
        name: name.to_string(),
        fields: fields.iter().map(|f| fid(f)).collect(),
        unique,
    }
}

fn order(field: &str, direction: RuntimeOrderDirection) -> RuntimeOrderTerm {
    RuntimeOrderTerm {
        field: fid(field),
        direction,
    }
}

fn fixture_runtime_schema() -> RuntimeSchema {
    let customers = RuntimeCollection {
        id: cid("col_customer"),
        api_type_name: "Customer".to_string(),
        api_plural_name: "Customers".to_string(),
        physical_table: "customers".to_string(),
        primary_key: vec![fid("fld_customer_id")],
        append_only: false,
        retention_purge: false,
        fields: vec![
            RuntimeField {
                generated: true,
                ..field("fld_customer_id", "id", "id", RuntimeValueKind::Uuid)
            },
            RuntimeField {
                filterable: true,
                sortable: true,
                ..field(
                    "fld_customer_name",
                    "name",
                    "name",
                    RuntimeValueKind::String,
                )
            },
            RuntimeField {
                unique: true,
                filterable: true,
                ..field(
                    "fld_customer_email",
                    "email",
                    "email",
                    RuntimeValueKind::String,
                )
            },
            RuntimeField {
                filterable: true,
                sortable: true,
                default: Some(RuntimeDefault::Literal("0".to_string())),
                ..field(
                    "fld_customer_loyalty_points",
                    "loyaltyPoints",
                    "loyalty_points",
                    RuntimeValueKind::Integer,
                )
            },
            RuntimeField {
                nullable: true,
                ..field(
                    "fld_customer_balance",
                    "balance",
                    "balance",
                    RuntimeValueKind::Float,
                )
            },
            RuntimeField {
                filterable: true,
                default: Some(RuntimeDefault::Literal("true".to_string())),
                ..field(
                    "fld_customer_is_active",
                    "isActive",
                    "is_active",
                    RuntimeValueKind::Boolean,
                )
            },
            RuntimeField {
                nullable: true,
                ..field(
                    "fld_customer_preferences",
                    "preferences",
                    "preferences",
                    RuntimeValueKind::Json,
                )
            },
            RuntimeField {
                nullable: true,
                ..field(
                    "fld_customer_avatar",
                    "avatar",
                    "avatar",
                    RuntimeValueKind::Bytes,
                )
            },
            RuntimeField {
                sortable: true,
                default: Some(RuntimeDefault::CurrentTimestamp),
                ..field(
                    "fld_customer_created_at",
                    "createdAt",
                    "created_at",
                    RuntimeValueKind::DateTime,
                )
            },
        ],
        relations: vec![
            relation(
                "rel_customer_contact_details",
                "contactDetails",
                "col_contact_detail",
                &[("fld_customer_id", "fld_contact_customer_id")],
                RelationCardinality::Many,
                None,
            ),
            relation(
                "rel_customer_notes",
                "notes",
                "col_customer_note",
                &[("fld_customer_id", "fld_note_customer_id")],
                RelationCardinality::Many,
                None,
            ),
        ],
        indexes: vec![index(
            "idx_customer_name",
            "idx_customers_name",
            &["fld_customer_name"],
            false,
        )],
        composite_unique: vec![],
        default_order: vec![order("fld_customer_created_at", RuntimeOrderDirection::Asc)],
    };

    let contact_details = RuntimeCollection {
        id: cid("col_contact_detail"),
        api_type_name: "ContactDetail".to_string(),
        api_plural_name: "ContactDetails".to_string(),
        physical_table: "contact_details".to_string(),
        primary_key: vec![fid("fld_contact_id")],
        append_only: false,
        retention_purge: false,
        fields: vec![
            RuntimeField {
                generated: true,
                ..field("fld_contact_id", "id", "id", RuntimeValueKind::Uuid)
            },
            RuntimeField {
                filterable: true,
                ..field(
                    "fld_contact_customer_id",
                    "customerId",
                    "customer_id",
                    RuntimeValueKind::Uuid,
                )
            },
            RuntimeField {
                filterable: true,
                ..field("fld_contact_kind", "kind", "kind", RuntimeValueKind::String)
            },
            field(
                "fld_contact_value",
                "value",
                "value",
                RuntimeValueKind::String,
            ),
        ],
        relations: vec![relation(
            "rel_contact_customer",
            "customer",
            "col_customer",
            &[("fld_contact_customer_id", "fld_customer_id")],
            RelationCardinality::One,
            Some(DeletePolicy::Cascade),
        )],
        indexes: vec![index(
            "idx_contact_customer",
            "idx_contact_details_customer_id",
            &["fld_contact_customer_id"],
            false,
        )],
        composite_unique: vec![vec![
            fid("fld_contact_customer_id"),
            fid("fld_contact_kind"),
        ]],
        default_order: vec![order("fld_contact_id", RuntimeOrderDirection::Asc)],
    };

    let customer_notes = RuntimeCollection {
        id: cid("col_customer_note"),
        api_type_name: "CustomerNote".to_string(),
        api_plural_name: "CustomerNotes".to_string(),
        physical_table: "customer_notes".to_string(),
        primary_key: vec![fid("fld_note_customer_id"), fid("fld_note_seq")],
        append_only: false,
        retention_purge: false,
        fields: vec![
            RuntimeField {
                filterable: true,
                ..field(
                    "fld_note_customer_id",
                    "customerId",
                    "customer_id",
                    RuntimeValueKind::Uuid,
                )
            },
            RuntimeField {
                sortable: true,
                ..field("fld_note_seq", "seq", "seq", RuntimeValueKind::Integer)
            },
            field("fld_note_body", "body", "body", RuntimeValueKind::String),
        ],
        relations: vec![
            relation(
                "rel_note_customer",
                "customer",
                "col_customer",
                &[("fld_note_customer_id", "fld_customer_id")],
                RelationCardinality::One,
                Some(DeletePolicy::Cascade),
            ),
            relation(
                "rel_note_attachments",
                "attachments",
                "col_note_attachment",
                &[
                    ("fld_note_customer_id", "fld_attach_customer_id"),
                    ("fld_note_seq", "fld_attach_note_seq"),
                ],
                RelationCardinality::Many,
                None,
            ),
        ],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![order("fld_note_seq", RuntimeOrderDirection::Asc)],
    };

    let note_attachments = RuntimeCollection {
        id: cid("col_note_attachment"),
        api_type_name: "NoteAttachment".to_string(),
        api_plural_name: "NoteAttachments".to_string(),
        physical_table: "note_attachments".to_string(),
        primary_key: vec![fid("fld_attach_id")],
        append_only: false,
        retention_purge: false,
        fields: vec![
            RuntimeField {
                generated: true,
                ..field("fld_attach_id", "id", "id", RuntimeValueKind::Uuid)
            },
            field(
                "fld_attach_customer_id",
                "customerId",
                "customer_id",
                RuntimeValueKind::Uuid,
            ),
            field(
                "fld_attach_note_seq",
                "noteSeq",
                "note_seq",
                RuntimeValueKind::Integer,
            ),
            field(
                "fld_attach_file_name",
                "fileName",
                "file_name",
                RuntimeValueKind::String,
            ),
        ],
        relations: vec![relation(
            "rel_attach_note",
            "note",
            "col_customer_note",
            &[
                ("fld_attach_customer_id", "fld_note_customer_id"),
                ("fld_attach_note_seq", "fld_note_seq"),
            ],
            RelationCardinality::One,
            Some(DeletePolicy::Restrict),
        )],
        indexes: vec![index(
            "idx_attach_note",
            "idx_note_attachments_customer_id_note_seq",
            &["fld_attach_customer_id", "fld_attach_note_seq"],
            false,
        )],
        composite_unique: vec![],
        default_order: vec![order("fld_attach_id", RuntimeOrderDirection::Asc)],
    };

    RuntimeSchema {
        format_version: graphql_orm::graphql::orm::RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![customers, contact_details, customer_notes, note_attachments],
    }
}

// ---------------------------------------------------------------------------
// Equivalence and determinism.
// ---------------------------------------------------------------------------

#[test]
fn static_and_runtime_definitions_agree_on_the_structural_fingerprint() {
    let converted = RuntimeSchema::from_static_entities(&static_metadata())
        .expect("static conversion succeeds")
        .validate()
        .expect("converted schema is valid");
    let hand_built = fixture_runtime_schema()
        .validate()
        .expect("hand-built schema is valid");

    assert_eq!(
        converted.structural_fingerprint(),
        hand_built.structural_fingerprint(),
        "static-derived and catalog-style schemas must share one structural fingerprint:\n\
         converted:\n{:?}\nhand-built:\n{:?}",
        String::from_utf8_lossy(&converted.canonical_bytes()),
        String::from_utf8_lossy(&hand_built.canonical_bytes()),
    );

    // Stable IDs differ (synthesized vs catalog-assigned), so the full fingerprints must not
    // be conflated with the logical contract.
    assert_ne!(converted.fingerprint(), hand_built.fingerprint());
}

#[test]
fn static_conversion_is_deterministic() {
    let first = RuntimeSchema::from_static_entities(&static_metadata()).expect("first conversion");
    let second =
        RuntimeSchema::from_static_entities(&static_metadata()).expect("second conversion");
    assert_eq!(first, second);
    assert_eq!(
        first.validate().expect("valid").fingerprint(),
        second.validate().expect("valid").fingerprint()
    );
}

#[test]
fn canonical_bytes_are_independent_of_declaration_order() {
    let ordered = fixture_runtime_schema();
    let mut shuffled = ordered.clone();
    shuffled.collections.reverse();
    for collection in &mut shuffled.collections {
        collection.fields.reverse();
        collection.relations.reverse();
        collection.indexes.reverse();
    }

    let ordered = ordered.validate().expect("ordered is valid");
    let shuffled = shuffled.validate().expect("shuffled is valid");
    assert_eq!(ordered.canonical_bytes(), shuffled.canonical_bytes());
    assert_eq!(ordered.fingerprint(), shuffled.fingerprint());
    assert_eq!(
        ordered.structural_fingerprint(),
        shuffled.structural_fingerprint()
    );
}

#[test]
fn retention_capability_is_validated_and_fingerprinted() {
    let mut append_only = fixture_runtime_schema();
    append_only.collections[0].append_only = true;
    let append_only = append_only.validate().expect("append-only schema is valid");

    let mut retained = fixture_runtime_schema();
    retained.collections[0].append_only = true;
    retained.collections[0].retention_purge = true;
    let retained = retained.validate().expect("retention schema is valid");

    assert_ne!(append_only.fingerprint(), retained.fingerprint());
    assert_ne!(
        append_only.structural_fingerprint(),
        retained.structural_fingerprint()
    );
    let canonical = String::from_utf8(retained.canonical_bytes()).expect("canonical utf8");
    assert!(canonical.contains("|retention_purge=true"));

    let mut invalid = fixture_runtime_schema();
    invalid.collections[0].retention_purge = true;
    assert!(
        diagnostic_codes(invalid)
            .contains(&RuntimeSchemaDiagnosticCode::RetentionRequiresAppendOnly)
    );
}

#[test]
fn owned_schema_survives_source_value_drop_and_serde_round_trip() {
    let validated = {
        // Build from short-lived heap strings; nothing borrowed may escape this scope.
        let source_table = String::from("things");
        let mut schema = RuntimeSchema::default();
        schema.collections.push(RuntimeCollection {
            id: CollectionId::new(source_table.clone()).expect("id"),
            api_type_name: "Thing".to_string(),
            api_plural_name: "Things".to_string(),
            physical_table: source_table.clone(),
            primary_key: vec![fid("things.id")],
            append_only: false,
            retention_purge: false,
            fields: vec![field("things.id", "id", "id", RuntimeValueKind::Uuid)],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
        });
        drop(source_table);
        schema.validate().expect("valid")
    };
    assert_eq!(validated.schema().collections[0].physical_table, "things");

    let json = serde_json::to_string(validated.schema()).expect("serializes");
    let round_tripped: RuntimeSchema = serde_json::from_str(&json).expect("deserializes");
    let round_tripped = round_tripped.validate().expect("still valid");
    assert_eq!(round_tripped.fingerprint(), validated.fingerprint());
    assert_eq!(
        round_tripped.structural_fingerprint(),
        validated.structural_fingerprint()
    );
}

// ---------------------------------------------------------------------------
// Diagnostics.
// ---------------------------------------------------------------------------

fn diagnostic_codes(schema: RuntimeSchema) -> Vec<RuntimeSchemaDiagnosticCode> {
    schema
        .validate()
        .expect_err("schema must be invalid")
        .diagnostics()
        .iter()
        .map(|d| d.code)
        .collect()
}

#[test]
fn validation_reports_all_diagnostics_not_just_the_first() {
    let mut schema = fixture_runtime_schema();
    // Nullable primary key in one collection, dangling relation target in another.
    schema.collections[0].fields[0].nullable = true;
    schema.collections[3].relations[0].target = cid("col_missing");
    let codes = diagnostic_codes(schema);
    assert!(codes.contains(&RuntimeSchemaDiagnosticCode::NullablePrimaryKeyField));
    assert!(codes.contains(&RuntimeSchemaDiagnosticCode::UnknownRelationTarget));
}

#[test]
fn duplicate_ids_and_names_are_rejected() {
    let mut schema = fixture_runtime_schema();
    let duplicate = schema.collections[0].clone();
    schema.collections.push(duplicate);
    let codes = diagnostic_codes(schema);
    assert!(codes.contains(&RuntimeSchemaDiagnosticCode::DuplicateStableId));
    assert!(codes.contains(&RuntimeSchemaDiagnosticCode::DuplicateApiName));
    assert!(codes.contains(&RuntimeSchemaDiagnosticCode::DuplicatePhysicalName));

    let mut schema = fixture_runtime_schema();
    schema.collections[0].fields[1].api_name = "Email".to_string();
    schema.collections[0].fields[2].api_name = "email".to_string();
    // Case-folded collision between `Email` and `email`.
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::DuplicateApiName));
}

#[test]
fn invalid_and_reserved_names_are_rejected() {
    let mut schema = fixture_runtime_schema();
    schema.collections[0].fields[1].api_name = "9starts_with_digit".to_string();
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::InvalidApiName));

    let mut schema = fixture_runtime_schema();
    schema.collections[0].fields[1].api_name = "__reserved".to_string();
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::ReservedApiName));

    let mut schema = fixture_runtime_schema();
    schema.collections[0].physical_table = "Customers".to_string();
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::InvalidPhysicalName));

    let mut schema = fixture_runtime_schema();
    schema.collections[0].physical_table = "x".repeat(64);
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::InvalidPhysicalName));
}

#[test]
fn key_and_reference_integrity_is_enforced() {
    // Key pair mixing uuid and integer.
    let mut schema = fixture_runtime_schema();
    schema.collections[1].relations[0].key_pairs[0].target = fid("fld_customer_loyalty_points");
    assert!(
        diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::RelationKeyTypeMismatch)
    );

    // Unknown source field in a relation key.
    let mut schema = fixture_runtime_schema();
    schema.collections[1].relations[0].key_pairs[0].source = fid("fld_missing");
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::UnknownFieldReference));

    // Empty primary key and unknown index field.
    let mut schema = fixture_runtime_schema();
    schema.collections[0].primary_key.clear();
    schema.collections[0].indexes[0].fields = vec![fid("fld_missing")];
    let codes = diagnostic_codes(schema);
    assert!(codes.contains(&RuntimeSchemaDiagnosticCode::EmptyPrimaryKey));
    assert!(codes.contains(&RuntimeSchemaDiagnosticCode::UnknownFieldReference));

    // Composite unique group with a single field.
    let mut schema = fixture_runtime_schema();
    schema.collections[1].composite_unique = vec![vec![fid("fld_contact_kind")]];
    assert!(
        diagnostic_codes(schema)
            .contains(&RuntimeSchemaDiagnosticCode::CompositeUniqueGroupTooSmall)
    );
}

#[test]
fn delete_behavior_rules_are_enforced() {
    // Foreign-key side without delete behavior.
    let mut schema = fixture_runtime_schema();
    schema.collections[1].relations[0].on_delete = None;
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::MissingDeleteBehavior));

    // Delete behavior on the non-enforcing side.
    let mut schema = fixture_runtime_schema();
    schema.collections[0].relations[0].on_delete = Some(DeletePolicy::Cascade);
    assert!(
        diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::UnexpectedDeleteBehavior)
    );

    // Set-null delete behavior over a non-nullable key field.
    let mut schema = fixture_runtime_schema();
    schema.collections[1].relations[0].on_delete = Some(DeletePolicy::SetNull);
    assert!(
        diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::SetNullOnNonNullableKey)
    );
}

#[test]
fn stable_ids_reject_invalid_values() {
    assert!(CollectionId::new("").is_err());
    assert!(FieldId::new("has space").is_err());
    assert!(RelationId::new("pipe|char").is_err());
    assert!(IndexId::new("x".repeat(200)).is_err());
    assert!(CollectionId::new("col_customer").is_ok());
}

#[test]
fn unsupported_static_capabilities_are_reported_not_dropped() {
    // A spatial column must not silently vanish from the converted schema.
    use graphql_orm::graphql::orm::{ColumnDef, EntityMetadata};

    let mut entity = Customer::metadata().clone();
    let mut fields: Vec<_> = entity.fields.to_vec();
    let spatial_source = ColumnDef::new("boundary", "TEXT");
    let mut spatial_field = graphql_orm::graphql::orm::FieldMetadata::from(&spatial_source);
    spatial_field.spatial = Some(graphql_orm::graphql::orm::SpatialColumnDef::geometry(
        graphql_orm::graphql::orm::SpatialGeometryType::Polygon,
        4326,
    ));
    fields.push(spatial_field);
    entity.fields = fields.into_boxed_slice();

    let others = [
        ContactDetail::metadata(),
        CustomerNote::metadata(),
        NoteAttachment::metadata(),
    ];
    let all: Vec<&EntityMetadata> = std::iter::once(&entity)
        .chain(others.iter().copied())
        .collect();
    let diagnostics =
        RuntimeSchema::from_static_entities(&all).expect_err("conversion must report spatial");
    assert!(
        diagnostics
            .diagnostics()
            .iter()
            .any(|d| d.code == RuntimeSchemaDiagnosticCode::UnsupportedCapability)
    );
}

// ---------------------------------------------------------------------------
// Review-hardening coverage: serde, canonical ambiguity, duplicates, FK proof,
// defaults, and fail-closed conversion.
// ---------------------------------------------------------------------------

#[test]
fn serde_cannot_bypass_stable_id_validation() {
    let bad_id = serde_json::json!({
        "collections": [{
            "id": "has space",
            "api_type_name": "Thing",
            "api_plural_name": "Things",
            "physical_table": "things",
            "primary_key": [],
            "append_only": false,
            "fields": [],
            "relations": [],
            "indexes": [],
            "composite_unique": [],
            "default_order": []
        }]
    });
    let err = serde_json::from_value::<RuntimeSchema>(bad_id).expect_err("invalid ID rejected");
    assert!(err.to_string().contains("stable ID"));
}

#[test]
fn legacy_runtime_schema_json_defaults_retention_to_disabled() {
    let mut value = serde_json::to_value(fixture_runtime_schema()).expect("serializes");
    for collection in value["collections"]
        .as_array_mut()
        .expect("collections array")
    {
        collection
            .as_object_mut()
            .expect("collection object")
            .remove("retention_purge");
    }

    let decoded: RuntimeSchema = serde_json::from_value(value).expect("legacy JSON deserializes");
    assert!(
        decoded
            .collections
            .iter()
            .all(|collection| !collection.retention_purge)
    );
}

#[test]
fn serde_rejects_unknown_fields() {
    let unknown = serde_json::json!({
        "collections": [],
        "extra_property": true
    });
    assert!(serde_json::from_value::<RuntimeSchema>(unknown).is_err());

    let unknown_field_prop = serde_json::json!({
        "collections": [{
            "id": "col_a",
            "api_type_name": "Thing",
            "api_plural_name": "Things",
            "physical_table": "things",
            "primary_key": [],
            "append_only": false,
            "fields": [{
                "id": "fld_a",
                "api_name": "a",
                "physical_column": "a",
                "value_kind": "string",
                "nullable": false,
                "unique": false,
                "filterable": false,
                "sortable": false,
                "generated": false,
                "default": null,
                "typoed_extra": 1
            }],
            "relations": [],
            "indexes": [],
            "composite_unique": [],
            "default_order": []
        }]
    });
    assert!(serde_json::from_value::<RuntimeSchema>(unknown_field_prop).is_err());
}

#[test]
fn hostile_literal_defaults_cannot_collide_canonical_bytes() {
    let build = |default_value: &str| {
        let mut schema = fixture_runtime_schema();
        schema.collections[0].fields[1].default =
            Some(RuntimeDefault::Literal(default_value.to_string()));
        schema.validate().expect("valid")
    };
    // Injected delimiter/newline text that would forge a field line if rendered unescaped.
    let plain = build("5");
    let hostile = build("5\"|unique=true\n  field|api=zzz");
    assert_ne!(plain.fingerprint(), hostile.fingerprint());
    assert_ne!(
        plain.structural_fingerprint(),
        hostile.structural_fingerprint()
    );
    let rendered = String::from_utf8(hostile.canonical_bytes()).expect("utf8");
    assert!(
        !rendered.contains("5\"|unique=true\n"),
        "literal must be escaped in canonical output"
    );
}

#[test]
fn duplicate_stable_ids_are_rejected_across_collections() {
    // Field ID reused in another collection.
    let mut schema = fixture_runtime_schema();
    schema.collections[3].fields[3].id = fid("fld_contact_value");
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::DuplicateStableId));

    // Relation ID reused.
    let mut schema = fixture_runtime_schema();
    schema.collections[3].relations[0].id =
        RelationId::new("rel_contact_customer").expect("valid id");
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::DuplicateStableId));

    // Index ID reused.
    let mut schema = fixture_runtime_schema();
    schema.collections[3].indexes[0].id = IndexId::new("idx_customer_name").expect("valid id");
    let codes = diagnostic_codes(schema);
    assert!(codes.contains(&RuntimeSchemaDiagnosticCode::DuplicateStableId));
}

#[test]
fn duplicate_key_members_are_rejected() {
    let mut schema = fixture_runtime_schema();
    let dup = schema.collections[2].primary_key[0].clone();
    schema.collections[2].primary_key.push(dup);
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::DuplicateKeyMember));

    let mut schema = fixture_runtime_schema();
    let dup = schema.collections[0].indexes[0].fields[0].clone();
    schema.collections[0].indexes[0].fields.push(dup);
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::DuplicateKeyMember));

    let mut schema = fixture_runtime_schema();
    schema.collections[1].composite_unique =
        vec![vec![fid("fld_contact_kind"), fid("fld_contact_kind")]];
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::DuplicateKeyMember));

    let mut schema = fixture_runtime_schema();
    let dup = schema.collections[3].relations[0].key_pairs[0].clone();
    schema.collections[3].relations[0].key_pairs.push(dup);
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::DuplicateKeyMember));
}

#[test]
fn relation_api_names_reject_the_reserved_prefix() {
    let mut schema = fixture_runtime_schema();
    schema.collections[1].relations[0].api_name = "__customer".to_string();
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::ReservedApiName));
}

#[test]
fn literal_defaults_must_match_their_value_kind() {
    let mut schema = fixture_runtime_schema();
    // loyaltyPoints is an integer field.
    schema.collections[0].fields[3].default =
        Some(RuntimeDefault::Literal("not-a-number".to_string()));
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::InvalidDefault));

    let mut schema = fixture_runtime_schema();
    // name is a string field; current_timestamp only fits datetime/integer.
    schema.collections[0].fields[1].default = Some(RuntimeDefault::CurrentTimestamp);
    assert!(diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::InvalidDefault));
}

#[test]
fn foreign_keys_require_a_provably_unique_target_key() {
    // Retarget the contact->customer key to a non-unique string field of the same kind.
    let mut schema = fixture_runtime_schema();
    schema.collections[1].relations[0].key_pairs[0] = RelationKeyPair {
        source: fid("fld_contact_kind"),
        target: fid("fld_customer_name"),
    };
    assert!(
        diagnostic_codes(schema).contains(&RuntimeSchemaDiagnosticCode::RelationTargetKeyNotUnique)
    );

    // A unique target field is acceptable.
    let mut schema = fixture_runtime_schema();
    schema.collections[1].relations[0].key_pairs[0] = RelationKeyPair {
        source: fid("fld_contact_kind"),
        target: fid("fld_customer_email"),
    };
    assert!(schema.validate().is_ok());
}

#[test]
fn conversion_fails_closed_on_policy_and_backup_semantics() {
    use graphql_orm::graphql::orm::{ColumnBackupPolicy, EntityMetadata, SchemaPolicy};

    let others = [
        ContactDetail::metadata(),
        CustomerNote::metadata(),
        NoteAttachment::metadata(),
    ];
    let convert_with = |mutate: &dyn Fn(&mut EntityMetadata)| {
        let mut entity = Customer::metadata().clone();
        mutate(&mut entity);
        let all: Vec<&EntityMetadata> = std::iter::once(&entity)
            .chain(others.iter().copied())
            .collect();
        RuntimeSchema::from_static_entities(&all)
    };

    for mutate in [
        (&|e: &mut EntityMetadata| e.read_policy = Some("owner_only")) as &dyn Fn(&mut _),
        &|e: &mut EntityMetadata| e.write_policy = Some("admin_only"),
        &|e: &mut EntityMetadata| e.backup_enabled = false,
        &|e: &mut EntityMetadata| e.backup_export_order = Some(3),
        &|e: &mut EntityMetadata| e.schema_policy = Some(SchemaPolicy::ExternalReadOnly),
        &|e: &mut EntityMetadata| {
            let mut fields = e.fields.to_vec();
            fields[1].backup_policy = ColumnBackupPolicy::Redact;
            e.fields = fields.into_boxed_slice();
        },
    ] {
        let diagnostics = convert_with(mutate).expect_err("must fail closed");
        assert!(
            diagnostics
                .diagnostics()
                .iter()
                .any(|d| d.code == RuntimeSchemaDiagnosticCode::UnsupportedCapability),
            "expected UnsupportedCapability, got: {diagnostics}"
        );
    }

    // Managed schema policy converts cleanly.
    assert!(
        convert_with(&|e: &mut EntityMetadata| e.schema_policy = Some(SchemaPolicy::Managed))
            .is_ok()
    );
}

#[test]
fn legacy_identifier_shapes_fail_validation_not_silently_convert() {
    // MSSQL-style schema-qualified table and PascalCase column names convert (IDs are valid
    // opaque tokens) but must fail physical-name validation rather than silently normalize.
    let mut entity = Customer::metadata().clone();
    entity.entity_name = "Job";
    entity.table_name = "dbo.Jobs";
    entity.plural_name = "Jobs";
    let mut fields = entity.fields.to_vec();
    fields[0].name = "JobId";
    entity.fields = fields.into_boxed_slice();
    entity.relations = Box::new([]);
    entity.indexes = Box::new([]);
    entity.primary_keys = Box::new(["JobId"]);
    entity.primary_key = "JobId";
    entity.default_sort = "JobId ASC";

    let schema = RuntimeSchema::from_static_entities(&[&entity]).expect("conversion itself works");
    let codes: Vec<_> = schema
        .validate()
        .expect_err("legacy identifiers are not portable")
        .diagnostics()
        .iter()
        .map(|d| d.code)
        .collect();
    assert!(codes.contains(&RuntimeSchemaDiagnosticCode::InvalidPhysicalName));
}

#[test]
fn renamed_fields_convert_with_their_public_api_name() {
    #[derive(
        GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
    )]
    #[graphql_entity(
        table = "renamed_things",
        plural = "RenamedThings",
        default_sort = "id"
    )]
    pub struct RenamedThing {
        #[primary_key]
        pub id: graphql_orm::uuid::Uuid,

        #[graphql(name = "customName")]
        pub internal_name: String,
    }

    let schema =
        RuntimeSchema::from_static_entities(&[RenamedThing::metadata()]).expect("converts");
    let field = schema.collections[0]
        .fields
        .iter()
        .find(|f| f.physical_column == "internal_name")
        .expect("field present");
    assert_eq!(field.api_name, "customName");
}
