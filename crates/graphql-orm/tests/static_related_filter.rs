#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
))]
#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{argument_name as a, field_name as f, resolver_name as r};
use serde_json::{Value, json};
#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "mssql")]
type Backend = MssqlBackend;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "filter_parents",
    plural = "FilterParents",
    keyset = "id asc",
    aggregate = true
)]
struct FilterParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
    #[relation(target = "FilterLink", from = "id", to = "parent_id", multiple, filter)]
    links: Vec<FilterLink>,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "filter_links", plural = "FilterLinks")]
struct FilterLink {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
    parent_id: String,
    #[filterable(type = "string")]
    target_id: String,
    #[filterable(type = "boolean")]
    visible: bool,
    #[relation(
        target = "FilterTarget",
        from = "target_id",
        to = "id",
        multiple,
        filter,
        emit_fk = false
    )]
    targets: Vec<FilterTarget>,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "filter_targets", plural = "FilterTargets")]
struct FilterTarget {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    label: String,
    #[filterable(type = "boolean")]
    visible: bool,
}
schema_roots! { query_custom_ops: [], entities: [FilterParent, FilterLink, FilterTarget], }
#[test]
fn generated_where_can_filter_a_collection() {
    let sdl = async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .finish()
    .sdl();
    let input = sdl
        .split("input FilterParentWhereInput {")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    assert!(
        input.contains(&format!("{}: [FilterLinkWhereInput!]", f("links"))),
        "{input}"
    );
}

