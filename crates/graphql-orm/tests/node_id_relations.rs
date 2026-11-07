// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Relation objects retain generated identity and managed version snapshots.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

macro_rules! entity {
    ($name:ident, $table:literal, $plural:literal, [$($attrs:tt)*], [$($derives:tt)*]) => {
        #[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, Clone, Debug, serde::Serialize, serde::Deserialize, $($derives)*)]
        #[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = $table, plural = $plural, node_id, auth = "none"))]
        #[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = $table, plural = $plural, node_id, auth = "none"))]
        #[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", schema_policy = "external_writable", table = $table, plural = $plural, node_id, auth = "none"))]
        $($attrs)*
        pub struct $name {
            #[primary_key]
    #[sortable]
            #[graphql_orm(auto_generated = false)]
            id: String,
            parent_id: String,
            name: String,
            owner_id: Option<String>,
            site_id: Option<String>,
            #[graphql(skip)]
            #[relation(target = "Site", from = ["owner_id", "site_id"], to = ["owner_id", "id"], emit_fk = false)]
            site: Option<Site>,
            #[graphql_orm(version = "managed", default = "0", read_policy = "version.read")]
            version: i64,
            #[graphql(skip)]
            #[relation(target = "Parent", from = "parent_id", to = "id", emit_fk = false, propagate_change = "up")]
            parent: Option<Parent>,
        }
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "identity_parents",
        plural = "IdentityParents",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "identity_parents",
        plural = "IdentityParents",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "identity_parents",
        plural = "IdentityParents",
        auth = "none"
    )
)]
pub struct Parent {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    name: String,
}
impl BatchLoadEntity for Parent {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        #[cfg(not(feature = "mssql"))]
        use graphql_orm::sqlx::Row;
        row.try_get("id")
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "identity_sites",
        plural = "IdentitySites",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "identity_sites",
        plural = "IdentitySites",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "identity_sites",
        plural = "IdentitySites",
        auth = "none"
    )
)]
pub struct Site {
    #[primary_key]
    #[sortable]
    owner_id: String,
    #[primary_key]
    id: String,
    name: String,
}
impl BatchLoadEntity for Site {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        #[cfg(not(feature = "mssql"))]
        use graphql_orm::sqlx::Row;
        row.try_get("id")
    }
}

entity!(Plain, "identity_plain", "PlainRows", [#[graphql(complex, rename_fields = "PascalCase")]], [SimpleObject]);
entity!(Composed, "identity_composed", "ComposedRows", [#[graphql(complex, rename_fields = "PascalCase")] #[graphql_orm(compose_complex_object)]], [SimpleObject]);

#[graphql_complex_object]
impl Composed {
    async fn label(&self) -> String {
        self.name.clone()
    }
}

// Pin the existing manual RecordVersion workaround with no NodeId opt-in.
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
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "identity_legacy",
        plural = "IdentityLegacy",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "identity_legacy",
        plural = "IdentityLegacy",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "identity_legacy",
        plural = "IdentityLegacy",
        auth = "none"
    )
)]
#[graphql(complex, rename_fields = "PascalCase")]
#[graphql_orm(compose_complex_object)]
pub struct Legacy {
    name: String,
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[graphql_orm(version = "managed", default = "0")]
    version: i64,
    #[graphql(skip)]
    #[relation(target = "Parent", from = "id", to = "id", emit_fk = false)]
    parent: Option<Parent>,
}
#[graphql_complex_object]
impl Legacy {
    #[graphql(name = "RecordVersion")]
    async fn record_version(&self) -> i64 {
        self.version
    }
}

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    auth: "none",
    query_custom_ops: [],
    entities: [Parent, Site, Plain, Composed, Legacy],
}

