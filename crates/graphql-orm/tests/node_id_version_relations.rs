#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
))]
//! One entity with a generated `NodeId`, a managed `RecordVersion` and a
//! generated collection relation.
//!
//! The supported declaration is a `SimpleObject` with `#[graphql(complex)]`
//! beside `GraphQLEntity`, `GraphQLOperations` and `GraphQLRelations`. The
//! relation derive then adds `NodeId`, `RecordVersion` and the relation fields
//! to that complex object. `ProbeChild` is a plain generated object with the
//! same identity and version declarations, and no relations.
//!
//! What is proven where:
//!
//! - Here: this exact combination with a `multiple` relation, a `row_version`
//!   managed version, `generated_mutations: "none"`, SDL, row-policy denial of
//!   relation targets and of `NodeId` lookups, batching, `propagate_change`
//!   `up` and `none`, and the `Changed` relation refusal. The CI Pascal-case
//!   lane runs this file too, so the same declaration is checked under
//!   `resolver-case-pascal`, `argument-case-pascal` and `field-case-pascal`.
//! - `node_id_relations.rs`: single and composite-key relations on complex
//!   entities, the `compose_complex_object` layout, field-policy revocation of
//!   `NodeId` mid-stream, and the manual `RecordVersion` workaround.
//!
//! SQLite executes the scenarios on temporary in-memory databases. PostgreSQL
//! and SQL Server compile the declarations; their generated code is the same.
//!
//! The ORM query counter is process-global and counts every statement in the
//! binary, so every test here holds `QUERY_COUNT_LOCK` for its whole run.

#[path = "support/graphql_case.rs"]
mod graphql_case;

#[cfg(feature = "sqlite")]
use graphql_case::{argument_name as a, field_name as f, resolver_name as r};
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

#[cfg(feature = "sqlite")]
static QUERY_COUNT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

macro_rules! backend_attrs {
    ($table:literal, $plural:literal, $(#[$meta:meta])* $vis:vis struct $name:ident { $($body:tt)* }) => {
        $(#[$meta])*
        #[cfg_attr(
            feature = "sqlite",
            graphql_entity(
                backend = "sqlite",
                schema_policy = "managed",
                node_id,
                table = $table,
                plural = $plural,
                default_sort = "id ASC",
                keyset = "id asc"
            )
        )]
        #[cfg_attr(
            feature = "postgres",
            graphql_entity(
                backend = "postgres",
                schema_policy = "managed",
                node_id,
                table = $table,
                plural = $plural,
                default_sort = "id ASC",
                keyset = "id asc"
            )
        )]
        #[cfg_attr(
            feature = "mssql",
            graphql_entity(
                backend = "mssql",
                schema_policy = "external_writable",
                node_id,
                table = $table,
                plural = $plural,
                default_sort = "id ASC",
                keyset = "id asc"
            )
        )]
        $vis struct $name { $($body)* }
    };
}

// A plain generated object: `NodeId` and `RecordVersion` without relations.
backend_attrs! {
    "probe_children", "ProbeChildren",
    #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[graphql(rename_fields = "PascalCase")]
    pub struct ProbeChild {
        #[primary_key]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        pub parent_id: String,
        #[graphql_orm(version = "managed", default = "0")]
        pub row_version: i64,
    }
}

// The supported declaration: the generated relation, `NodeId` and
// `RecordVersion` join the complex object of a `SimpleObject`.
backend_attrs! {
    "probe_parents", "ProbeParents",
    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        GraphQLRelations,
        SimpleObject,
        Clone,
        Debug,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql(complex, rename_fields = "PascalCase")]
    pub struct ProbeParent {
        pub label: String,
        #[primary_key]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql_orm(version = "managed", default = "0")]
        pub row_version: i64,
        #[graphql(skip)]
        #[serde(default)]
        #[relation(
            target = "ProbeChild",
            from = "id",
            to = "parent_id",
            multiple,
            emit_fk = false,
            propagate_change = "none"
        )]
        pub children: Vec<ProbeChild>,
    }
}

// A child that propagates its changes up to the parent's `Changed` stream.
backend_attrs! {
    "probe_notes", "ProbeNotes",
    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        GraphQLRelations,
        SimpleObject,
        Clone,
        Debug,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql(complex, rename_fields = "PascalCase")]
    pub struct ProbeNote {
        #[primary_key]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        pub parent_id: String,
        #[graphql_orm(version = "managed", default = "0")]
        pub row_version: i64,
        #[graphql(skip)]
        #[serde(default)]
        #[relation(
            target = "ProbeParent",
            from = "parent_id",
            to = "id",
            emit_fk = false,
            propagate_change = "up"
        )]
        pub parent: Option<ProbeParent>,
    }
}