struct Visibility(bool);
impl RowPolicy for Visibility {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(row.downcast_ref::<FilterTarget>().is_none_or(|r| r.visible)
                && row.downcast_ref::<FilterLink>().is_none_or(|r| r.visible))
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            Ok(match entity {
                "FilterTarget" if self.0 => ReadVisibility::CallbackOnly,
                "FilterTarget" => ReadVisibility::Complete(
                    ReadPredicate::from_filter::<Backend, _>(&FilterTargetWhereInput {
                        visible: Some(BoolFilter {
                            eq: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })?,
                ),
                "FilterLink" => ReadVisibility::Complete(ReadPredicate::from_filter::<Backend, _>(
                    &FilterLinkWhereInput {
                        visible: Some(BoolFilter {
                            eq: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )?),
                _ => ReadVisibility::Unrestricted,
            })
        })
    }
}
type Schema = async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;
async fn setup() -> Schema {
    setup_policy(false).await
}
async fn setup_policy(callback: bool) -> Schema {
    setup_options(callback, true).await
}
async fn setup_options(callback: bool, include_denied: bool) -> Schema {
    #[cfg(feature = "sqlite")]
    let mut db = Database::<Backend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    #[cfg(feature = "postgres")]
    let db = {
        let owned = owned_postgres::OwnedPostgres::start("static-filter").unwrap();
        let db = Database::<Backend>::connect_postgres(&owned.url)
            .await
            .unwrap();
        // Keep the test-owned container alive until after all schema users drop.
        // Its owner handle is installed in schema data below.
        (db, owned)
    };
    #[cfg(feature = "postgres")]
    let (mut db, owned) = db;
    #[cfg(feature = "mssql")]
    let (mut db, owned) = {
        let owned = owned_mssql::OwnedSqlServer::start().await.unwrap();
        let db = Database::<Backend>::connect_ado(&owned.connection_string)
            .await
            .unwrap();
        (db, owned)
    };
    #[cfg(not(feature = "mssql"))]
    {
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "related-filter",
                "owned fixture",
                &[
                    FilterParent::metadata(),
                    FilterLink::metadata(),
                    FilterTarget::metadata(),
                ],
            )
            .await
            .unwrap();
        db.schema()
            .apply_migration(&plan, Default::default())
            .await
            .unwrap();
    }
    #[cfg(not(feature = "mssql"))]
    {
        for id in ["a", "b", "c", "d"] {
            FilterParent::insert(
                &db,
                CreateFilterParentInput {
                    id: id.into(),
                    label: id.into(),
                },
            )
            .await
            .unwrap();
        }
        for (id, visible) in [("allowed", true), ("denied", false)] {
            if id == "denied" && !include_denied {
                continue;
            }
            FilterTarget::insert(
                &db,
                CreateFilterTargetInput {
                    id: id.into(),
                    label: id.into(),
                    visible,
                },
            )
            .await
            .unwrap();
        }
        for (id, parent, target, visible) in [
            ("1", "a", "denied", true),
            ("2", "b", "allowed", true),
            ("3", "c", "allowed", true),
            ("4", "d", "allowed", false),
        ] {
            FilterLink::insert(
                &db,
                CreateFilterLinkInput {
                    id: id.into(),
                    label: id.into(),
                    parent_id: parent.into(),
                    target_id: target.into(),
                    visible,
                },
            )
            .await
            .unwrap();
        }
    }
    #[cfg(feature = "mssql")]
    {
        // MSSQL exposes generated reads here; seed this disposable server
        // directly rather than requiring the SQLx-only repository write API.
        owned_mssql::execute_batch(&owned.connection_string,
            "CREATE TABLE filter_parents (id NVARCHAR(64) NOT NULL PRIMARY KEY, label NVARCHAR(128) NOT NULL);
             CREATE TABLE filter_targets (id NVARCHAR(64) NOT NULL PRIMARY KEY, label NVARCHAR(128) NOT NULL, visible BIT NOT NULL);
             CREATE TABLE filter_links (id NVARCHAR(64) NOT NULL PRIMARY KEY, label NVARCHAR(128) NOT NULL, parent_id NVARCHAR(64) NOT NULL, target_id NVARCHAR(64) NOT NULL, visible BIT NOT NULL);
             INSERT INTO filter_parents (id, label) VALUES ('a','a'),('b','b'),('c','c'),('d','d');
             INSERT INTO filter_targets (id, label, visible) VALUES ('allowed','allowed',1);
             INSERT INTO filter_links (id, label, parent_id, target_id, visible) VALUES
             ('1','1','a','denied',1),('2','2','b','allowed',1),('3','3','c','allowed',1),('4','4','d','allowed',0);"
        ).await.unwrap();
        if include_denied {
            owned_mssql::execute_batch(
                &owned.connection_string,
                "INSERT INTO filter_targets (id, label, visible) VALUES ('denied','denied',0)",
            )
            .await
            .unwrap();
        }
    }
    db.set_row_policy(Visibility(callback));
    let builder = async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .data(db)
    .data(AuthSubject::builder("reader").build());
    #[cfg(any(feature = "postgres", feature = "mssql"))]
    let builder = builder.data(owned);
    builder.finish()
}
fn related(target: &str) -> Value {
    json!({ f("links"): [{f("targets"): [{f("id"): {f("eq"): target}}]}] })
}
fn selection() -> String {
    format!(
        "edges {{ node {{ {} }} cursor }} pageInfo {{ totalCount endCursor }}",
        f("id")
    )
}
async fn list(schema: &Schema, filter: Value, limit: i32) -> Value {
    let response = schema.execute(async_graphql::Request::new(format!(
        "query($w: FilterParentWhereInput!) {{ rows: {}({}: $w, {}: {{limit: {limit}}}) {{ {} }} }}",
        r("filterParents"), a("where"), a("page"), selection()))
        .variables(async_graphql::Variables::from_json(json!({"w": filter})))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["rows"].clone()
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn filter_precedes_paging_and_counts() {
    let schema = setup().await;
    let rows = list(&schema, related("allowed"), 1).await;
    assert_eq!(rows["edges"][0]["node"][f("id")], "b");
    assert_eq!(rows["pageInfo"]["totalCount"], 2);
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn denied_target_is_indistinguishable_from_absent_even_under_negation() {
    let present = setup_options(false, true).await;
    let absent = setup_options(false, false).await;
    for negate in [false, true] {
        let filter = if negate {
            json!({f("not"): related("denied")})
        } else {
            related("denied")
        };
        assert_eq!(
            list(&present, filter.clone(), 10).await,
            list(&absent, filter, 10).await
        );
    }
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn empty_match_list_matches_no_rows() {
    let schema = setup().await;
    let rows = list(&schema, json!({f("links"): []}), 10).await;
    assert_eq!(rows["edges"], json!([]));
    assert_eq!(rows["pageInfo"]["totalCount"], 0);
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn link_row_policy_is_inside_exists() {
    let schema = setup().await;
    let rows = list(&schema, related("allowed"), 10).await;
    assert_eq!(
        rows["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["node"][f("id")].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn keyset_continues_with_same_filter_and_rejects_changed_filter() {
    let schema = setup().await;
    let query = format!(
        "query($w: FilterParentWhereInput!, $after: String) {{ rows: {}({}: $w, {}: {{limit: 1, after: $after, includeTotalCount: true}}) {{ {} }} }}",
        r("filterParentsKeyset"),
        a("where"),
        a("page"),
        selection()
    );
    let execute = |filter, after| {
        schema.execute(async_graphql::Request::new(&query).variables(
            async_graphql::Variables::from_json(json!({"w": filter, "after": after})),
        ))
    };
    let first = execute(related("allowed"), Value::Null).await;
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let first = first.data.into_json().unwrap();
    assert_eq!(first["rows"]["pageInfo"]["totalCount"], 2);
    let cursor = first["rows"]["pageInfo"]["endCursor"].clone();
    let second = execute(related("allowed"), cursor.clone()).await;
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(
        second.data.into_json().unwrap()["rows"]["edges"][0]["node"][f("id")],
        "c"
    );
    assert!(!execute(related("denied"), cursor).await.errors.is_empty());
    let legacy_cursor = graphql_orm::graphql::pagination::encode_keyset_cursor(
        FilterParent::KEYSET_FINGERPRINT,
        vec![KeysetValue::String("b".into())],
    );
    assert!(
        !execute(related("allowed"), json!(legacy_cursor))
            .await
            .errors
            .is_empty()
    );
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn aggregate_count_root_honors_related_filter() {
    let schema = setup().await;
    let query = format!(
        "query($w: FilterParentWhereInput!) {{ rows: {}({}: $w, {}: [], {}: [{{{}: COUNT}}], {}: 1) {{ {} {{ {} }} }} }}",
        r("filterParentsAggregate"),
        a("where"),
        a("groupBy"),
        a("metrics"),
        f("operator"),
        a("groupLimit"),
        f("metrics"),
        f("value")
    );
    let response =
        schema
            .execute(async_graphql::Request::new(query).variables(
                async_graphql::Variables::from_json(json!({"w": related("allowed")})),
            ))
            .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["rows"][0][f("metrics")][0][f("value")],
        "2"
    );
}

#[test]
fn unprepared_relation_filter_cannot_be_used_as_a_repository_predicate() {
    let filter = FilterParentWhereInput {
        links: Some(vec![]),
        ..Default::default()
    };
    assert!(filter.validate().is_err());
    let negated = FilterParentWhereInput {
        not: Some(Box::new(filter)),
        ..Default::default()
    };
    assert!(negated.validate().is_err());
    assert_eq!(negated.to_sql_conditions().0, ["1 = 0"]);
}

#[test]
fn exists_sql_is_quoted_and_parameterized_for_the_selected_backend() {
    let value = "untrusted' OR 1=1 --";
    let target = ReadPredicate::from_filter::<Backend, _>(&FilterTargetWhereInput {
        id: Some(StringFilter {
            eq: Some(value.into()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    let predicate =
        ReadPredicate::related::<Backend, FilterLink, FilterTarget>(&[("target_id", "id")], target)
            .unwrap();
    let query = EntityQuery::<FilterLink, Backend>::new()
        .with_read_visibility(&ReadVisibility::Complete(predicate))
        .unwrap();
    assert_eq!(query.values, vec![SqlValue::String(value.into())]);
    let sql = query.where_clauses.join(" AND ");
    assert!(sql.contains("EXISTS (SELECT 1 FROM"), "{sql}");
    assert!(!sql.contains(value), "{sql}");
    #[cfg(feature = "mssql")]
    assert!(
        sql.contains("[filter_targets]") && sql.contains("@P1"),
        "{sql}"
    );
    #[cfg(feature = "postgres")]
    assert!(
        sql.contains("\"filter_targets\"") && sql.contains("$1"),
        "{sql}"
    );
    #[cfg(feature = "sqlite")]
    assert!(
        sql.contains("\"filter_targets\"") && sql.contains('?'),
        "{sql}"
    );
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn callback_only_target_authority_fails_closed() {
    let schema = setup_policy(true).await;
    let query = format!(
        "query($w: FilterParentWhereInput!) {{ {}({}: $w) {{ {} }} }}",
        r("filterParents"),
        a("where"),
        selection()
    );
    let response =
        schema
            .execute(async_graphql::Request::new(query).variables(
                async_graphql::Variables::from_json(json!({"w": related("allowed")})),
            ))
            .await;
    assert!(!response.errors.is_empty());
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn filter_budget_refuses_excess_depth() {
    let schema = setup().await;
    let mut filter = related("allowed");
    for _ in 0..9 {
        filter = json!({f("and"): [filter]});
    }
    let query = format!(
        "query($w: FilterParentWhereInput!) {{ {}({}: $w) {{ {} }} }}",
        r("filterParents"),
        a("where"),
        selection()
    );
    let response = schema
        .execute(
            async_graphql::Request::new(query)
                .variables(async_graphql::Variables::from_json(json!({"w": filter}))),
        )
        .await;
    assert!(!response.errors.is_empty());
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned database"
)]
async fn untrusted_filter_values_are_bound() {
    let schema = setup().await;
    let rows = list(&schema, related("allowed' OR 1=1 --"), 10).await;
    assert_eq!(rows["pageInfo"]["totalCount"], 0);
}

#[test]
fn cursor_identity_excludes_private_prepared_policy_parameters() {
    let mut filter = FilterParentWhereInput {
        links: Some(vec![]),
        ..Default::default()
    };
    let identity = filter.keyset_filter_identity().unwrap();
    filter.__gom_related.insert(
        f("links").into(),
        (
            vec!["EXISTS (private policy)".into()],
            vec![SqlValue::String("private-policy-value".into())],
        ),
    );
    assert_eq!(filter.keyset_filter_identity().unwrap(), identity);
    assert!(!identity.contains("private-policy-value"));
    assert!(!identity.contains("__gom_related"));
}

fn prepared_filter() -> FilterParentWhereInput {
    let mut filter = FilterParentWhereInput {
        links: Some(vec![]),
        ..Default::default()
    };
    filter
        .__gom_related
        .insert(f("links").into(), (vec!["1 = 1".into()], vec![]));
    filter
}

#[test]
fn related_filters_never_match_in_memory() {
    let entity = FilterParent {
        id: "a".into(),
        label: "a".into(),
        links: vec![],
    };
    for prepared in [false, true] {
        let filter = if prepared {
            prepared_filter()
        } else {
            FilterParentWhereInput {
                links: Some(vec![]),
                ..Default::default()
            }
        };
        for filter in [
            filter.clone(),
            FilterParentWhereInput {
                not: Some(Box::new(filter.clone())),
                ..Default::default()
            },
            FilterParentWhereInput {
                or: Some(vec![FilterParentWhereInput::default(), filter]),
                ..Default::default()
            },
        ] {
            assert!(!filter.matches_entity(&entity).unwrap());
        }
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn bulk_writes_refuse_prepared_relations_at_every_entry_point() {
    let db = Database::<Backend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "bulk",
            "owned",
            &[
                FilterParent::metadata(),
                FilterLink::metadata(),
                FilterTarget::metadata(),
            ],
        )
        .await
        .unwrap();
    db.schema()
        .apply_migration(&plan, Default::default())
        .await
        .unwrap();
    FilterParent::insert(
        &db,
        CreateFilterParentInput {
            id: "a".into(),
            label: "original".into(),
        },
    )
    .await
    .unwrap();
    let input = || UpdateFilterParentInput {
        label: Some("changed".into()),
    };
    for filter in [
        FilterParentWhereInput {
            links: Some(vec![]),
            ..Default::default()
        },
        prepared_filter(),
        FilterParentWhereInput {
            and: Some(vec![prepared_filter()]),
            ..Default::default()
        },
        FilterParentWhereInput {
            or: Some(vec![prepared_filter()]),
            ..Default::default()
        },
        FilterParentWhereInput {
            not: Some(Box::new(prepared_filter())),
            ..Default::default()
        },
    ] {
        assert!(
            FilterParent::update_where(&db, filter.clone(), input())
                .await
                .is_err()
        );
        assert!(
            FilterParent::delete_where(&db, filter.clone())
                .await
                .is_err()
        );
        assert!(
            FilterParent::update_where_bounded(
                &db,
                filter.clone(),
                input(),
                MutationLimit::new(5).unwrap()
            )
            .await
            .is_err()
        );
        assert!(
            FilterParent::delete_where_bounded(&db, filter.clone(), MutationLimit::new(5).unwrap())
                .await
                .is_err()
        );
        let mut context = MutationContext::<Backend>::begin(&db, TransactionMode::StateMachine)
            .await
            .unwrap();
        assert!(
            context
                .update_where::<FilterParent>(filter.clone(), input())
                .await
                .is_err()
        );
        assert!(
            context
                .delete_where::<FilterParent>(filter.clone())
                .await
                .is_err()
        );
        assert!(
            context
                .update_where_bounded::<FilterParent>(
                    filter.clone(),
                    input(),
                    MutationLimit::new(5).unwrap()
                )
                .await
                .is_err()
        );
        assert!(
            context
                .delete_where_bounded::<FilterParent>(filter, MutationLimit::new(5).unwrap())
                .await
                .is_err()
        );
        context.rollback().await.unwrap();
    }
    let rows = EntityQuery::<FilterParent, Backend>::new()
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "original");
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn graphql_bulk_writes_report_relation_refusal() {
    let schema = setup().await;
    for op in ["updateFilterParents", "deleteFilterParents"] {
        for filter in [
            related("allowed"),
            json!({f("and"): [related("allowed")]}),
            json!({f("or"): [related("allowed")]}),
            json!({f("not"): related("allowed")}),
        ] {
            let input = if op.starts_with("update") {
                format!(", {}: {{{}: \"changed\"}}", a("input"), f("label"))
            } else {
                String::new()
            };
            let request = async_graphql::Request::new(format!(
                "mutation($w: FilterParentWhereInput!) {{ result: {}({}: $w{input}) {{ {} {} {} }} }}",
                r(op),
                a("where"),
                f("success"),
                f("error"),
                f("errorCode")
            ))
            .variables(async_graphql::Variables::from_json(json!({"w": filter})));
            let response = schema.execute(request).await;
            // The same channel as every other bulk refusal: the payload, not a top-level error.
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let data = response.data.into_json().unwrap();
            assert_eq!(data["result"][f("success")], false);
            assert!(
                data["result"][f("error")]
                    .as_str()
                    .is_some_and(|message| message.contains("Related-row filters")),
                "{data}"
            );
            assert_eq!(data["result"][f("errorCode")], "INVALID_INPUT");
        }
    }
    assert_eq!(
        list(&schema, json!({}), 10).await["pageInfo"]["totalCount"],
        4
    );
}
