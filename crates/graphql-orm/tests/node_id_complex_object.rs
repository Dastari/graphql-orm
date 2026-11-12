// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Complex `SimpleObject` entities with typed computed fields and no relations
//! serve the generated `NodeId` and managed `RecordVersion`.
//!
//! `compose_complex_object` plus `#[graphql_complex_object]` mounts the
//! generated identity fields beside the handwritten ones. `Profile` derives
//! only `GraphQLEntity`, which then owns the composed object; `Tagged` also
//! derives `GraphQLRelations` with no relation fields, which must not emit a
//! second composed object. `Shadowed` flattens a handwritten object that also
//! names `NodeId` and `RecordVersion`; the generated fields still resolve and
//! are the advertised definitions.
//!
//! The generated `NodeId` and `RecordVersion` carry their own policy checks.
//! The `SimpleObject` scalar fields do not: on this shape their column read
//! policies need `#[graphql_policy_object]`, which both entities use.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

/// A typed view of a JSON text column.
#[derive(SimpleObject, Clone, Debug, serde::Deserialize)]
pub struct TypedScope {
    kind: String,
    ids: Vec<String>,
}

fn parse_scope(json: &str) -> graphql_orm::async_graphql::Result<TypedScope> {
    serde_json::from_str(json).map_err(|error| error.to_string().into())
}

/// A handwritten flattened object whose fields collide with generated ones.
#[derive(SimpleObject)]
pub struct ShadowFields {
    #[graphql(name = "NodeId")]
    node_id: String,
    #[graphql(name = "RecordVersion")]
    record_version: String,
}

macro_rules! entity {
    ($name:ident, $table:literal, $plural:literal, [$($derives:tt)*] $($extra:item)*) => {
        #[graphql_policy_object]
        #[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize, $($derives)*)]
        #[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = $table, plural = $plural, node_id, auth = "none"))]
        #[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = $table, plural = $plural, node_id, auth = "none"))]
        #[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", schema_policy = "external_writable", table = $table, plural = $plural, node_id, auth = "none"))]
        #[graphql(complex)]
        #[graphql_orm(compose_complex_object)]
        pub struct $name {
            #[primary_key]
            #[sortable]
            #[graphql_orm(auto_generated = false)]
            id: String,
            scope_json: String,
            #[graphql_orm(version = "managed", default = "0", read_policy = "version.read")]
            version: i64,
        }

        #[graphql_complex_object]
        impl $name {
            /// The typed computed field.
            async fn scope(&self) -> graphql_orm::async_graphql::Result<TypedScope> {
                parse_scope(&self.scope_json)
            }

            $($extra)*
        }
    };
}

entity!(Profile, "complex_profiles", "Profiles", []);
entity!(Tagged, "complex_tagged", "TaggedRows", [GraphQLRelations]);
entity!(
    Shadowed,
    "complex_shadowed",
    "ShadowedRows",
    []
    #[graphql(flatten)]
    async fn shadow(&self) -> ShadowFields {
        ShadowFields {
            node_id: "handwritten".to_string(),
            record_version: "handwritten".to_string(),
        }
    }
);

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    auth: "none",
    query_custom_ops: [],
    entities: [Profile, Tagged, Shadowed],
}