macro_rules! batch_by_id {
    ($($entity:ty),+) => {$(
        impl BatchLoadEntity for $entity {
            fn batch_column() -> &'static str {
                "id"
            }
            fn batch_key_from_row(
                row: &graphql_orm::DbRow,
            ) -> Result<String, graphql_orm::sqlx::Error> {
                #[cfg(not(feature = "mssql"))]
                use graphql_orm::sqlx::Row;
                row.try_get("id")
            }
        }
    )+};
}
batch_by_id!(ProbeChild, ProbeParent);

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    generated_mutations: "none",
    entities: [ProbeParent, ProbeChild, ProbeNote],
}

#[cfg(feature = "sqlite")]
mod runtime {
    use super::*;
    use graphql_orm::async_graphql::{Request, Response};
    use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use std::task::Poll;
    use std::time::Duration;

    /// Hides one relation target: either every `ProbeChild` or one by id.
    #[derive(Clone, Default)]
    struct Denied(Arc<Mutex<Option<&'static str>>>);
    impl RowPolicy for Denied {
        fn can_read_row<'a>(
            &'a self,
            _: Option<&'a graphql_orm::async_graphql::Context<'_>>,
            _: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
            row: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>>
        {
            Box::pin(async move {
                let denied = *self.0.lock().unwrap();
                Ok(match (denied, row.downcast_ref::<ProbeChild>()) {
                    (Some(id), Some(child)) => child.id != id,
                    _ => true,
                })
            })
        }
        fn can_write_row<'a>(
            &'a self,
            _: Option<&'a graphql_orm::async_graphql::Context<'_>>,
            _: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
            _: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>>
        {
            Box::pin(async { Ok(true) })
        }
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn database() -> Result<Database, Box<dyn std::error::Error>> {
        let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
        for table in ["probe_children", "probe_notes"] {
            graphql_orm::sqlx::query(&format!(
                "CREATE TABLE {table} (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL, row_version INTEGER NOT NULL DEFAULT 0)"
            ))
            .execute(db.pool())
            .await?;
        }
        graphql_orm::sqlx::query("CREATE TABLE probe_parents (id TEXT PRIMARY KEY, label TEXT NOT NULL, row_version INTEGER NOT NULL DEFAULT 0)")
            .execute(db.pool())
            .await?;
        Ok(db)
    }

    async fn sql(db: &Database, statement: &str) -> TestResult {
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await?;
        Ok(())
    }

    /// Spell the connection and event members the way `field-case-*` generates
    /// them. Entity members are fixed Pascal case by `rename_fields`.
    fn request(query: impl Into<String>) -> Request {
        let query = query
            .into()
            .replace("edges {", &format!("{} {{", f("edges")))
            .replace("node {", &format!("{} {{", f("node")))
            .replace(
                "action changeKind sourceEntity",
                &format!("{} {} {}", f("action"), f("changeKind"), f("sourceEntity")),
            );
        Request::new(query).data(graphql_orm::graphql::auth::AuthSubject::new("test-user"))
    }

    fn object<'a>(sdl: &'a str, name: &str) -> &'a str {
        sdl.split(&format!("type {name} "))
            .nth(1)
            .unwrap_or_else(|| panic!("no type {name} in\n{sdl}"))
            .split("\n}")
            .next()
            .unwrap()
    }

