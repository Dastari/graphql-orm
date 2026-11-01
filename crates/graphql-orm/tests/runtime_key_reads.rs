//! Complete-key read conformance. PostgreSQL uses only a test-owned server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]

use graphql_orm::{db::Database, graphql::orm::*};
#[allow(dead_code)]
#[path = "../examples/runtime_key_read.rs"]
mod host_example;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

fn cid() -> CollectionId {
    CollectionId::new("records").unwrap()
}
fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}
fn definition(composite: bool) -> RuntimeSchema {
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            classification: None,
            description: None,
            id: cid(),
            api_type_name: "Record".into(),
            api_plural_name: "Records".into(),
            physical_table: "runtime_key_records".into(),
            primary_key: if composite {
                vec![fid("shard"), fid("id")]
            } else {
                vec![fid("id")]
            },
            append_only: false,
            retention_purge: false,
            fields: [
                ("id", RuntimeValueKind::Integer),
                ("shard", RuntimeValueKind::String),
                ("tenant", RuntimeValueKind::String),
                ("label", RuntimeValueKind::String),
            ]
            .into_iter()
            .map(|(name, kind)| RuntimeField {
                description: None,
                classification: None,
                export: None,
                id: fid(name),
                api_name: name.into(),
                physical_column: name.into(),
                value_kind: kind,
                nullable: false,
                unique: false,
                filterable: name == "tenant",
                sortable: false,
                generated: false,
                default: None,
                format: None,
                auto_timestamp: None,
                auto_actor: None,
            })
            .collect(),
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            label_fields: Vec::new(),
            search: None,
        }],
    }
}