#[cfg(feature = "sqlite")]
struct DenyField(
    std::sync::Arc<std::sync::Mutex<Option<&'static str>>>,
    /// Number of refused `id` reads, so a test can wait for a refusal.
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
);
#[cfg(feature = "sqlite")]
impl FieldPolicy for DenyField {
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
            if field == "version" {
                let record = record.expect("version policy receives the entity");
                assert!(
                    record.is::<Profile>() || record.is::<Tagged>() || record.is::<Shadowed>(),
                    "the composed wrapper must hand the policy the entity itself"
                );
            }
            let refused = *self.0.lock().unwrap() == Some(field);
            if refused && field == "id" {
                self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(!refused)
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
#[tokio::test]
async fn complex_object_serves_node_id_record_version_and_computed_fields()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::futures::{StreamExt, future::poll_fn};
    use std::task::Poll;

    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    for table in ["complex_profiles", "complex_tagged", "complex_shadowed"] {
        graphql_orm::sqlx::query(&format!(
            "CREATE TABLE {table} (id TEXT PRIMARY KEY, scope_json TEXT NOT NULL, version INTEGER NOT NULL)"
        ))
        .execute(db.pool())
        .await?;
        graphql_orm::sqlx::query(&format!(
            r#"INSERT INTO {table} VALUES ('a', '{{"kind":"team","ids":["x","y"]}}', 3)"#
        ))
        .execute(db.pool())
        .await?;
    }
    let denied = std::sync::Arc::new(std::sync::Mutex::new(None));
    let refused_keys = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    db.set_field_policy(DenyField(denied.clone(), refused_keys.clone()));
    let schema = schema_builder(db.clone()).finish();

    let sdl =
        schema.sdl_with_options(graphql_orm::async_graphql::SDLExportOptions::new().federation());
    for name in ["Profile", "Tagged", "Shadowed"] {
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
        assert!(object.contains("scope: TypedScope!"), "{object}");
        assert!(object.contains("scopeJson: String!"), "{object}");
    }
    assert_eq!(
        <Profile as Entity>::managed_version_field(),
        Some("version")
    );
    assert_eq!(<Tagged as Entity>::managed_version_field(), Some("version"));

    for (kind, field) in [
        ("Profile", "profile"),
        ("Tagged", "tagged"),
        ("Shadowed", "shadowed"),
    ] {
        let identity =
            graphql_orm::graphql::federation::encode_node_id(kind, vec![serde_json::json!("a")]).0;
        let selection = "NodeId RecordVersion scope { kind ids }";

        let response = schema
            .execute(format!(r#"{{ {field}(id: "a") {{ {selection} }} }}"#))
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        let value = response.data.into_json()?;
        assert_eq!(value[field]["NodeId"], identity.as_str());
        assert_eq!(value[field]["RecordVersion"], 3);
        assert_eq!(value[field]["scope"]["kind"], "team");
        assert_eq!(value[field]["scope"]["ids"], serde_json::json!(["x", "y"]));

        // The `NodeId` key is resolvable: this is the lookup a router's
        // `node(id:)` turns into.
        let lookup = format!(
            r#"{{ _entities(representations: [{{__typename: "{kind}", NodeId: "{identity}"}}]) {{ ... on {kind} {{ {selection} }} }} }}"#
        );
        let response = schema.execute(lookup.clone()).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let value = response.data.into_json()?;
        assert_eq!(value["_entities"][0]["NodeId"], identity.as_str());
        assert_eq!(value["_entities"][0]["RecordVersion"], 3);
        assert_eq!(value["_entities"][0]["scope"]["kind"], "team");

        // `RecordVersion` and, through `graphql_policy_object`, the `version`
        // column itself keep the declared column policy; the computed field and
        // `NodeId` stay readable beside them.
        *denied.lock().unwrap() = Some("version");
        for selected in ["RecordVersion", "version"] {
            let refused = schema
                .execute(format!(r#"{{ {field}(id: "a") {{ {selected} }} }}"#))
                .await;
            assert!(
                !refused.errors.is_empty(),
                "{selected} was served despite the version column policy"
            );
        }
        let allowed = schema
            .execute(format!(
                r#"{{ {field}(id: "a") {{ NodeId scopeJson scope {{ kind }} }} }}"#
            ))
            .await;
        assert!(allowed.errors.is_empty(), "{allowed:?}");
        assert_eq!(
            allowed.data.into_json()?[field]["NodeId"],
            identity.as_str()
        );
        *denied.lock().unwrap() = None;
        let allowed = schema
            .execute(format!(r#"{{ {field}(id: "a") {{ version }} }}"#))
            .await;
        assert!(allowed.errors.is_empty(), "{allowed:?}");
        assert_eq!(allowed.data.into_json()?[field]["version"], 3);

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
                r#"mutation {{ update{kind}(id: "a", input: {{ scopeJson: "{{\"kind\":\"site\",\"ids\":[]}}" }}) {{ success }} }}"#
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
        assert_eq!(event[format!("{field}Changed")][field]["RecordVersion"], 4);

        // A `Changed` snapshot selecting `NodeId` still proves the encoded key
        // through its field policy. The stream authorizes when it is polled, so
        // keep the key denied and poll until the policy has refused it; no frame
        // with data may arrive meanwhile.
        let refusals = refused_keys.load(std::sync::atomic::Ordering::SeqCst);
        *denied.lock().unwrap() = Some("id");
        let response = schema
            .execute(format!(
                r#"mutation {{ update{kind}(id: "a", input: {{ scopeJson: "{{\"kind\":\"site\",\"ids\":[]}}" }}) {{ success }} }}"#
            ))
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while refused_keys.load(std::sync::atomic::Ordering::SeqCst) == refusals {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the Changed snapshot never checked the denied key"
            );
            tokio::select! {
                event = stream.next() => {
                    let event = event.expect("the subscription ended");
                    assert!(
                        !event.errors.is_empty()
                            && event.data == graphql_orm::async_graphql::Value::Null,
                        "a frame was released while the key was denied: {event:?}"
                    );
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
            }
        }
        // Positive control: the refused write is settled, so lift the denial and
        // write again. Every frame before the control's must carry no data.
        *denied.lock().unwrap() = None;
        let response = schema
            .execute(format!(
                r#"mutation {{ update{kind}(id: "a", input: {{ scopeJson: "{{\"kind\":\"site\",\"ids\":[]}}" }}) {{ success }} }}"#
            ))
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await?
                .expect("control event");
            if event.errors.is_empty() {
                let event = event.data.into_json()?;
                assert_eq!(
                    event[format!("{field}Changed")][field]["RecordVersion"],
                    6,
                    "the refused write released a snapshot: {event}"
                );
                break;
            }
            assert_eq!(
                event.data,
                graphql_orm::async_graphql::Value::Null,
                "denied key leaked through NodeId: {event:?}"
            );
        }

        let response = schema.execute(lookup).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let value = response.data.into_json()?;
        assert_eq!(value["_entities"][0]["RecordVersion"], 6);
        assert_eq!(value["_entities"][0]["scope"]["kind"], "site");
    }
    Ok(())
}