#[cfg(feature = "sqlite")]
struct IdentityPolicy(std::sync::Arc<std::sync::Mutex<Option<&'static str>>>);
#[cfg(feature = "sqlite")]
impl FieldPolicy for IdentityPolicy {
    fn can_read_field<'a>(
        &'a self,
        _: &'a graphql_orm::async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move {
            if field == "Version" {
                let record = record.expect("version policy receives the entity");
                assert!(record.is::<Plain>() || record.is::<Composed>());
            }
            Ok(*self.0.lock().unwrap() != Some(field))
        })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a graphql_orm::async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[cfg(feature = "sqlite")]
impl RowPolicy for IdentityPolicy {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a graphql_orm::async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move { Ok(*self.0.lock().unwrap() != Some(entity)) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a graphql_orm::async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[cfg(feature = "sqlite")]
#[derive(Clone)]
struct RevokeIdentity(
    std::sync::Arc<std::sync::Mutex<Option<&'static str>>>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
);
#[cfg(feature = "sqlite")]
impl graphql_orm::async_graphql::extensions::ExtensionFactory for RevokeIdentity {
    fn create(&self) -> std::sync::Arc<dyn graphql_orm::async_graphql::extensions::Extension> {
        std::sync::Arc::new(self.clone())
    }
}
#[cfg(feature = "sqlite")]
#[graphql_orm::async_graphql::async_trait::async_trait]
impl graphql_orm::async_graphql::extensions::Extension for RevokeIdentity {
    async fn resolve(
        &self,
        ctx: &graphql_orm::async_graphql::extensions::ExtensionContext<'_>,
        info: graphql_orm::async_graphql::extensions::ResolveInfo<'_>,
        next: graphql_orm::async_graphql::extensions::NextResolve<'_>,
    ) -> graphql_orm::async_graphql::ServerResult<Option<graphql_orm::async_graphql::Value>> {
        let revoke =
            info.name == "NodeId" && self.1.swap(false, std::sync::atomic::Ordering::SeqCst);
        let value = next.run(ctx, info).await?;
        if revoke {
            *self.0.lock().unwrap() = Some("Id");
        }
        Ok(value)
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn relation_identity_queries_and_changed_snapshots() -> Result<(), Box<dyn std::error::Error>>
{
    use graphql_orm::futures::{StreamExt, future::poll_fn};
    use std::task::Poll;
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    for sql in [
        "CREATE TABLE identity_parents (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
        "INSERT INTO identity_parents VALUES ('p', 'parent')",
        "CREATE TABLE identity_sites (owner_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, PRIMARY KEY (owner_id, id))",
        "INSERT INTO identity_sites VALUES ('owner-a', 's', 'allowed'), ('owner-b', 's', 'collision')",
        "CREATE TABLE identity_legacy (id TEXT PRIMARY KEY, version INTEGER NOT NULL, name TEXT NOT NULL)",
        "INSERT INTO identity_legacy VALUES ('p', 7, 'legacy')",
    ] {
        graphql_orm::sqlx::query(sql).execute(db.pool()).await?;
    }
    for table in ["identity_plain", "identity_composed"] {
        graphql_orm::sqlx::query(&format!("CREATE TABLE {table} (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL, name TEXT NOT NULL, owner_id TEXT, site_id TEXT, version INTEGER NOT NULL)")).execute(db.pool()).await?;
        graphql_orm::sqlx::query(&format!(
            "INSERT INTO {table} VALUES ('a', 'p', 'first', 'owner-a', 's', 1)"
        ))
        .execute(db.pool())
        .await?;
    }
    let denied = std::sync::Arc::new(std::sync::Mutex::new(None));
    db.set_field_policy(IdentityPolicy(denied.clone()));
    db.set_row_policy(IdentityPolicy(denied.clone()));
    let revoke_during_serialization =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let schema = schema_builder(db.clone())
        .extension(RevokeIdentity(
            denied.clone(),
            revoke_during_serialization.clone(),
        ))
        .finish();
    let sdl =
        schema.sdl_with_options(graphql_orm::async_graphql::SDLExportOptions::new().federation());
    for name in ["Plain", "Composed"] {
        let object = sdl
            .split(&format!("type {name} "))
            .nth(1)
            .expect("object")
            .split("\n}")
            .next()
            .unwrap();
        assert!(object.contains("@key(fields: \"NodeId\")"), "{object}");
        assert!(object.contains("NodeId: ID!"), "{object}");
        assert!(object.contains("RecordVersion: Int!"), "{object}");
        assert!(object.contains("Parent"), "{object}");
    }
    assert_eq!(<Plain as Entity>::managed_version_field(), Some("Version"));
    assert_eq!(
        <Composed as Entity>::managed_version_field(),
        Some("Version")
    );
    for index in 0..8 {
        graphql_orm::sqlx::query("INSERT INTO identity_parents VALUES (?, 'parent')")
            .bind(format!("p{index}"))
            .execute(db.pool())
            .await?;
        graphql_orm::sqlx::query(
            "INSERT INTO identity_plain (id, parent_id, name, version) VALUES (?, ?, 'batched', 1)",
        )
        .bind(format!("b{index}"))
        .bind(format!("p{index}"))
        .execute(db.pool())
        .await?;
    }
    for count in [1, 8] {
        let representations = (0..count)
            .map(|index| {
                let id = graphql_orm::graphql::federation::encode_node_id(
                    "Plain",
                    vec![serde_json::json!(format!("b{index}"))],
                );
                format!(r#"{{__typename: "Plain", NodeId: "{}"}}"#, id.as_str())
            })
            .collect::<Vec<_>>()
            .join(",");
        graphql_orm::graphql::orm::reset_query_count();
        let response = schema.execute(format!("{{ _entities(representations: [{representations}]) {{ ... on Plain {{ NodeId Parent {{ id }} }} }} }}")).await;
        let statements = graphql_orm::graphql::orm::query_count();
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(
            response.data.into_json()?["_entities"]
                .as_array()
                .unwrap()
                .len(),
            count
        );
        assert_eq!(
            statements, 2,
            "identity and relation reads must stay batched at {count} rows"
        );
    }
    for (kind, field) in [("Plain", "plain"), ("Composed", "composed")] {
        let identity =
            graphql_orm::graphql::federation::encode_node_id(kind, vec![serde_json::json!("a")]).0;
        let query = format!(
            r#"{{ _entities(representations: [{{__typename: "{kind}", NodeId: "{identity}"}}]) {{ ... on {kind} {{ NodeId RecordVersion Parent {{ id }} Site {{ name }} }} }} }}"#
        );
        let response = schema.execute(query.clone()).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let value = response.data.into_json()?;
        assert_eq!(value["_entities"][0]["NodeId"], identity.as_str());
        assert_eq!(value["_entities"][0]["RecordVersion"], 1);
        assert_eq!(value["_entities"][0]["Parent"]["id"], "p");
        assert_eq!(value["_entities"][0]["Site"]["name"], "allowed");

        for owner in [Some("missing"), None] {
            let table = if kind == "Plain" {
                "identity_plain"
            } else {
                "identity_composed"
            };
            graphql_orm::sqlx::query(&format!("UPDATE {table} SET owner_id = ? WHERE id = 'a'"))
                .bind(owner)
                .execute(db.pool())
                .await?;
            let missing = schema.execute(query.clone()).await;
            assert!(missing.errors.is_empty(), "{missing:?}");
            assert!(missing.data.into_json()?["_entities"][0]["Site"].is_null());
            graphql_orm::sqlx::query(&format!(
                "UPDATE {table} SET owner_id = 'owner-a' WHERE id = 'a'"
            ))
            .execute(db.pool())
            .await?;
        }
        *denied.lock().unwrap() = Some(kind);
        let refused = schema.execute(query.clone()).await;
        assert!(refused.errors.is_empty(), "{refused:?}");
        assert!(refused.data.into_json()?["_entities"][0].is_null());
        *denied.lock().unwrap() = Some("Parent");
        let refused = schema.execute(query.clone()).await;
        assert!(refused.data.into_json()?["_entities"][0]["Parent"].is_null());

        *denied.lock().unwrap() = Some("Version");
        let refused = schema
            .execute(format!(r#"{{ {field}(id: "a") {{ RecordVersion }} }}"#))
            .await;
        assert!(
            !refused.errors.is_empty(),
            "managed alias bypassed its column policy"
        );
        *denied.lock().unwrap() = None;

        let mut parent_stream =
            schema.execute_stream("subscription { parentChanged { parent { id } } }");
        poll_fn(|cx| match parent_stream.poll_next_unpin(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(value) => panic!("parent subscription rejected: {value:?}"),
        })
        .await;
        let mut stream = schema.execute_stream(format!(
            "subscription {{ {field}Changed {{ {field} {{ NodeId RecordVersion }} }} }}"
        ));
        poll_fn(|cx| match stream.poll_next_unpin(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(value) => panic!("subscription rejected: {value:?}"),
        })
        .await;
        let response = schema
            .execute(format!(
                r#"mutation {{ update{kind}(id: "a", input: {{ Name: "second" }}) {{ success }} }}"#
            ))
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await?
            .expect("event");
        assert!(event.errors.is_empty(), "{event:?}");
        let event = event.data.into_json()?;
        assert_eq!(
            event[format!("{field}Changed")][field]["NodeId"],
            identity.as_str()
        );
        assert_eq!(event[format!("{field}Changed")][field]["RecordVersion"], 2);
        let parent_event =
            tokio::time::timeout(std::time::Duration::from_secs(5), parent_stream.next())
                .await?
                .expect("propagated event");
        assert!(parent_event.errors.is_empty(), "{parent_event:?}");
        assert_eq!(
            parent_event.data.into_json()?["parentChanged"]["parent"]["id"],
            "p"
        );

        // NodeId still proves every encoded key through the ordinary field policy.
        *denied.lock().unwrap() = Some("Id");
        let response = schema
            .execute(format!(
                r#"mutation {{ update{kind}(id: "a", input: {{ Name: "third" }}) {{ success }} }}"#
            ))
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        if let Ok(Some(event)) =
            tokio::time::timeout(std::time::Duration::from_millis(100), stream.next()).await
        {
            assert!(
                !event.errors.is_empty(),
                "denied key leaked through NodeId: {event:?}"
            );
            assert_eq!(event.data, graphql_orm::async_graphql::Value::Null);
        }
        *denied.lock().unwrap() = None;

        // A successful NodeId resolver must not release a frame after its key
        // policy is revoked during serialization.
        revoke_during_serialization.store(true, std::sync::atomic::Ordering::SeqCst);
        let response = schema
            .execute(format!(
                r#"mutation {{ update{kind}(id: "a", input: {{ Name: "fourth" }}) {{ success }} }}"#
            ))
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "late key revocation leaked a frame"
        );
        assert_eq!(
            *denied.lock().unwrap(),
            Some("Id"),
            "serializer revocation actually ran"
        );
        *denied.lock().unwrap() = None;
        let response = schema
            .execute(format!(
                r#"mutation {{ update{kind}(id: "a", input: {{ Name: "fifth" }}) {{ success }} }}"#
            ))
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        let resumed = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await?
            .expect("resumed event");
        assert!(resumed.errors.is_empty(), "{resumed:?}");
        assert_eq!(
            resumed.data.into_json()?[format!("{field}Changed")][field]["RecordVersion"],
            5
        );
    }
    let response = schema
        .execute(r#"{ legacy(id: "p") { RecordVersion Parent { id } } }"#)
        .await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(response.data.into_json()?["legacy"]["RecordVersion"], 7);
    Ok(())
}