struct Fixture {
    db: Database<Backend>,
    schema: ValidatedRuntimeSchema,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
impl Fixture {
    async fn new(composite: bool, duplicates: bool) -> Self {
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite_with_options(
            "sqlite::memory:",
            graphql_orm::db::ConnectionOptions::default().max_connections(1),
        )
        .await
        .unwrap();
        #[cfg(feature = "postgres")]
        let owned = owned_postgres::OwnedPostgres::start("runtime-key-read").unwrap();
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres(&owned.url)
            .await
            .unwrap();
        let constraint = if duplicates {
            ""
        } else if composite {
            ", PRIMARY KEY (shard, id)"
        } else {
            ", PRIMARY KEY (id)"
        };
        sqlx::query(&format!("CREATE TABLE runtime_key_records (id BIGINT NOT NULL, shard TEXT NOT NULL, tenant TEXT NOT NULL, label TEXT NOT NULL{constraint})"))
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO runtime_key_records VALUES (1, 'north', 'north', 'old north')")
            .execute(db.pool())
            .await
            .unwrap();
        let second_id = if composite || duplicates { 1 } else { 2 };
        let second_shard = if duplicates { "north" } else { "south" };
        sqlx::query(&format!("INSERT INTO runtime_key_records VALUES ({second_id}, '{second_shard}', '{second_shard}', 'old south')"))
            .execute(db.pool()).await.unwrap();
        Self {
            db,
            schema: definition(composite).validate().unwrap(),
            #[cfg(feature = "postgres")]
            owned,
        }
    }
    fn collection(&self) -> RuntimeCollectionHandle {
        self.schema.resolve_collection(&cid()).unwrap()
    }
    fn field(&self, name: &str) -> RuntimeFieldHandle {
        self.schema
            .resolve_field(&self.collection(), &fid(name))
            .unwrap()
    }
    fn key(&self, id: i64, shard: Option<&str>) -> RuntimeKey {
        let mut members = vec![(self.field("id"), RuntimeValue::Integer(id))];
        if let Some(shard) = shard {
            members.push((self.field("shard"), RuntimeValue::String(shard.into())));
        }
        self.schema
            .runtime_key(&self.collection(), &members)
            .unwrap()
    }
    fn row(&self, tenant: &str) -> RuntimePredicate {
        self.schema
            .runtime_compare(
                &self.collection(),
                &self.field("tenant"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::String(tenant.into()),
                Default::default(),
            )
            .unwrap()
    }
    fn projection(&self) -> RuntimeProjection {
        self.schema
            .resolve_projection(&self.collection(), &[self.field("label")])
            .unwrap()
    }
    fn request(&self, key: &RuntimeKey, row: &RuntimePredicate) -> RuntimeKeyReadRequest {
        self.schema
            .runtime_key_read_request(key, &self.projection(), row, None, Default::default())
            .unwrap()
    }
    async fn finish(self) {
        self.db.pool().close().await;
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup().unwrap();
        }
    }
}

#[tokio::test]
async fn nonfilterable_identity_returns_exact_projection_and_hidden_matches_missing() {
    let f = Fixture::new(false, false).await;
    let key = f.key(1, None);
    assert!(
        f.schema
            .runtime_compare(
                &f.collection(),
                &f.field("id"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::Integer(1),
                Default::default()
            )
            .is_err()
    );
    let request = f.request(&key, &f.row("north"));
    fn owned_send_sync<T: Send + Sync + 'static>(_: &T) {}
    owned_send_sync(&request);
    fn send_future<T: Send>(_: &T) {}
    let future = f.db.execute_runtime_key_read(&request, None);
    send_future(&future);
    let record = future.await.unwrap().unwrap();
    assert_eq!(record.string(&f.field("label")).unwrap(), "old north");
    assert!(matches!(
        record.state(&f.field("id")).unwrap(),
        RuntimeFieldState::Unloaded
    ));
    assert!(matches!(
        record.state(&f.field("tenant")).unwrap(),
        RuntimeFieldState::Unloaded
    ));
    assert!(
        f.db.execute_runtime_key_read(&f.request(&key, &f.row("south")), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.db.execute_runtime_key_read(&f.request(&f.key(999, None), &f.row("north")), None)
            .await
            .unwrap()
            .is_none()
    );
    f.finish().await;
}

#[tokio::test]
async fn complete_composite_key_is_ordered_and_all_members_are_applied_as_binds() {
    let f = Fixture::new(true, false).await;
    let key = f.key(1, Some("south"));
    assert_eq!(key.fields()[0].0.id(), &fid("shard"));
    let record =
        f.db.execute_runtime_key_read(&f.request(&key, &f.row("south")), None)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(record.string(&f.field("label")).unwrap(), "old south");
    let attack = f.key(1, Some("north' OR 1=1 -- ? $1"));
    let required = f
        .schema
        .runtime_and(&f.collection(), vec![], Default::default())
        .unwrap();
    assert!(
        f.db.execute_runtime_key_read(&f.request(&attack, &required), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        record.state(&f.field("shard")).unwrap(),
        RuntimeFieldState::Unloaded
    ));
    f.finish().await;
}

#[tokio::test]
async fn duplicate_physical_identity_is_a_redacted_error_even_with_page_limit_one() {
    let f = Fixture::new(false, true).await;
    let row = f.row("north");
    let request = f
        .schema
        .runtime_key_read_request(
            &f.key(1, None),
            &f.projection(),
            &row,
            None,
            RuntimeQueryLimits {
                max_page_size: 1,
                ..Default::default()
            },
        )
        .unwrap();
    let error =
        f.db.execute_runtime_key_read(&request, None)
            .await
            .unwrap_err();
    assert_eq!(
        error.code(),
        RuntimeKeyReadErrorCode::UnexpectedMultiplicity
    );
    let safe = format!("{error:?} {error} {request:?}");
    for secret in [
        "old north",
        "old south",
        "runtime_key_records",
        "SELECT",
        "north",
    ] {
        assert!(!safe.contains(secret), "redacted diagnostics");
    }
    f.finish().await;
}

#[tokio::test]
async fn optional_filter_intersects_required_row_and_never_grants_filterability() {
    let f = Fixture::new(false, false).await;
    assert!(
        f.schema
            .runtime_compare(
                &f.collection(),
                &f.field("label"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::String("old north".into()),
                Default::default()
            )
            .is_err()
    );
    let request = f
        .schema
        .runtime_key_read_request(
            &f.key(1, None),
            &f.projection(),
            &f.row("north"),
            Some(&f.row("south")),
            Default::default(),
        )
        .unwrap();
    assert!(
        f.db.execute_runtime_key_read(&request, None)
            .await
            .unwrap()
            .is_none()
    );
    f.finish().await;
}

#[tokio::test]
async fn rendered_bind_budget_is_checked_before_io_for_suffix_predicates() {
    let f = Fixture::new(false, false).await;
    let predicate = f
        .schema
        .runtime_compare(
            &f.collection(),
            &f.field("tenant"),
            RuntimeScalarOperator::EndsWith,
            RuntimeValue::String("rth".into()),
            Default::default(),
        )
        .unwrap();
    let positive_budget = if cfg!(feature = "sqlite") { 3 } else { 2 };
    let request = f
        .schema
        .runtime_key_read_request(
            &f.key(1, None),
            &f.projection(),
            &predicate,
            None,
            RuntimeQueryLimits {
                max_bind_parameters: positive_budget,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        f.db.execute_runtime_key_read(&request, None)
            .await
            .unwrap()
            .unwrap()
            .string(&f.field("label"))
            .unwrap(),
        "old north"
    );
    #[cfg(feature = "sqlite")]
    {
        let request = f
            .schema
            .runtime_key_read_request(
                &f.key(1, None),
                &f.projection(),
                &predicate,
                None,
                RuntimeQueryLimits {
                    max_bind_parameters: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        f.db.pool().close().await;
        assert_eq!(
            f.db.execute_runtime_key_read(&request, None)
                .await
                .unwrap_err()
                .code(),
            RuntimeKeyReadErrorCode::LimitExceeded
        );
    }
    f.finish().await;
}

#[tokio::test]
async fn malformed_and_mixed_handles_and_stricter_request_budgets_fail_without_database() {
    let f = Fixture::new(true, false).await;
    f.db.pool().close().await; // Any executor I/O would now fail.
    for fields in [
        vec![],
        vec![(f.field("id"), RuntimeValue::Integer(1))],
        vec![
            (f.field("id"), RuntimeValue::Integer(1)),
            (f.field("id"), RuntimeValue::Integer(2)),
        ],
        vec![
            (f.field("id"), RuntimeValue::Null),
            (f.field("shard"), RuntimeValue::String("north".into())),
        ],
        vec![
            (f.field("id"), RuntimeValue::String("wrong kind".into())),
            (f.field("shard"), RuntimeValue::String("north".into())),
        ],
    ] {
        assert!(f.schema.runtime_key(&f.collection(), &fields).is_err());
    }
    let mut stale = definition(true);
    stale.collections[0].fields[3].api_name = "Renamed".into();
    let stale = stale.validate().unwrap();
    assert_eq!(
        stale
            .runtime_key_read_request(
                &f.key(1, Some("north")),
                &f.projection(),
                &f.row("north"),
                None,
                Default::default()
            )
            .unwrap_err()
            .code(),
        RuntimeKeyReadErrorCode::InvalidHandle
    );
    let stale_collection = stale.resolve_collection(&cid()).unwrap();
    let stale_label = stale
        .resolve_field(&stale_collection, &fid("label"))
        .unwrap();
    let stale_projection = stale
        .resolve_projection(&stale_collection, &[stale_label])
        .unwrap();
    assert_eq!(
        f.schema
            .runtime_key_read_request(
                &f.key(1, Some("north")),
                &stale_projection,
                &f.row("north"),
                None,
                Default::default()
            )
            .unwrap_err()
            .code(),
        RuntimeKeyReadErrorCode::InvalidProjection
    );
    let stale_row = stale
        .runtime_and(&stale_collection, vec![], Default::default())
        .unwrap();
    assert_eq!(
        f.schema
            .runtime_key_read_request(
                &f.key(1, Some("north")),
                &f.projection(),
                &stale_row,
                None,
                Default::default()
            )
            .unwrap_err()
            .code(),
        RuntimeKeyReadErrorCode::InvalidPredicate
    );
    assert!(f.schema.resolve_projection(&f.collection(), &[]).is_err());
    assert!(
        f.schema
            .resolve_projection(&f.collection(), &[f.field("label"), f.field("label")])
            .is_err()
    );
    let mut other = definition(false);
    other.collections[0].id = CollectionId::new("other").unwrap();
    other.collections[0].api_type_name = "Other".into();
    other.collections[0].api_plural_name = "Others".into();
    other.collections[0].physical_table = "other_records".into();
    let mut other = other.collections.remove(0);
    for field in &mut other.fields {
        field.id = fid(&format!("other_{}", field.id));
    }
    other.primary_key = vec![fid("other_id")];
    let mut mixed = definition(true);
    mixed.collections.push(other);
    let mixed = mixed.validate().unwrap();
    let a = mixed.resolve_collection(&cid()).unwrap();
    let b = mixed
        .resolve_collection(&CollectionId::new("other").unwrap())
        .unwrap();
    let b_id = mixed.resolve_field(&b, &fid("other_id")).unwrap();
    let a_shard = mixed.resolve_field(&a, &fid("shard")).unwrap();
    assert!(
        mixed
            .runtime_key(
                &a,
                &[
                    (b_id, RuntimeValue::Integer(1)),
                    (a_shard, RuntimeValue::String("north".into()))
                ]
            )
            .is_err()
    );
    let b_label = mixed.resolve_field(&b, &fid("other_label")).unwrap();
    let b_projection = mixed.resolve_projection(&b, &[b_label]).unwrap();
    let key = mixed
        .runtime_key(
            &a,
            &[
                (
                    mixed.resolve_field(&a, &fid("id")).unwrap(),
                    RuntimeValue::Integer(1),
                ),
                (
                    mixed.resolve_field(&a, &fid("shard")).unwrap(),
                    RuntimeValue::String("north".into()),
                ),
            ],
        )
        .unwrap();
    let required = mixed.runtime_and(&a, vec![], Default::default()).unwrap();
    assert_eq!(
        mixed
            .runtime_key_read_request(&key, &b_projection, &required, None, Default::default())
            .unwrap_err()
            .code(),
        RuntimeKeyReadErrorCode::InvalidProjection
    );
    let b_row = mixed.runtime_and(&b, vec![], Default::default()).unwrap();
    let projection = mixed
        .resolve_projection(&a, &[mixed.resolve_field(&a, &fid("label")).unwrap()])
        .unwrap();
    assert_eq!(
        mixed
            .runtime_key_read_request(&key, &projection, &b_row, None, Default::default())
            .unwrap_err()
            .code(),
        RuntimeKeyReadErrorCode::InvalidPredicate
    );
    for limits in [
        RuntimeQueryLimits {
            max_bind_parameters: 2,
            ..Default::default()
        },
        RuntimeQueryLimits {
            max_predicate_nodes: 1,
            ..Default::default()
        },
        RuntimeQueryLimits {
            max_predicate_depth: 1,
            ..Default::default()
        },
        RuntimeQueryLimits {
            max_projection_fields: 0,
            ..Default::default()
        },
    ] {
        assert_eq!(
            f.schema
                .runtime_key_read_request(
                    &f.key(1, Some("north")),
                    &f.projection(),
                    &f.row("north"),
                    None,
                    limits
                )
                .unwrap_err()
                .code(),
            RuntimeKeyReadErrorCode::LimitExceeded
        );
    }
    let list = f
        .schema
        .runtime_list(
            &f.collection(),
            &f.field("tenant"),
            RuntimeListOperator::In,
            vec![
                RuntimeValue::String("north".into()),
                RuntimeValue::String("south".into()),
            ],
            Default::default(),
        )
        .unwrap();
    assert_eq!(
        f.schema
            .runtime_key_read_request(
                &f.key(1, Some("north")),
                &f.projection(),
                &list,
                None,
                RuntimeQueryLimits {
                    max_values_per_list: 1,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code(),
        RuntimeKeyReadErrorCode::LimitExceeded
    );
    f.finish().await;
}

#[tokio::test]
async fn a_retained_request_reads_current_committed_data_and_no_deleted_preimage() {
    let f = Fixture::new(false, false).await;
    let request = f.request(&f.key(1, None), &f.row("north"));
    sqlx::query("UPDATE runtime_key_records SET label='new north' WHERE id=1")
        .execute(f.db.pool())
        .await
        .unwrap();
    assert_eq!(
        f.db.execute_runtime_key_read(&request, None)
            .await
            .unwrap()
            .unwrap()
            .string(&f.field("label"))
            .unwrap(),
        "new north"
    );
    sqlx::query("DELETE FROM runtime_key_records WHERE id=1")
        .execute(f.db.pool())
        .await
        .unwrap();
    assert!(
        f.db.execute_runtime_key_read(&request, None)
            .await
            .unwrap()
            .is_none()
    );
    f.finish().await;
}

#[tokio::test]
async fn host_example_authorizes_exact_intent_and_denies_before_protected_io() {
    struct TrustedTestAuthority {
        allow_key_read: bool,
    }
    impl host_example::CurrentReadAuthority for TrustedTestAuthority {
        fn authorize<'a>(
            &'a self,
            schema: &'a ValidatedRuntimeSchema,
            key: &'a RuntimeKey,
            projection: &'a RuntimeProjection,
        ) -> host_example::AuthorityFuture<'a> {
            Box::pin(async move {
                // Synthetic trusted authority, not principal data from a caller.
                if !self.allow_key_read
                    || projection.fields().len() != 1
                    || projection.fields()[0].id() != &fid("label")
                {
                    return Err(host_example::HostReadError::Denied);
                }
                let tenant = schema
                    .resolve_field(key.collection(), &fid("tenant"))
                    .unwrap();
                let row = schema
                    .runtime_compare(
                        key.collection(),
                        &tenant,
                        RuntimeScalarOperator::Eq,
                        RuntimeValue::String("north".into()),
                        Default::default(),
                    )
                    .unwrap();
                Ok((
                    row,
                    DbAuthContext {
                        tenant_id: Some("north".into()),
                        ..Default::default()
                    },
                ))
            })
        }
    }
    let f = Fixture::new(false, false).await;
    let allowed = TrustedTestAuthority {
        allow_key_read: true,
    };
    let record = host_example::read_authorized(
        &f.db,
        &f.schema,
        &f.key(1, None),
        &f.projection(),
        Some(&allowed),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(record.string(&f.field("label")).unwrap(), "old north");
    f.db.pool().close().await;
    let result =
        host_example::read_authorized(&f.db, &f.schema, &f.key(1, None), &f.projection(), None)
            .await;
    assert!(matches!(result, Err(host_example::HostReadError::Denied)));
    let denied = TrustedTestAuthority {
        allow_key_read: false,
    };
    assert!(matches!(
        host_example::read_authorized(
            &f.db,
            &f.schema,
            &f.key(1, None),
            &f.projection(),
            Some(&denied)
        )
        .await,
        Err(host_example::HostReadError::Denied)
    ));
    let forbidden_projection = f
        .schema
        .resolve_projection(&f.collection(), &[f.field("id")])
        .unwrap();
    assert!(matches!(
        host_example::read_authorized(
            &f.db,
            &f.schema,
            &f.key(1, None),
            &forbidden_projection,
            Some(&allowed)
        )
        .await,
        Err(host_example::HostReadError::Denied)
    ));
    f.finish().await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_non_superuser_rls_isolation_cancellation_and_concurrent_change() {
    use std::{str::FromStr, time::Duration};
    let f = Fixture::new(false, false).await;
    for sql in [
        "ALTER TABLE runtime_key_records ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE runtime_key_records FORCE ROW LEVEL SECURITY",
        "CREATE POLICY tenant_rows ON runtime_key_records USING (tenant = current_setting('app.tenant_id', true))",
        "CREATE ROLE runtime_key_reader LOGIN PASSWORD 'owned-fixture-only' NOSUPERUSER NOBYPASSRLS",
        "GRANT USAGE ON SCHEMA public TO runtime_key_reader",
        "GRANT SELECT ON runtime_key_records TO runtime_key_reader",
    ] {
        sqlx::query(sql).execute(f.db.pool()).await.unwrap();
    }
    let options = sqlx::postgres::PgConnectOptions::from_str(&f.owned.url)
        .unwrap()
        .username("runtime_key_reader")
        .password("owned-fixture-only");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let db = Database::<Backend>::new(pool.clone());
    let (name, superuser, bypass): (String, bool, bool) = sqlx::query_as(
        "SELECT current_user::text, rolsuper, rolbypassrls FROM pg_roles WHERE rolname=current_user")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(name, "runtime_key_reader");
    assert!(!superuser && !bypass);
    let required = f
        .schema
        .runtime_and(&f.collection(), vec![], Default::default())
        .unwrap();
    let first = f.request(&f.key(1, None), &required);
    let second = f.request(&f.key(2, None), &required);
    let north = DbAuthContext {
        tenant_id: Some("north".into()),
        ..Default::default()
    };
    let south = DbAuthContext {
        tenant_id: Some("south".into()),
        ..Default::default()
    };
    let (a, b) = tokio::join!(
        db.execute_runtime_key_read(&first, Some(&north)),
        db.execute_runtime_key_read(&first, Some(&south))
    );
    assert!(a.unwrap().is_some());
    assert!(b.unwrap().is_none());
    assert!(
        db.execute_runtime_key_read(&first, None)
            .await
            .unwrap()
            .is_none()
    );
    let mut lock = f.db.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE runtime_key_records IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let pending_db = db.clone();
    let pending_request = first.clone();
    let pending_auth = north.clone();
    let pending = tokio::spawn(async move {
        pending_db
            .execute_runtime_key_read(&pending_request, Some(&pending_auth))
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE usename='runtime_key_reader' AND wait_event_type='Lock' AND query LIKE 'SELECT %runtime_key_records%')")
                .fetch_one(f.db.pool()).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("owned read must reach the SQL lock before cancellation");
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    sqlx::query("UPDATE runtime_key_records SET label='changed while waiting' WHERE id=2")
        .execute(&mut *lock)
        .await
        .unwrap();
    lock.commit().await.unwrap();
    let record = tokio::time::timeout(
        Duration::from_secs(10),
        db.execute_runtime_key_read(&second, Some(&south)),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(
        record.string(&f.field("label")).unwrap(),
        "changed while waiting"
    );
    assert!(
        db.execute_runtime_key_read(&second, Some(&north))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db.execute_runtime_key_read(&second, None)
            .await
            .unwrap()
            .is_none()
    );
    let setting: Option<String> =
        sqlx::query_scalar("SELECT nullif(current_setting('app.tenant_id', true), '')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(setting.is_none());
    pool.close().await;
    f.finish().await;
}
