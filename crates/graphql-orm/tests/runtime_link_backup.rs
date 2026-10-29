//! Backup and restore of composite-key tables: the ORM-owned link tables of many-to-many runtime
//! relations, including a shared inverse and a self link. Each PostgreSQL case owns its server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use graphql_orm::{graphql::orm::*, prelude::*};
use std::collections::BTreeMap;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

fn cid(s: &str) -> CollectionId {
    CollectionId::new(s).unwrap()
}
fn fid(s: &str) -> FieldId {
    FieldId::new(s).unwrap()
}
fn field(id: &str, name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: fid(id),
        api_name: name.into(),
        physical_column: name.to_lowercase(),
        value_kind: kind,
        nullable: false,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}
fn collection(id: &str, name: &str) -> RuntimeCollection {
    RuntimeCollection {
        description: None,
        classification: None,
        id: cid(id),
        api_type_name: name.into(),
        api_plural_name: format!("{name}s"),
        physical_table: format!("lb_{id}"),
        primary_key: vec![fid(&format!("{id}.id"))],
        append_only: false,
        retention_purge: false,
        fields: vec![
            field(&format!("{id}.id"), "id", RuntimeValueKind::Integer),
            field(&format!("{id}.label"), "label", RuntimeValueKind::String),
        ],
        relations: vec![],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
        label_fields: Vec::new(),
    }
}
fn join(
    id: &str,
    target: &str,
    table: &str,
    source: (&str, &str),
    to: (&str, &str),
) -> RuntimeRelation {
    RuntimeRelation {
        description: None,
        classification: None,
        export: None,
        id: RelationId::new(id).unwrap(),
        api_name: id.into(),
        target: cid(target),
        key_pairs: vec![],
        cardinality: RelationCardinality::Many,
        enforce_foreign_key: false,
        on_delete: None,
        join: Some(RuntimeJoinTable {
            physical_table: table.into(),
            source: vec![RuntimeJoinColumn {
                field: fid(source.0),
                column: source.1.into(),
            }],
            target: vec![RuntimeJoinColumn {
                field: fid(to.0),
                column: to.1.into(),
            }],
        }),
    }
}
/// Posts and tags linked both ways through `lb_post_tags`; nodes link to nodes.
fn definition() -> ValidatedRuntimeSchema {
    ordered(false)
}
/// The same schema, optionally with its collections in reverse order.
fn ordered(reversed: bool) -> ValidatedRuntimeSchema {
    let mut post = collection("post", "Post");
    let mut tag = collection("tag", "Tag");
    let mut node = collection("node", "Node");
    post.relations.push(join(
        "tags",
        "tag",
        "lb_post_tags",
        ("post.id", "post_id"),
        ("tag.id", "tag_id"),
    ));
    tag.relations.push(join(
        "posts",
        "post",
        "lb_post_tags",
        ("tag.id", "tag_id"),
        ("post.id", "post_id"),
    ));
    node.relations.push(join(
        "links",
        "node",
        "lb_node_links",
        ("node.id", "from_id"),
        ("node.id", "to_id"),
    ));
    let mut collections = vec![post, tag, node];
    if reversed {
        collections.reverse();
    }
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections,
    }
    .validate()
    .unwrap()
}
/// Host-built descriptors of the endpoint collections, as an application host writes them.
fn endpoint(c: &RuntimeCollection) -> EntityBackupDescriptor {
    EntityBackupDescriptor {
        entity_name: c.api_type_name.clone(),
        table_name: c.physical_table.clone(),
        primary_key_column: "id".into(),
        primary_key_columns: Vec::new(),
        retention_purge: false,
        export_order: 10,
        restore_order: 10,
        columns: [
            ("id", BackupValueKind::Integer, true),
            ("label", BackupValueKind::String, false),
        ]
        .into_iter()
        .map(|(name, kind, key)| ColumnBackupDescriptor {
            column_name: name.into(),
            rust_field_name: name.into(),
            logical_type: kind,
            decimal: None,
            nullable: false,
            is_primary_key: key,
            is_generated: false,
            backup_policy: ColumnBackupPolicy::Include,
        })
        .collect(),
        dependencies: vec![],
    }
}
fn descriptors(schema: &ValidatedRuntimeSchema) -> Vec<EntityBackupDescriptor> {
    let mut all = schema
        .schema()
        .collections
        .iter()
        .map(endpoint)
        .collect::<Vec<_>>();
    all.extend(schema.join_table_backup_descriptors(|_| Some(10)).unwrap());
    all
}
fn snapshot(entities: Vec<EntityBackupDescriptor>) -> GraphqlOrmSchemaSnapshot {
    GraphqlOrmSchemaSnapshot {
        backend: format!("{:?}", Backend::DIALECT),
        migration_version: "links".into(),
        schema_hash: stable_schema_hash(&entities),
        entities,
    }
}