    async fn pending<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
        poll_fn(|cx| match stream.poll_next_unpin(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(value) => panic!("subscription answered before a write: {value:?}"),
        })
        .await;
    }

    async fn next<S: Stream<Item = Response> + Unpin>(stream: &mut S) -> Value {
        let response = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("event within five seconds")
            .expect("open stream");
        assert!(response.errors.is_empty(), "{response:?}");
        response.data.into_json().unwrap()
    }

    async fn silent<S: Stream<Item = Response> + Unpin>(stream: &mut S) -> bool {
        tokio::time::timeout(Duration::from_millis(250), stream.next())
            .await
            .is_err()
    }

    #[tokio::test]
    async fn sdl_exports_identity_version_relation_and_persisted_scalars() -> TestResult {
        let _guard = QUERY_COUNT_LOCK.lock().await;
        let schema = schema_builder(database().await?).finish();
        let sdl = schema.sdl();
        let federated = schema
            .sdl_with_options(graphql_orm::async_graphql::SDLExportOptions::new().federation());
        for name in ["ProbeParent", "ProbeChild", "ProbeNote"] {
            let body = object(&sdl, name);
            assert!(body.contains("NodeId: ID!"), "{name}: {body}");
            assert!(body.contains("RecordVersion: Int!"), "{name}: {body}");
            assert!(body.contains("Id: String!"), "{name}: {body}");
            assert!(body.contains("RowVersion: Int!"), "{name}: {body}");
            assert!(
                object(&federated, name).contains("@key(fields: \"NodeId\")"),
                "{name}"
            );
        }
        let parent = object(&sdl, "ProbeParent");
        assert!(parent.contains("Label: String!"), "{parent}");
        assert!(parent.contains("Children("), "{parent}");
        assert!(parent.contains("ProbeChildConnection!"), "{parent}");
        assert!(
            !object(&sdl, "ProbeChild").contains("Children"),
            "no relations here"
        );
        assert!(
            object(&sdl, "ProbeNote").contains("Parent: ProbeParent"),
            "{sdl}"
        );

        // The managed version is declared once for the catalogue.
        assert_eq!(
            <ProbeParent as Entity>::managed_version_field(),
            Some("RowVersion")
        );
        assert_eq!(
            <ProbeChild as Entity>::managed_version_field(),
            Some("RowVersion")
        );
        // Generated queries and subscriptions exist; generated mutations do not.
        for root in [
            r("probeParents"),
            r("probeParent"),
            r("probeChildren"),
            r("probeChild"),
        ] {
            assert!(object(&sdl, "Query").contains(root), "{root}");
        }
        assert!(object(&sdl, "Subscription").contains(r("probeParentChanged")));
        assert!(!sdl.contains("type Mutation"), "{sdl}");
        assert!(!sdl.to_lowercase().contains("createprobeparent"), "{sdl}");
        Ok(())
    }

    async fn seed(db: &Database, parents: usize) -> TestResult {
        for p in 0..parents {
            sql(
                db,
                &format!("INSERT INTO probe_parents VALUES ('p{p}', 'label {p}', 3)"),
            )
            .await?;
            for c in 0..2 {
                sql(
                    db,
                    &format!("INSERT INTO probe_children VALUES ('p{p}c{c}', 'p{p}', 7)"),
                )
                .await?;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn identity_version_and_relation_resolve_together() -> TestResult {
        let _guard = QUERY_COUNT_LOCK.lock().await;
        let db = database().await?;
        seed(&db, 1).await?;
        let schema = schema_builder(db).finish();
        let parent_id =
            graphql_orm::graphql::federation::encode_node_id("ProbeParent", vec![json!("p0")]);
        let query = format!(
            r#"{{ _entities(representations: [{{__typename: "ProbeParent", NodeId: "{}"}}]) {{
                ... on ProbeParent {{ NodeId RecordVersion RowVersion Label
                    Children({}: {{ Id: ASC }}) {{ edges {{ node {{ Id NodeId RecordVersion }} }} }} }} }} }}"#,
            parent_id.as_str(),
            a("orderBy")
        );
        let response = schema.execute(request(query)).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let entity = &response.data.into_json()?["_entities"][0];
        assert_eq!(entity["NodeId"], parent_id.as_str());
        assert_eq!(entity["RecordVersion"], 3);
        assert_eq!(entity["RowVersion"], 3);
        assert_eq!(entity["Label"], "label 0");
        let edges = entity["Children"][f("edges")].as_array().unwrap();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0][f("node")]["Id"], "p0c0");
        assert_eq!(edges[0][f("node")]["RecordVersion"], 7);
        assert_eq!(
            edges[1][f("node")]["NodeId"],
            graphql_orm::graphql::federation::encode_node_id("ProbeChild", vec![json!("p0c1")])
                .as_str()
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_denied_relation_target_is_not_returned_or_reachable_by_node_id() -> TestResult {
        let _guard = QUERY_COUNT_LOCK.lock().await;
        let db = database().await?;
        seed(&db, 2).await?;
        let denied = Denied::default();
        let mut db = db;
        db.set_row_policy(denied.clone());
        let db = db.with_authorized_scan_config(AuthorizedScanConfig::new([17; 32], 8, 64));
        let schema = schema_builder(db).finish();
        let children = |id: &str| {
            format!(
                r#"{{ {}({}: "{id}") {{ Id Children {{ edges {{ node {{ Id }} }} }} }} }}"#,
                r("probeParent"),
                a("id")
            )
        };
        let ids = |value: &Value, root: &str| -> Vec<String> {
            value[root]["Children"][f("edges")]
                .as_array()
                .unwrap()
                .iter()
                .map(|edge| edge[f("node")]["Id"].as_str().unwrap().to_owned())
                .collect()
        };
        let all = schema.execute(request(children("p0"))).await;
        assert!(all.errors.is_empty(), "{all:?}");
        assert_eq!(
            ids(&all.data.into_json()?, r("probeParent")),
            ["p0c0", "p0c1"]
        );

        *denied.0.lock().unwrap() = Some("p0c1");
        let some = schema.execute(request(children("p0"))).await;
        assert!(some.errors.is_empty(), "{some:?}");
        assert_eq!(ids(&some.data.into_json()?, r("probeParent")), ["p0c0"]);

        // The same target is also absent from a list read of the parents.
        let list = schema
            .execute(request(format!(
                "{{ {}({}: {{ Id: ASC }}) {{ edges {{ node {{ Id Children {{ edges {{ node {{ Id }} }} }} }} }} }} }}",
                r("probeParents"),
                a("orderBy")
            )))
            .await;
        assert!(list.errors.is_empty(), "{list:?}");
        let list = list.data.into_json()?;
        let listed: Vec<&str> = list[r("probeParents")][f("edges")][0][f("node")]["Children"]
            [f("edges")]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge[f("node")]["Id"].as_str().unwrap())
        .collect();
        assert_eq!(listed, ["p0c0"]);

        // The denied child is not reachable by its NodeId or by its key; an
        // allowed sibling still is, and a denied row looks like a missing one.
        let lookup = |child: &str| {
            let id =
                graphql_orm::graphql::federation::encode_node_id("ProbeChild", vec![json!(child)]);
            format!(
                r#"{{ _entities(representations: [{{__typename: "ProbeChild", NodeId: "{}"}}]) {{ ... on ProbeChild {{ NodeId RecordVersion }} }} }}"#,
                id.as_str()
            )
        };
        let by_node = schema.execute(request(lookup("p0c1"))).await;
        assert!(by_node.errors.is_empty(), "{by_node:?}");
        assert!(by_node.data.into_json()?["_entities"][0].is_null());
        let by_key = schema
            .execute(request(format!(
                r#"{{ {}({}: "p0c1") {{ Id }} }}"#,
                r("probeChild"),
                a("id")
            )))
            .await;
        assert!(by_key.errors.is_empty(), "{by_key:?}");
        assert!(by_key.data.into_json()?[r("probeChild")].is_null());
        let allowed = schema.execute(request(lookup("p0c0"))).await;
        assert!(allowed.errors.is_empty(), "{allowed:?}");
        assert_eq!(
            allowed.data.into_json()?["_entities"][0]["RecordVersion"],
            7
        );
        Ok(())
    }

    #[tokio::test]
    async fn relation_loading_stays_batched_as_parents_grow() -> TestResult {
        let _guard = QUERY_COUNT_LOCK.lock().await;
        let db = database().await?;
        seed(&db, 9).await?;
        let schema = schema_builder(db).finish();
        let mut counts = Vec::new();
        for parents in [1, 9] {
            let representations = (0..parents)
                .map(|p| {
                    let id = graphql_orm::graphql::federation::encode_node_id(
                        "ProbeParent",
                        vec![json!(format!("p{p}"))],
                    );
                    format!(
                        r#"{{__typename: "ProbeParent", NodeId: "{}"}}"#,
                        id.as_str()
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let queries = [
                format!(
                    "{{ _entities(representations: [{representations}]) {{ ... on ProbeParent {{ NodeId Children {{ edges {{ node {{ Id }} }} }} }} }} }}"
                ),
                format!(
                    "{{ {}({}: {{ {}: {parents} }}) {{ edges {{ node {{ Id Children {{ edges {{ node {{ Id }} }} }} }} }} }} }}",
                    r("probeParents"),
                    a("page"),
                    f("limit")
                ),
            ];
            for (surface, query) in queries.into_iter().enumerate() {
                graphql_orm::graphql::orm::reset_query_count();
                let response = schema.execute(request(query)).await;
                let statements = graphql_orm::graphql::orm::query_count();
                assert!(response.errors.is_empty(), "{response:?}");
                let value = response.data.into_json()?;
                let rows = if surface == 0 {
                    value["_entities"].as_array().unwrap().len()
                } else {
                    value[r("probeParents")][f("edges")]
                        .as_array()
                        .unwrap()
                        .len()
                };
                assert_eq!(rows, parents);
                counts.push((surface, parents, statements));
            }
        }
        for surface in 0..2 {
            let small = counts
                .iter()
                .find(|c| c.0 == surface && c.1 == 1)
                .unwrap()
                .2;
            let large = counts
                .iter()
                .find(|c| c.0 == surface && c.1 == 9)
                .unwrap()
                .2;
            assert_eq!(
                small, large,
                "surface {surface}: relation reads must not grow with the parent count: {counts:?}"
            );
            assert!(small <= 3, "surface {surface}: {counts:?}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn propagate_change_up_reaches_the_parent_and_none_stays_put() -> TestResult {
        let _guard = QUERY_COUNT_LOCK.lock().await;
        let db = database().await?;
        seed(&db, 1).await?;
        let schema = schema_builder(db.clone()).finish();
        let changed = |root: &str, field: &str| {
            format!(
                "subscription {{ {} {{ action changeKind sourceEntity {} {{ Id RecordVersion }} }} }}",
                r(root),
                f(field)
            )
        };
        let mut parent_stream =
            Box::pin(schema.execute_stream(request(changed("probeParentChanged", "probeParent"))));
        let mut child_stream =
            Box::pin(schema.execute_stream(request(changed("probeChildChanged", "probeChild"))));
        pending(&mut parent_stream).await;
        pending(&mut child_stream).await;

        // `up`: a note change reaches the parent's stream as PROPAGATED.
        ProbeNote::insert(
            &db,
            CreateProbeNoteInput {
                parent_id: "p0".into(),
            },
        )
        .await?;
        let event = next(&mut parent_stream).await;
        let event = &event[r("probeParentChanged")];
        assert_eq!(event[f("changeKind")], "PROPAGATED", "{event}");
        assert_eq!(event[f("sourceEntity")], "ProbeNote");
        assert_eq!(event[f("probeParent")]["Id"], "p0");
        assert!(silent(&mut child_stream).await, "a note is not a child");

        // `none`: a direct parent change reaches the parent, never its children.
        ProbeParent::update_by_id(
            &db,
            &"p0".to_string(),
            UpdateProbeParentInput {
                label: Some("renamed".into()),
            },
        )
        .await?;
        let event = next(&mut parent_stream).await;
        let event = &event[r("probeParentChanged")];
        assert_eq!(event[f("changeKind")], "DIRECT", "{event}");
        assert_eq!(event[f("probeParent")]["RecordVersion"], 4);
        assert!(
            silent(&mut child_stream).await,
            "propagate_change = \"none\" must not notify the relation target"
        );
        Ok(())
    }

    #[tokio::test]
    async fn changed_snapshots_serve_identity_and_refuse_relation_fields() -> TestResult {
        let _guard = QUERY_COUNT_LOCK.lock().await;
        let db = database().await?;
        seed(&db, 1).await?;
        let schema = schema_builder(db.clone()).finish();
        let selection = |inner: &str| {
            format!(
                "subscription {{ {} {{ {} {{ {inner} }} }} }}",
                r("probeParentChanged"),
                f("probeParent")
            )
        };
        let mut allowed =
            Box::pin(schema.execute_stream(request(selection("Id NodeId RecordVersion Label"))));
        pending(&mut allowed).await;
        ProbeParent::update_by_id(
            &db,
            &"p0".to_string(),
            UpdateProbeParentInput {
                label: Some("again".into()),
            },
        )
        .await?;
        let snapshot = next(&mut allowed).await;
        let snapshot = &snapshot[r("probeParentChanged")][f("probeParent")];
        assert_eq!(snapshot["Label"], "again");
        assert_eq!(snapshot["RecordVersion"], 4);
        assert_eq!(
            snapshot["NodeId"],
            graphql_orm::graphql::federation::encode_node_id("ProbeParent", vec![json!("p0")])
                .as_str()
        );

        // A relation field is refused before registration; scalars are not.
        let mut refused = Box::pin(
            schema.execute_stream(request(selection("Id Children { edges { node { Id } } }"))),
        );
        let response = refused.next().await.expect("admission answer");
        assert!(
            response.errors.iter().any(|e| e
                .message
                .contains("persisted subscribable scalar fields only")),
            "{response:?}"
        );
        Ok(())
    }
}
