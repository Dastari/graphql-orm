//! Runtime relation layers stay under SQLite's per-statement variable limit.
//!
//! SQLite builds before 3.32, and later builds that keep the old default,
//! refuse a statement with more than 999 variables. A relation layer that
//! binds more is sent as several statements, each within that limit, and
//! returns what one statement would have. One parent group that alone needs
//! more is refused before any statement runs (#528).
//!
//! The test's connection has its variable limit set to 999, as such a build
//! has, so a statement above it fails in SQLite itself.
#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
use graphql_orm::db::Database;
use graphql_orm::graphql::orm::*;

const PARENTS: i64 = 100;

// SQLite's own limit setter, from the library sqlx links.
unsafe extern "C" {
    fn sqlite3_limit(
        db: *mut std::ffi::c_void,
        id: std::ffi::c_int,
        new_value: std::ffi::c_int,
    ) -> std::ffi::c_int;
}
const SQLITE_LIMIT_VARIABLE_NUMBER: std::ffi::c_int = 9;

/// Give the pool's only connection the 999-variable limit of older builds.
async fn limit_variables(database: &Database<SqliteBackend>) {
    let mut connection = database.pool().acquire().await.unwrap();
    let mut handle = connection.lock_handle().await.unwrap();
    let raw = handle.as_raw_handle().as_ptr().cast::<std::ffi::c_void>();
    // SAFETY: `raw` is the open connection this guard holds locked, and
    // `sqlite3_limit` only reads and sets a per-connection integer.
    unsafe { sqlite3_limit(raw, SQLITE_LIMIT_VARIABLE_NUMBER, 999) };
    let current = unsafe { sqlite3_limit(raw, SQLITE_LIMIT_VARIABLE_NUMBER, -1) };
    assert_eq!(current, 999);
}

fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}
fn cid(value: &str) -> CollectionId {
    CollectionId::new(value).unwrap()
}
fn field(name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: fid(name),
        api_name: name.into(),
        physical_column: name.into(),
        value_kind: kind,
        nullable: false,
        unique: name.ends_with("_id"),
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}
fn relation(
    id: &str,
    source: &str,
    target: &str,
    cardinality: RelationCardinality,
) -> RuntimeRelation {
    RuntimeRelation {
        description: None,
        classification: None,
        export: None,
        id: RelationId::new(id).unwrap(),
        api_name: id.into(),
        target: cid("items"),
        key_pairs: vec![RelationKeyPair {
            source: fid(source),
            target: fid(target),
        }],
        cardinality,
        enforce_foreign_key: false,
        on_delete: None,
        join: None,
    }
}
fn collection(
    id: &str,
    key: &str,
    table: &str,
    fields: Vec<RuntimeField>,
    relations: Vec<RuntimeRelation>,
) -> RuntimeCollection {
    RuntimeCollection {
        node_identity: None,
        live_subscriptions: None,
        classification: None,
        description: None,
        id: cid(id),
        api_type_name: format!("Wide{id}"),
        api_plural_name: format!("Wide{id}s"),
        physical_table: table.into(),
        primary_key: vec![fid(key)],
        append_only: false,
        retention_purge: false,
        fields,
        relations,
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
        label_fields: Vec::new(),
        search: None,
    }
}
fn schema() -> ValidatedRuntimeSchema {
    RuntimeSchema {
        repository_only: false,
        format_version: 1,
        collections: vec![
            collection(
                "owners",
                "owner_id",
                "wide_owners",
                vec![
                    field("owner_id", RuntimeValueKind::Integer),
                    field("favourite_item", RuntimeValueKind::Integer),
                ],
                vec![
                    relation(
                        "owner_items",
                        "owner_id",
                        "item_owner",
                        RelationCardinality::Many,
                    ),
                    relation(
                        "owner_favourite",
                        "favourite_item",
                        "item_id",
                        RelationCardinality::One,
                    ),
                ],
            ),
            collection(
                "items",
                "item_id",
                "wide_items",
                vec![
                    field("item_id", RuntimeValueKind::Integer),
                    field("item_owner", RuntimeValueKind::Integer),
                    field("label", RuntimeValueKind::String),
                ],
                vec![],
            ),
        ],
    }
    .validate()
    .unwrap()
}

