#![cfg(feature = "postgres")]

use std::process::Command;

use graphql_orm::db::Database;
use graphql_orm::graphql::orm::{
    CollectionId, DbAuthContext, FieldId, PostgresBackend, RelationCardinality, RelationId,
    RelationKeyPair, RuntimeCollection, RuntimeField, RuntimeFieldState, RuntimeOrderDirection,
    RuntimeOrderTerm, RuntimePageRequest, RuntimeQueryLimits, RuntimeRelation,
    RuntimeRelationLimits, RuntimeRelationSelection, RuntimeRelationValue, RuntimeSchema,
    RuntimeValueKind,
};

fn cid(value: &str) -> CollectionId {
    CollectionId::new(value).unwrap()
}

fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}

fn field(id: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        id: fid(id),
        api_name: id.to_string(),
        physical_column: id.to_string(),
        value_kind: kind,
        nullable: false,
        unique: id.ends_with("_id"),
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
    }
}

fn schema() -> graphql_orm::graphql::orm::ValidatedRuntimeSchema {
    let customer_id = field("pg_customer_id", RuntimeValueKind::Integer);
    let customer_tenant = field("pg_customer_tenant", RuntimeValueKind::String);
    let contact_id = field("pg_contact_id", RuntimeValueKind::Integer);
    let note_id = field("pg_note_id", RuntimeValueKind::Integer);
    RuntimeSchema {
        format_version: 1,
        collections: vec![
            RuntimeCollection {
                id: cid("pg_customers"),
                api_type_name: "PgCustomer".into(),
                api_plural_name: "PgCustomers".into(),
                physical_table: "runtime_relation_pg_customers".into(),
                primary_key: vec![customer_id.id.clone()],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    customer_id.clone(),
                    customer_tenant.clone(),
                    field("pg_customer_status", RuntimeValueKind::String),
                ],
                relations: vec![RuntimeRelation {
                    id: RelationId::new("pg_customer_contacts").unwrap(),
                    api_name: "contacts".into(),
                    target: cid("pg_contacts"),
                    key_pairs: vec![
                        RelationKeyPair {
                            source: customer_tenant.id.clone(),
                            target: fid("pg_contact_tenant"),
                        },
                        RelationKeyPair {
                            source: customer_id.id.clone(),
                            target: fid("pg_contact_owner_id"),
                        },
                    ],
                    cardinality: RelationCardinality::Many,
                    enforce_foreign_key: false,
                    on_delete: None,
                }],
                indexes: vec![],
                composite_unique: vec![],
                default_order: vec![RuntimeOrderTerm {
                    field: customer_id.id.clone(),
                    direction: RuntimeOrderDirection::Asc,
                }],
            },
            RuntimeCollection {
                id: cid("pg_contacts"),
                api_type_name: "PgContact".into(),
                api_plural_name: "PgContacts".into(),
                physical_table: "runtime_relation_pg_contacts".into(),
                primary_key: vec![contact_id.id.clone()],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    contact_id.clone(),
                    field("pg_contact_tenant", RuntimeValueKind::String),
                    field("pg_contact_owner_id", RuntimeValueKind::Integer),
                    field("pg_contact_label", RuntimeValueKind::String),
                    field("pg_contact_payload", RuntimeValueKind::Bytes),
                ],
                relations: vec![RuntimeRelation {
                    id: RelationId::new("pg_contact_notes").unwrap(),
                    api_name: "notes".into(),
                    target: cid("pg_notes"),
                    key_pairs: vec![RelationKeyPair {
                        source: contact_id.id.clone(),
                        target: fid("pg_note_contact_id"),
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
                id: cid("pg_notes"),
                api_type_name: "PgNote".into(),
                api_plural_name: "PgNotes".into(),
                physical_table: "runtime_relation_pg_notes".into(),
                primary_key: vec![note_id.id.clone()],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    note_id.clone(),
                    field("pg_note_contact_id", RuntimeValueKind::Integer),
                    field("pg_note_tenant", RuntimeValueKind::String),
                    field("pg_note_body", RuntimeValueKind::String),
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

struct OwnedPostgres {
    name: String,
    token: String,
    url: String,
    app_url: String,
    app_password: String,
}

impl Drop for OwnedPostgres {
    fn drop(&mut self) {
        let identity = Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{ index .Config.Labels \"graphql-orm.test-owner\" }}",
                &self.name,
            ])
            .output();
        if identity
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == self.token)
        {
            let _ = Command::new("docker")
                .args(["rm", "--force", &self.name])
                .output();
        }
    }
}

impl OwnedPostgres {
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
        let name = format!("graphql-orm-runtime-relation-{token}");
        let password = format!("relation_{token}");
        let app_password = format!("reader_{token}");
        let database = format!("relation_{token}");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--name",
                &name,
                "--label",
                &format!("graphql-orm.test-owner={token}"),
                "--publish",
                "127.0.0.1::5432",
                "--env",
                "POSTGRES_USER=relation_owner",
                "--env",
                &format!("POSTGRES_PASSWORD={password}"),
                "--env",
                &format!("POSTGRES_DB={database}"),
                "postgres:17-alpine",
            ])
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into());
        }
        let mut owned = Self {
            name,
            token,
            url: String::new(),
            app_url: String::new(),
            app_password,
        };
        for _ in 0..120 {
            let ready = Command::new("docker")
                .args(["exec", &owned.name, "pg_isready", "-U", "relation_owner"])
                .output()?;
            if ready.status.success() {
                let output = Command::new("docker")
                    .args(["port", &owned.name, "5432/tcp"])
                    .output()?;
                let published = String::from_utf8(output.stdout)?;
                let port = published
                    .lines()
                    .find_map(|line| line.strip_prefix("127.0.0.1:"))
                    .ok_or("owned PostgreSQL was not loopback-published")?;
                owned.url =
                    format!("postgres://relation_owner:{password}@127.0.0.1:{port}/{database}");
                owned.app_url = format!(
                    "postgres://relation_reader:{}@127.0.0.1:{port}/{database}",
                    owned.app_password
                );
                std::thread::sleep(std::time::Duration::from_millis(400));
                return Ok(owned);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err("owned PostgreSQL 17 did not become ready".into())
    }
}

#[tokio::test]
#[ignore = "creates and owns a disposable loopback-only PostgreSQL 17 container"]
async fn composite_relation_batches_match_portable_sqlite_semantics()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = OwnedPostgres::start()?;
    let database = Database::<PostgresBackend>::connect_postgres(&owned.url).await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE runtime_relation_pg_customers (
            pg_customer_id BIGINT PRIMARY KEY,
            pg_customer_tenant TEXT NOT NULL,
            pg_customer_status TEXT NOT NULL
         )",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE runtime_relation_pg_contacts (
            pg_contact_id BIGINT PRIMARY KEY,
            pg_contact_tenant TEXT NOT NULL,
            pg_contact_owner_id BIGINT NOT NULL,
            pg_contact_label TEXT NOT NULL,
            pg_contact_payload BYTEA NOT NULL
         )",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE runtime_relation_pg_notes (
            pg_note_id BIGINT PRIMARY KEY,
            pg_note_contact_id BIGINT NOT NULL,
            pg_note_tenant TEXT NOT NULL,
            pg_note_body TEXT NOT NULL
         )",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO runtime_relation_pg_customers VALUES
         (1, 'tenant-a', 'active'), (2, 'tenant-a', 'active'), (3, 'tenant-b', 'empty')",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO runtime_relation_pg_notes VALUES
         (100, 10, 'tenant-a', 'note-a'),
         (200, 20, 'tenant-a', 'note-c'),
         (300, 30, 'tenant-b', 'note-private-b')",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO runtime_relation_pg_contacts VALUES
         (10, 'tenant-a', 1, 'a', decode('00ff', 'hex')),
         (11, 'tenant-a', 1, 'b', decode('01fe', 'hex')),
         (20, 'tenant-a', 2, 'c', decode('02fd', 'hex')),
         (30, 'tenant-b', 3, 'private-b', decode('03fc', 'hex'))",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query("ALTER TABLE runtime_relation_pg_contacts ENABLE ROW LEVEL SECURITY")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query("ALTER TABLE runtime_relation_pg_contacts FORCE ROW LEVEL SECURITY")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query("ALTER TABLE runtime_relation_pg_notes ENABLE ROW LEVEL SECURITY")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query("ALTER TABLE runtime_relation_pg_notes FORCE ROW LEVEL SECURITY")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query(
        "CREATE POLICY runtime_relation_tenant ON runtime_relation_pg_contacts
         USING (pg_contact_tenant = current_setting('app.tenant_id', true))",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "CREATE POLICY runtime_relation_note_tenant ON runtime_relation_pg_notes
         USING (pg_note_tenant = current_setting('app.tenant_id', true))",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(&format!(
        "CREATE ROLE relation_reader LOGIN PASSWORD '{}'",
        owned.app_password
    ))
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "GRANT SELECT ON runtime_relation_pg_customers, runtime_relation_pg_contacts,
         runtime_relation_pg_notes
         TO relation_reader",
    )
    .execute(database.pool())
    .await?;
    let app_database = Database::<PostgresBackend>::connect_postgres(&owned.app_url).await?;

    let tenant_a = DbAuthContext {
        tenant_id: Some("tenant-a".to_string()),
        ..Default::default()
    };

    let schema = schema();
    let customers = schema.resolve_collection(&cid("pg_customers"))?;
    let contacts = schema.resolve_collection(&cid("pg_contacts"))?;
    let notes = schema.resolve_collection(&cid("pg_notes"))?;
    let status = schema.resolve_field(&customers, &fid("pg_customer_status"))?;
    let customer_id = schema.resolve_field(&customers, &fid("pg_customer_id"))?;
    let label = schema.resolve_field(&contacts, &fid("pg_contact_label"))?;
    let note_body = schema.resolve_field(&notes, &fid("pg_note_body"))?;
    let relation = schema.resolve_relation(
        &customers,
        &RelationId::new("pg_customer_contacts").unwrap(),
    )?;
    let note_relation =
        schema.resolve_relation(&contacts, &RelationId::new("pg_contact_notes").unwrap())?;
    let parent_projection = schema.resolve_projection(&customers, std::slice::from_ref(&status))?;
    let child_projection = schema.resolve_projection(&contacts, std::slice::from_ref(&label))?;
    let note_projection = schema.resolve_projection(&notes, std::slice::from_ref(&note_body))?;
    let query_limits = RuntimeQueryLimits::default();
    let parent_request = schema.runtime_read_request_with_relation_keys(
        &customers,
        &parent_projection,
        None,
        schema.runtime_order(&customers, None, query_limits)?,
        RuntimePageRequest::first(10, None),
        false,
        std::slice::from_ref(&relation),
        query_limits,
    )?;
    let parents = app_database
        .execute_runtime_anchored_read(&parent_request, Some(&tenant_a))
        .await?;
    assert_eq!(
        parents.edges[0].node.state(&customer_id)?,
        RuntimeFieldState::Unloaded
    );
    let anchors = parents.relation_parents(&relation)?;
    let request = schema.runtime_relation_batch_request_with_relation_keys(
        &relation,
        anchors.clone(),
        &child_projection,
        None,
        schema.runtime_order(&contacts, None, query_limits)?,
        RuntimeRelationSelection::ToMany {
            pages: vec![RuntimePageRequest::first(1, None); anchors.len()],
            include_count: true,
        },
        std::slice::from_ref(&note_relation),
        RuntimeRelationLimits::default(),
    )?;
    let batch = app_database
        .execute_runtime_relation_batch(&request, Some(&tenant_a))
        .await?;
    let RuntimeRelationValue::ToMany(first) = &batch.results[0].value else {
        return Err("expected to-many relation".into());
    };
    assert_eq!(first.edges[0].node.string(&label)?, "a");
    assert_eq!(first.total_count, Some(2));
    assert!(first.page_info.has_next_page);
    let RuntimeRelationValue::ToMany(empty) = &batch.results[2].value else {
        return Err("expected to-many relation".into());
    };
    assert!(empty.edges.is_empty());
    assert_eq!(empty.total_count, Some(0));

    let note_anchors = batch.relation_parents(&note_relation)?;
    let note_request = schema.runtime_relation_batch_request(
        &note_relation,
        note_anchors.clone(),
        &note_projection,
        None,
        schema.runtime_order(&notes, None, query_limits)?,
        RuntimeRelationSelection::ToMany {
            pages: vec![RuntimePageRequest::first(10, None); note_anchors.len()],
            include_count: false,
        },
        RuntimeRelationLimits::default(),
    )?;
    let notes_batch = app_database
        .execute_runtime_relation_batch(&note_request, Some(&tenant_a))
        .await?;
    let RuntimeRelationValue::ToMany(first_notes) = &notes_batch.results[0].value else {
        return Err("expected nested notes relation".into());
    };
    let RuntimeRelationValue::ToMany(second_notes) = &notes_batch.results[1].value else {
        return Err("expected nested notes relation".into());
    };
    assert_eq!(first_notes.edges[0].node.string(&note_body)?, "note-a");
    assert_eq!(second_notes.edges[0].node.string(&note_body)?, "note-c");

    let tenant_b = DbAuthContext {
        tenant_id: Some("tenant-b".to_string()),
        ..Default::default()
    };
    let tenant_b_batch = app_database
        .execute_runtime_relation_batch(&request, Some(&tenant_b))
        .await?;
    let RuntimeRelationValue::ToMany(hidden_a) = &tenant_b_batch.results[0].value else {
        return Err("expected to-many relation".into());
    };
    assert!(hidden_a.edges.is_empty());
    assert_eq!(hidden_a.total_count, Some(0));
    let RuntimeRelationValue::ToMany(visible_b) = &tenant_b_batch.results[2].value else {
        return Err("expected to-many relation".into());
    };
    assert_eq!(visible_b.edges[0].node.string(&label)?, "private-b");
    assert_eq!(visible_b.total_count, Some(1));
    Ok(())
}