struct Fixture {
    db: Database<Backend>,
    #[cfg(feature = "sqlite")]
    path: std::path::PathBuf,
    #[cfg(feature = "postgres")]
    _owned: owned_postgres::OwnedPostgres,
}
async fn fixture(schema: &ValidatedRuntimeSchema, seed: &[&str]) -> Fixture {
    #[cfg(feature = "sqlite")]
    let path = std::env::temp_dir().join(format!(
        "graphql-orm-link-backup-{}.sqlite3",
        graphql_orm::uuid::Uuid::new_v4().simple()
    ));
    #[cfg(feature = "sqlite")]
    let db = Database::<Backend>::connect_sqlite(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    #[cfg(feature = "postgres")]
    let owned = owned_postgres::OwnedPostgres::start("link-backup").unwrap();
    #[cfg(feature = "postgres")]
    let db = Database::<Backend>::connect_postgres(&owned.url)
        .await
        .unwrap();
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new(schema.owned_table_names()).unwrap();
    let plan = db
        .schema()
        .plan_owned_migration("init", "links", &target, &ownership, PlanOptions::strict())
        .await
        .unwrap();
    db.schema()
        .apply_owned_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    for sql in seed {
        Backend::fetch_rows(db.pool(), sql, &[]).await.unwrap();
    }
    Fixture {
        db,
        #[cfg(feature = "sqlite")]
        path,
        #[cfg(feature = "postgres")]
        _owned: owned,
    }
}
#[cfg(feature = "sqlite")]
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
async fn count(db: &Database<Backend>, table: &str) -> i64 {
    let rows = Backend::fetch_rows(
        db.pool(),
        &format!("SELECT COUNT(*) AS n FROM {table}"),
        &[],
    )
    .await
    .unwrap();
    use graphql_orm::sqlx::Row;
    rows[0].try_get::<i64, _>(0).unwrap()
}
async fn pairs(db: &Database<Backend>, sql: &str) -> Vec<(i64, i64)> {
    let rows = Backend::fetch_rows(db.pool(), sql, &[]).await.unwrap();
    rows.iter()
        .map(|row| {
            use graphql_orm::sqlx::Row;
            (
                row.try_get::<i64, _>(0).unwrap(),
                row.try_get::<i64, _>(1).unwrap(),
            )
        })
        .collect()
}

const SEED: &[&str] = &[
    "INSERT INTO lb_post (id, label) VALUES (1, 'one'), (2, 'two')",
    "INSERT INTO lb_tag (id, label) VALUES (1, 'x'), (2, 'y')",
    "INSERT INTO lb_node (id, label) VALUES (1, 'a'), (2, 'b')",
    "INSERT INTO lb_post_tags (post_id, tag_id) VALUES (1, 1), (1, 2), (2, 1)",
    "INSERT INTO lb_node_links (from_id, to_id) VALUES (1, 2), (2, 1), (1, 1)",
];

#[test]
fn link_tables_have_composite_keys_cascading_dependencies_and_later_orders() {
    let schema = definition();
    let links = schema
        .join_table_backup_descriptors(|id| Some(if id.as_str() == "tag" { 30 } else { 20 }))
        .unwrap();
    // One descriptor per table: the inverse `posts` relation shares `lb_post_tags`.
    assert_eq!(
        links
            .iter()
            .map(|d| d.table_name.as_str())
            .collect::<Vec<_>>(),
        ["lb_node_links", "lb_post_tags"]
    );
    let post_tags = &links[1];
    assert_eq!(post_tags.key_columns(), ["post_id", "tag_id"]);
    assert_eq!(post_tags.primary_key_column, "post_id");
    assert_eq!((post_tags.export_order, post_tags.restore_order), (31, 31));
    assert!(
        post_tags
            .columns
            .iter()
            .all(|c| c.is_primary_key && !c.nullable)
    );
    assert_eq!(
        post_tags
            .dependencies
            .iter()
            .map(|d| (
                d.table_name.as_str(),
                d.source_column.as_str(),
                d.target_column.as_str(),
                &d.on_delete
            ))
            .collect::<Vec<_>>(),
        [
            ("lb_post", "post_id", "id", &DeletePolicy::Cascade),
            ("lb_tag", "tag_id", "id", &DeletePolicy::Cascade)
        ]
    );
    assert!(matches!(
        schema.join_table_backup_descriptors(|id| (id.as_str() != "tag").then_some(1)),
        Err(RuntimeLinkBackupError::UnorderedEndpoint(id)) if id.as_str() == "tag"
    ));
    // Single-column descriptors keep their wire shape and schema hash input.
    let single = endpoint(&schema.schema().collections[0]);
    let value = serde_json::to_value(&single).unwrap();
    assert!(value.get("primary_key_columns").is_none());
    let mut legacy = value.clone();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("primary_key_columns");
    assert_eq!(
        serde_json::from_value::<EntityBackupDescriptor>(legacy).unwrap(),
        single
    );
    assert_ne!(
        stable_schema_hash(std::slice::from_ref(post_tags)),
        stable_schema_hash(&[EntityBackupDescriptor {
            primary_key_columns: Vec::new(),
            ..post_tags.clone()
        }])
    );
}

#[tokio::test]
async fn link_tables_round_trip_and_orphan_links_fail_the_restore() {
    let schema = definition();
    let source = fixture(&schema, SEED).await;
    let entities = descriptors(&schema);
    let mut snap = source.db.begin_consistent_snapshot().await.unwrap();
    let mut rows = BTreeMap::new();
    for entity in &entities {
        rows.insert(
            entity.table_name.clone(),
            source
                .db
                .export_table_rows(&mut snap, entity)
                .await
                .unwrap(),
        );
    }
    drop(snap);
    // Composite identities are canonical, distinct and in key order.
    assert_eq!(
        rows["lb_post_tags"]
            .iter()
            .map(|r| r.primary_key.as_str())
            .collect::<Vec<_>>(),
        [r#"["1","1"]"#, r#"["1","2"]"#, r#"["2","1"]"#]
    );
    assert_eq!(
        rows["lb_node_links"]
            .iter()
            .map(|r| r.primary_key.as_str())
            .collect::<Vec<_>>(),
        [r#"["1","1"]"#, r#"["1","2"]"#, r#"["2","1"]"#]
    );
    // Single-key identities are unchanged.
    assert_eq!(rows["lb_post"][0].primary_key, "1");

    let restored = fixture(&schema, &[]).await;
    let snapshot = snapshot(entities.clone());
    let reports = restored
        .db
        .restore_backup_rows(
            &snapshot,
            &snapshot,
            &rows,
            &RestoreContext::empty_database(),
        )
        .await
        .unwrap();
    assert!(
        reports
            .iter()
            .all(|r| r.rows_imported == rows[&r.table_name].len())
    );
    assert_eq!(
        pairs(
            &restored.db,
            "SELECT post_id, tag_id FROM lb_post_tags ORDER BY post_id, tag_id"
        )
        .await,
        [(1, 1), (1, 2), (2, 1)]
    );
    assert_eq!(
        pairs(
            &restored.db,
            "SELECT from_id, to_id FROM lb_node_links ORDER BY from_id, to_id"
        )
        .await,
        [(1, 1), (1, 2), (2, 1)]
    );

    // A link whose endpoint row is missing is refused before anything is written, also as a dry run.
    let mut orphaned = rows.clone();
    orphaned
        .get_mut("lb_post")
        .unwrap()
        .retain(|row| row.primary_key == "1");
    let target = fixture(&schema, &[]).await;
    for context in [RestoreContext::dry_run(), RestoreContext::empty_database()] {
        let error = target
            .db
            .restore_backup_rows(&snapshot, &snapshot, &orphaned, &context)
            .await
            .unwrap_err();
        let orphan = BackupOrphanReference::from_error(&error).expect("typed orphan refusal");
        assert_eq!(
            (
                orphan.table_name.as_str(),
                orphan.column.as_str(),
                orphan.target_table.as_str()
            ),
            ("lb_post_tags", "post_id", "lb_post"),
            "{error}"
        );
    }
    for table in [
        "lb_post",
        "lb_tag",
        "lb_node",
        "lb_post_tags",
        "lb_node_links",
    ] {
        assert_eq!(
            count(&target.db, table).await,
            0,
            "{table} after a refused restore"
        );
    }

    // A target that already holds rows is refused before anything is written, and unchanged.
    let occupied = fixture(
        &schema,
        &["INSERT INTO lb_tag (id, label) VALUES (9, 'kept')"],
    )
    .await;
    let error = occupied
        .db
        .restore_backup_rows(
            &snapshot,
            &snapshot,
            &rows,
            &RestoreContext::empty_database(),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("already contains 1 rows"),
        "{error}"
    );
    for (table, rows) in [
        ("lb_post", 0),
        ("lb_tag", 1),
        ("lb_node", 0),
        ("lb_post_tags", 0),
        ("lb_node_links", 0),
    ] {
        assert_eq!(count(&occupied.db, table).await, rows, "{table}");
    }
}

#[test]
fn the_shared_inverse_side_does_not_depend_on_collection_order() {
    let (forward, reversed) = (definition(), ordered(true));
    let descriptors = |schema: &ValidatedRuntimeSchema| {
        schema.join_table_backup_descriptors(|_| Some(1)).unwrap()
    };
    assert_eq!(descriptors(&forward), descriptors(&reversed));
    assert_eq!(
        stable_schema_hash(&descriptors(&forward)),
        stable_schema_hash(&descriptors(&reversed))
    );
}

/// SQLite admits NULL in a composite primary key; a null part would collide with the text "null".
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn a_null_composite_key_part_is_refused_at_export() {
    let schema = definition();
    let source = fixture(
        &schema,
        &[
            "CREATE TABLE lb_loose (a TEXT, b TEXT, PRIMARY KEY (a, b))",
            "INSERT INTO lb_loose (a, b) VALUES (NULL, 'x')",
        ],
    )
    .await;
    let column = |name: &str| ColumnBackupDescriptor {
        column_name: name.into(),
        rust_field_name: name.into(),
        logical_type: BackupValueKind::String,
        decimal: None,
        nullable: true,
        is_primary_key: true,
        is_generated: false,
        backup_policy: ColumnBackupPolicy::Include,
    };
    let loose = EntityBackupDescriptor {
        entity_name: "Loose".into(),
        table_name: "lb_loose".into(),
        primary_key_column: "a".into(),
        primary_key_columns: vec!["a".into(), "b".into()],
        retention_purge: false,
        export_order: 1,
        restore_order: 1,
        columns: vec![column("a"), column("b")],
        dependencies: vec![],
    };
    let mut snap = source.db.begin_consistent_snapshot().await.unwrap();
    let error = source
        .db
        .export_table_rows(&mut snap, &loose)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("null primary key part"),
        "{error}"
    );
}