#[tokio::test]
async fn wide_relation_layers_are_split_at_the_sqlite_variable_limit() {
    let schema = schema();
    let owners = schema.resolve_collection(&cid("owners")).unwrap();
    let items = schema.resolve_collection(&cid("items")).unwrap();
    let label = schema.resolve_field(&items, &fid("label")).unwrap();
    let to_many = schema
        .resolve_relation(&owners, &RelationId::new("owner_items").unwrap())
        .unwrap();
    let to_one = schema
        .resolve_relation(&owners, &RelationId::new("owner_favourite").unwrap())
        .unwrap();
    let database = Database::<SqliteBackend>::connect_sqlite_with_options(
        "sqlite::memory:",
        graphql_orm::db::ConnectionOptions::default().max_connections(1),
    )
    .await
    .unwrap();
    for sql in [
        "CREATE TABLE wide_owners (owner_id INTEGER PRIMARY KEY, favourite_item INTEGER NOT NULL)",
        "CREATE TABLE wide_items (item_id INTEGER PRIMARY KEY, item_owner INTEGER NOT NULL, label TEXT NOT NULL)",
    ] {
        sqlx::query(sql).execute(database.pool()).await.unwrap();
    }
    // Owner n has items n*10 ("l0"), n*10+1 ("l1") and n*10+2 ("other");
    // its favourite is the second.
    for owner in 1..=PARENTS {
        sqlx::query("INSERT INTO wide_owners (owner_id, favourite_item) VALUES (?, ?)")
            .bind(owner)
            .bind(owner * 10 + 1)
            .execute(database.pool())
            .await
            .unwrap();
        for (offset, name) in ["l0", "l1", "other"].into_iter().enumerate() {
            sqlx::query("INSERT INTO wide_items (item_id, item_owner, label) VALUES (?, ?, ?)")
                .bind(owner * 10 + offset as i64)
                .bind(owner)
                .bind(name)
                .execute(database.pool())
                .await
                .unwrap();
        }
    }
    limit_variables(&database).await;
    // The limit is real: SQLite refuses 1,000 variables on this connection.
    let too_many = format!("SELECT {}", vec!["?"; 1_000].join(" + "));
    let mut query = sqlx::query(&too_many);
    for _ in 0..1_000 {
        query = query.bind(1_i64);
    }
    assert!(query.fetch_all(database.pool()).await.is_err());
    let query_limits = RuntimeQueryLimits::default();
    // Twelve bound values per parent, plus its key.
    let wanted = || {
        schema
            .runtime_list(
                &items,
                &label,
                RuntimeListOperator::In,
                (0..12)
                    .map(|n| RuntimeValue::String(format!("l{n}")))
                    .collect(),
                query_limits,
            )
            .unwrap()
    };
    let projection = schema
        .resolve_projection(&items, std::slice::from_ref(&label))
        .unwrap();
    let parents = |count: i64| {
        let request = schema
            .runtime_read_request_with_relation_keys(
                &owners,
                &schema
                    .resolve_projection(
                        &owners,
                        &[schema.resolve_field(&owners, &fid("owner_id")).unwrap()],
                    )
                    .unwrap(),
                None,
                schema.runtime_order(&owners, None, query_limits).unwrap(),
                RuntimePageRequest::first(count, None),
                false,
                &[to_many.clone(), to_one.clone()],
                query_limits,
            )
            .unwrap();
        let database = database.clone();
        async move {
            database
                .execute_runtime_anchored_read(&request, None)
                .await
                .unwrap()
        }
    };
    let labels = |connection: &RuntimeConnection| {
        connection
            .edges
            .iter()
            .map(|edge| match edge.node.state(&label).unwrap() {
                RuntimeFieldState::Value(RuntimeValue::String(text)) => text.clone(),
                other => panic!("unexpected label state: {other:?}"),
            })
            .collect::<Vec<_>>()
    };
    // (parents, statements for the paged rows and their counts, statements
    // for the to-one layer). 13 binds per parent: 76 parents fill 988 of
    // SQLite's 999 variables, so 77 parents need a second statement.
    for (count, to_many_statements, to_one_statements) in
        [(50, 2, 1), (76, 2, 1), (77, 4, 2), (PARENTS, 4, 2)]
    {
        let page = parents(count).await;
        let many = schema
            .runtime_relation_batch_request(
                &to_many,
                page.relation_parents(&to_many).unwrap(),
                &projection,
                Some(wanted()),
                schema.runtime_order(&items, None, query_limits).unwrap(),
                RuntimeRelationSelection::ToMany {
                    pages: vec![RuntimePageRequest::first(1, None); count as usize],
                    include_count: true,
                },
                RuntimeRelationLimits::default(),
            )
            .unwrap();
        reset_query_count();
        let batch = database
            .execute_runtime_relation_batch(&many, None)
            .await
            .unwrap();
        assert_eq!(query_count(), to_many_statements, "{count} parents");
        assert_eq!(batch.results.len(), count as usize);
        for (index, result) in batch.results.iter().enumerate() {
            assert_eq!(result.parent_index, index);
            let RuntimeRelationValue::ToMany(connection) = &result.value else {
                panic!("to-many relation");
            };
            assert_eq!(labels(connection), ["l0"], "parent {index} of {count}");
            assert_eq!(connection.total_count, Some(2), "parent {index} of {count}");
            assert!(connection.page_info.has_next_page);
        }
        let one = schema
            .runtime_relation_batch_request(
                &to_one,
                page.relation_parents(&to_one).unwrap(),
                &projection,
                Some(wanted()),
                schema.runtime_order(&items, None, query_limits).unwrap(),
                RuntimeRelationSelection::ToOne,
                RuntimeRelationLimits::default(),
            )
            .unwrap();
        reset_query_count();
        let batch = database
            .execute_runtime_relation_batch(&one, None)
            .await
            .unwrap();
        assert_eq!(query_count(), to_one_statements, "{count} parents");
        for (index, result) in batch.results.iter().enumerate() {
            let RuntimeRelationValue::ToOne(Some(record)) = &result.value else {
                panic!("parent {index} of {count} has a favourite");
            };
            assert_eq!(
                record.state(&label).unwrap(),
                RuntimeFieldState::Value(&RuntimeValue::String("l1".into()))
            );
        }
    }
    // One parent group that needs more than 999 variables cannot be split.
    // `filter(n)` binds n values across ten lists; with the parent key a
    // group binds n + 1.
    let filter = |values: usize| {
        let lists = (0..10)
            .map(|list| {
                let size = values / 10 + usize::from(list < values % 10);
                schema
                    .runtime_list(
                        &items,
                        &label,
                        RuntimeListOperator::In,
                        (0..size)
                            .map(|n| RuntimeValue::String(format!("l{n}")))
                            .collect(),
                        query_limits,
                    )
                    .unwrap()
            })
            .collect();
        schema.runtime_and(&items, lists, query_limits).unwrap()
    };
    let page = parents(2).await;
    for (values, served) in [(998, true), (999, false)] {
        for include_count in [false, true] {
            let request = schema
                .runtime_relation_batch_request(
                    &to_many,
                    page.relation_parents(&to_many).unwrap(),
                    &projection,
                    Some(filter(values)),
                    schema.runtime_order(&items, None, query_limits).unwrap(),
                    RuntimeRelationSelection::ToMany {
                        pages: vec![RuntimePageRequest::first(5, None); 2],
                        include_count,
                    },
                    RuntimeRelationLimits::default(),
                )
                .unwrap();
            reset_query_count();
            let result = database
                .execute_runtime_relation_batch(&request, None)
                .await;
            if served {
                // 999 variables per statement: one statement per parent.
                let batch = result.unwrap();
                assert_eq!(query_count(), if include_count { 4 } else { 2 });
                for result in &batch.results {
                    let RuntimeRelationValue::ToMany(connection) = &result.value else {
                        panic!("to-many relation");
                    };
                    assert_eq!(labels(connection), ["l0", "l1"]);
                }
            } else {
                // 1,000 variables: refused before anything is sent, with the
                // typed refusal the static relation loaders give.
                let error = result.unwrap_err();
                assert_eq!(error.code(), RuntimeRelationErrorCode::ResourceLimit);
                assert_eq!(query_count(), 0, "{values} values");
                let source = std::error::Error::source(&error)
                    .and_then(|source| source.downcast_ref::<sqlx::Error>())
                    .expect("the typed public refusal");
                assert_eq!(
                    graphql_orm::graphql::errors::OrmPublicError::from_sqlx(source).code,
                    graphql_orm::graphql::errors::OrmErrorCode::InvalidInput
                );
            }
        }
    }
    // The same for a to-one layer.
    let one = schema
        .runtime_relation_batch_request(
            &to_one,
            page.relation_parents(&to_one).unwrap(),
            &projection,
            Some(filter(999)),
            schema.runtime_order(&items, None, query_limits).unwrap(),
            RuntimeRelationSelection::ToOne,
            RuntimeRelationLimits::default(),
        )
        .unwrap();
    reset_query_count();
    let error = database
        .execute_runtime_relation_batch(&one, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), RuntimeRelationErrorCode::ResourceLimit);
    assert_eq!(query_count(), 0);
    // The layer's own limit still bounds the total: nothing is sent above it.
    let page = parents(PARENTS).await;
    let refused = schema
        .runtime_relation_batch_request(
            &to_many,
            page.relation_parents(&to_many).unwrap(),
            &projection,
            Some(wanted()),
            schema.runtime_order(&items, None, query_limits).unwrap(),
            RuntimeRelationSelection::ToMany {
                pages: vec![RuntimePageRequest::first(1, None); PARENTS as usize],
                include_count: true,
            },
            RuntimeRelationLimits {
                max_bind_parameters: 1_299,
                ..Default::default()
            },
        )
        .unwrap();
    reset_query_count();
    let error = database
        .execute_runtime_relation_batch(&refused, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), RuntimeRelationErrorCode::ResourceLimit);
    assert_eq!(query_count(), 0);
}
