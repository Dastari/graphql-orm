// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres"))]
//! Client-facing metadata batch: the opt-in `Changed` started frame (#341),
//! the opt-in aggregate source entity (#372), the client metadata export
//! (#340, #670, #672) and the interpretation fingerprint (#671).
//!
//! SQLite runs every scenario. PostgreSQL runs the database scenarios in one
//! ignored test against a test-owned container:
//!
//! ```sh
//! cargo test -p graphql-orm --no-default-features --features postgres --test client_facing_metadata -- --include-ignored
//! ```

use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt};
use graphql_orm::graphql::orm::{
    AuthorizedKeysetEntity, DefaultBackend, GraphqlClientMetadata, GraphqlLifetimeEncoding,
    GraphqlRelationshipKeyMapping, GraphqlRelationshipNotBatchableReason, InterpretationEntity,
    interpretation_fingerprint,
};
use graphql_orm::prelude::*;
use tokio::time::{Duration, timeout};

#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{argument_name, field_name, resolver_name};

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A record that opts into every client-facing schema signal.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "meta_parents",
        plural = "MetaParents",
        aggregate = true,
        aggregate_source = true,
        changed_started = true,
        lifetime(field = "expires_at", format = "unix_seconds")
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "meta_parents",
        plural = "MetaParents",
        aggregate = true,
        aggregate_source = true,
        changed_started = true,
        lifetime(field = "expires_at", format = "unix_seconds")
    )
)]
struct MetaParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    label: String,
    /// Not carried by the Changed stream.
    #[graphql_orm(subscribe = false)]
    note: String,
    #[graphql_orm(private)]
    owner_ref: String,
    expires_at: Option<i64>,
}

/// A record whose relations cover each key-mapping outcome.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "meta_children",
        plural = "MetaChildren",
        lifetime(field = "valid_until", format = "sqlite_utc_text")
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "meta_children",
        plural = "MetaChildren",
        lifetime(field = "valid_until", format = "sqlite_utc_text")
    )
)]
struct MetaChild {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    parent_id: String,
    #[graphql_orm(read_policy = "child.restricted_key")]
    hidden_parent_id: String,
    #[graphql_orm(sensitive)]
    sensitive_parent_id: String,
    kind: i32,
    valid_until: Option<String>,
    /// Batchable: public key fields on both sides.
    #[graphql(skip)]
    #[relation(target = "MetaParent", from = "parent_id", to = "id")]
    parent: Option<MetaParent>,
    /// No exposed key: the source key has a field policy.
    #[graphql(skip)]
    #[relation(
        target = "MetaParent",
        from = "hidden_parent_id",
        to = "id",
        emit_fk = false
    )]
    hidden_parent: Option<MetaParent>,
    /// No exposed key: the source key is classified secret.
    #[graphql(skip)]
    #[relation(
        target = "MetaParent",
        from = "sensitive_parent_id",
        to = "id",
        emit_fk = false
    )]
    sensitive_parent: Option<MetaParent>,
    /// Not batchable: the relation has a source condition.
    #[graphql(skip)]
    #[relation(
        target = "MetaParent",
        from = "parent_id",
        to = "id",
        source_condition(field = "kind", equals = 1),
        emit_fk = false
    )]
    kind_parent: Option<MetaParent>,
}

/// A record with no opt-in: its schema shapes stay the defaults.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "meta_plains",
        plural = "MetaPlains",
        aggregate = true,
        keyset = "label asc, id asc"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "meta_plains",
        plural = "MetaPlains",
        aggregate = true,
        keyset = "label asc, id asc"
    )
)]
struct MetaPlain {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[sortable]
    label: String,
}

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    client_metadata: true,
    query_custom_ops: [],
    entities: [MetaParent, MetaChild, MetaPlain],
}
#[cfg(feature = "postgres")]
schema_roots! {
    backend: "postgres",
    client_metadata: true,
    query_custom_ops: [],
    entities: [MetaParent, MetaChild, MetaPlain],
}

/// Whether every case feature is at its default, so the goldens apply.
fn default_case() -> bool {
    field_name("sourceEntity") == "sourceEntity"
        && argument_name("startedArg") == "startedArg"
        && resolver_name("metaParentChanged") == "metaParentChanged"
}

fn golden(name: &str) -> String {
    let path = format!("{}/tests/goldens/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

/// The SDL definition block that starts with `head`, up to its closing brace.
fn block<'a>(sdl: &'a str, head: &str) -> &'a str {
    let start = sdl
        .find(head)
        .unwrap_or_else(|| panic!("`{head}` is not in the SDL"));
    let end = sdl[start..].find("\n}").expect("block end") + start;
    &sdl[start..end]
}

/// Whether a definition block has a line that is exactly `line`, ignoring
/// indentation.
fn has_line(block: &str, line: &str) -> bool {
    block.lines().any(|candidate| candidate.trim() == line)
}

/// One root field definition, from its name to its result type.
fn root_field<'a>(sdl: &'a str, name: &str) -> &'a str {
    let head = format!("\n\t{name}(");
    let start = sdl
        .find(&head)
        .unwrap_or_else(|| panic!("root `{name}` is not in the SDL"))
        + 1;
    let close = sdl[start..].find("\n\t): ").expect("argument list end") + start;
    let end = sdl[close + 1..].find('\n').expect("line end") + close + 1;
    &sdl[start..end]
}

#[tokio::test]
async fn schema_signals_are_present_only_for_opted_in_entities() {
    let sdl = schema_builder_without_database().sdl();
    let started = argument_name("started");

    // #341: the opted-in root takes `Started`, and its frame type has it.
    let parent_root = root_field(&sdl, resolver_name("metaParentChanged"));
    assert!(
        parent_root.contains(&format!("{started}: Boolean")),
        "{parent_root}"
    );
    let parent_event = block(&sdl, "type MetaParentChangedEvent");
    assert!(
        has_line(
            parent_event,
            &format!("{}: Boolean!", field_name("started"))
        ),
        "{parent_event}"
    );
    assert!(
        has_line(
            parent_event,
            &format!("{}: ChangeAction", field_name("action"))
        ),
        "{parent_event}"
    );

    // Default: no argument, no field, the action stays non-null.
    let plain_root = root_field(&sdl, resolver_name("metaPlainChanged"));
    assert!(!plain_root.contains(&format!("{started}:")), "{plain_root}");
    let plain_event = block(&sdl, "type MetaPlainChangedEvent");
    assert!(
        !plain_event.contains(field_name("started")),
        "{plain_event}"
    );
    assert!(
        has_line(
            plain_event,
            &format!("{}: ChangeAction!", field_name("action"))
        ),
        "{plain_event}"
    );

    // #372: the opted-in aggregate root names its entity through a type.
    let parent_aggregate = root_field(&sdl, resolver_name("metaParentsAggregate"));
    assert!(
        parent_aggregate.ends_with(": [MetaParentAggregateResultRow!]!"),
        "{parent_aggregate}"
    );
    let row = block(&sdl, "type MetaParentAggregateResultRow");
    assert!(
        has_line(row, &format!("{}: MetaParent", field_name("sourceEntity"))),
        "{row}"
    );
    let plain_aggregate = root_field(&sdl, resolver_name("metaPlainsAggregate"));
    assert!(
        plain_aggregate.ends_with(": [GraphqlAggregateResultRow!]!"),
        "{plain_aggregate}"
    );
    assert!(!sdl.contains("MetaPlainAggregateResultRow"));
}

/// The schema's SDL needs no database: build it over an unconnected pool.
fn schema_builder_without_database() -> AppSchema {
    #[cfg(feature = "sqlite")]
    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .connect_lazy("sqlite::memory:")
        .expect("lazy pool");
    #[cfg(feature = "postgres")]
    let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:9/unused")
        .expect("lazy pool");
    schema_builder(Database::new(pool)).finish()
}

#[test]
fn operation_catalogue_describes_the_opted_in_shapes() {
    let catalog = graphql_orm_semantic_catalog();
    let operation = |name: &str| {
        catalog
            .operations
            .iter()
            .find(|operation| operation.field_name == name)
            .unwrap_or_else(|| panic!("operation `{name}`"))
    };
    let changed = operation(resolver_name("metaParentChanged"));
    assert!(
        changed
            .arguments
            .iter()
            .any(|argument| argument.graphql_name == argument_name("started"))
    );
    let plain = operation(resolver_name("metaPlainChanged"));
    assert!(
        !plain
            .arguments
            .iter()
            .any(|argument| argument.graphql_name == argument_name("started"))
    );
    let aggregate = operation(resolver_name("metaParentsAggregate"));
    assert_eq!(
        aggregate.generated_entity_name.as_deref(),
        Some("MetaParent")
    );
}

#[test]
fn client_metadata_export_matches_its_golden() -> TestResult {
    let metadata = graphql_orm_client_metadata();
    let catalog = graphql_orm_semantic_catalog();
    metadata.validate_against(catalog)?;
    let exported = metadata.export_json()?;
    let decoded = GraphqlClientMetadata::from_export_json(&exported)?;
    assert_eq!(&decoded, metadata);
    decoded.validate_against(catalog)?;

    // Every carried field is one the subscribe-time check admits, and the
    // private and `subscribe = false` fields are absent.
    let parent = metadata.entity("MetaParent").expect("parent facts");
    let stream = parent.changed_stream.as_ref().expect("stream facts");
    assert_eq!(stream.root_field, resolver_name("metaParentChanged"));
    assert!(stream.started_signal);
    assert!(stream.identity_carried);
    assert!(
        stream
            .carried_fields
            .contains(&field_name("label").to_owned())
    );
    assert!(
        !stream
            .carried_fields
            .contains(&field_name("note").to_owned())
    );
    assert!(
        !stream
            .carried_fields
            .iter()
            .any(|name| name.contains("wner"))
    );
    let lifetime = parent.lifetime.as_ref().expect("lifetime facts");
    assert_eq!(lifetime.field, field_name("expiresAt"));
    assert_eq!(lifetime.encoding, GraphqlLifetimeEncoding::UnixSeconds);

    let child = metadata.entity("MetaChild").expect("child facts");
    assert_eq!(
        child.lifetime.as_ref().map(|lifetime| lifetime.encoding),
        Some(GraphqlLifetimeEncoding::SqliteUtcText)
    );
    let mapping = |name: &str| {
        child
            .relationships
            .iter()
            .find(|relationship| relationship.field_name == field_name(name))
            .unwrap_or_else(|| panic!("relationship `{name}`"))
            .key_mapping
            .clone()
    };
    assert_eq!(
        mapping("parent"),
        GraphqlRelationshipKeyMapping::Batchable {
            source_fields: vec![field_name("parentId").to_owned()],
            target_fields: vec![field_name("id").to_owned()],
            target_filterable: true,
        }
    );
    assert_eq!(
        mapping("hiddenParent"),
        GraphqlRelationshipKeyMapping::NoExposedKey
    );
    assert_eq!(
        mapping("sensitiveParent"),
        GraphqlRelationshipKeyMapping::NoExposedKey
    );
    assert_eq!(
        mapping("kindParent"),
        GraphqlRelationshipKeyMapping::NotBatchable {
            reason: GraphqlRelationshipNotBatchableReason::Conditional,
        }
    );
    // The policy-guarded key is a public field, so it may appear as a carried
    // field, but no relationship mapping names it. The private field appears
    // nowhere, under any case feature.
    let relationships = serde_json::to_string(&child.relationships)?;
    assert!(!relationships.contains(field_name("hiddenParentId")));
    assert!(!relationships.contains("hidden_parent_id"));
    assert!(!relationships.contains(field_name("sensitiveParentId")));
    assert!(!relationships.contains("sensitive_parent_id"));
    assert!(!exported.contains("wner"));

    // Entities without a fact still report their stream, and nothing else.
    let plain = metadata.entity("MetaPlain").expect("plain facts");
    assert!(plain.lifetime.is_none() && plain.relationships.is_empty());
    assert!(
        !plain
            .changed_stream
            .as_ref()
            .expect("stream")
            .started_signal
    );

    if default_case() {
        // The export names public GraphQL identities only, so it is the
        // same on every backend.
        if std::env::var_os("GRAPHQL_ORM_PRINT_GOLDENS").is_some() {
            println!("{exported}");
        }
        let expected = golden("client_metadata.json");
        assert_eq!(exported, expected.trim_end(), "regenerate the golden");
    }
    Ok(())
}

#[test]
fn interpretation_fingerprint_is_stable_and_follows_its_inputs() {
    let unsalted = graphql_orm_interpretation_fingerprint(None);
    assert_eq!(unsalted.len(), 64);
    assert!(unsalted.bytes().all(|byte| byte.is_ascii_hexdigit()));
    // Stable: the same inputs give the same value.
    assert_eq!(unsalted, graphql_orm_interpretation_fingerprint(None));
    let salted = graphql_orm_interpretation_fingerprint(Some("cursor-key-2"));
    assert_ne!(salted, unsalted);
    assert_eq!(
        salted,
        graphql_orm_interpretation_fingerprint(Some("cursor-key-2"))
    );
    assert_ne!(
        salted,
        graphql_orm_interpretation_fingerprint(Some("cursor-key-3"))
    );
    // The salt never appears in the value.
    assert!(!salted.contains("cursor"));

    let dialect = if cfg!(feature = "sqlite") {
        "sqlite"
    } else {
        "postgres"
    };
    let entities = || {
        vec![
            InterpretationEntity::of::<MetaParent, DefaultBackend>(
                MetaParent::__GOM_INTERPRETATION_SOURCE,
            ),
            InterpretationEntity::of::<MetaChild, DefaultBackend>(
                MetaChild::__GOM_INTERPRETATION_SOURCE,
            ),
            InterpretationEntity::of::<MetaPlain, DefaultBackend>(
                MetaPlain::__GOM_INTERPRETATION_SOURCE,
            ),
        ]
    };
    assert_eq!(
        interpretation_fingerprint(dialect, &entities(), None),
        unsalted
    );
    // The hashed keyset input is exactly the fingerprint keyset cursors are
    // bound to, and an entity without a keyset order hashes none.
    assert_eq!(
        MetaPlain::__GOM_INTERPRETATION_SOURCE,
        format!(
            "node_id=false;keyset={}",
            <MetaPlain as AuthorizedKeysetEntity>::KEYSET_FINGERPRINT
        )
    );
    assert_eq!(
        MetaParent::__GOM_INTERPRETATION_SOURCE,
        "node_id=false;keyset="
    );
    // Each input changes the value: a keyset order change, a different
    // entity set and the backend dialect.
    let mut changed = entities();
    changed[2] = InterpretationEntity::of::<MetaPlain, DefaultBackend>(
        "node_id=false;keyset=v1:label:desc:nulls_last,id:asc:nulls_last",
    );
    assert_ne!(
        interpretation_fingerprint(dialect, &changed, None),
        unsalted
    );
    assert_ne!(
        interpretation_fingerprint(dialect, &entities()[..2], None),
        unsalted
    );
    // The order of `entities:` does not matter.
    let mut reordered = entities();
    reordered.reverse();
    assert_eq!(
        interpretation_fingerprint(dialect, &reordered, None),
        unsalted
    );
    assert_ne!(
        interpretation_fingerprint("mssql", &entities(), None),
        unsalted
    );

    if default_case() {
        let expected = golden(&format!("interpretation_fingerprint_{dialect}.txt"));
        assert_eq!(unsalted, expected.trim_end(), "regenerate the golden");
    }
}

// Database scenarios: the started frame and the aggregate source row.

async fn fixture() -> Result<Database, Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    // The pool keeps the owned container alive; it is removed when the last clone drops.
    #[cfg(feature = "postgres")]
    let mut db = Database::new(owned_postgres::OwnedPostgres::pool("client-metadata", 4).await?);
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "client-metadata-init",
            "client-facing metadata test",
            &[
                MetaParent::metadata(),
                MetaChild::metadata(),
                MetaPlain::metadata(),
            ],
        )
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    db.set_entity_policy(AllowEntityPolicy);
    Ok(db)
}

type Events = std::pin::Pin<Box<dyn Stream<Item = Response> + Send>>;

fn open(db: &Database, query: String) -> Events {
    Box::pin(
        schema_builder(db.clone())
            .finish()
            .execute_stream(Request::new(query).data(AuthSubject::new("reader"))),
    )
}

struct AllowEntityPolicy;

impl EntityPolicy for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

async fn next_frame(stream: &mut Events, root: &str) -> serde_json::Value {
    let response = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("a frame within the timeout")
        .expect("stream open");
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().expect("json")[root].clone()
}

async fn insert_parent(db: &Database, id: &str, label: &str) -> graphql_orm::Result<()> {
    MetaParent::insert(
        db,
        CreateMetaParentInput {
            id: id.to_owned(),
            label: label.to_owned(),
            note: "note".to_owned(),
            owner_ref: "o".to_owned(),
            expires_at: Some(1_900_000_000),
        },
    )
    .await?;
    Ok(())
}

fn parent_subscription(arguments: &str) -> String {
    format!(
        "subscription {{ {root}{arguments} {{ {started} {action} {id} {entity} {{ {label} }} }} }}",
        root = resolver_name("metaParentChanged"),
        started = field_name("started"),
        action = field_name("action"),
        id = field_name("id"),
        entity = field_name("metaParent"),
        label = field_name("label"),
    )
}

async fn started_frame_precedes_events_only_when_requested(db: &Database) -> TestResult {
    let root = resolver_name("metaParentChanged");
    let started = field_name("started");
    let action = field_name("action");
    let id = field_name("id");

    let mut requested = open(
        db,
        parent_subscription(&format!("({}: true)", argument_name("started"))),
    );
    let first = next_frame(&mut requested, root).await;
    assert_eq!(first[started], true, "{first}");
    assert!(first[action].is_null() && first[id].is_null(), "{first}");
    assert!(first[field_name("metaParent")].is_null(), "{first}");

    // Committed after the started frame, so it is delivered.
    let mut unrequested = open(db, parent_subscription(""));
    let mut explicit_false = open(
        db,
        parent_subscription(&format!("({}: false)", argument_name("started"))),
    );
    // Open both streams before the write: their first poll subscribes them.
    let pending = timeout(Duration::from_millis(200), unrequested.next()).await;
    assert!(pending.is_err(), "no frame before an event: {pending:?}");
    let pending = timeout(Duration::from_millis(200), explicit_false.next()).await;
    assert!(pending.is_err(), "no frame before an event: {pending:?}");
    insert_parent(db, "p1", "first").await?;

    for stream in [&mut requested, &mut unrequested, &mut explicit_false] {
        let event = next_frame(stream, root).await;
        assert_eq!(event[started], false, "{event}");
        assert_eq!(event[id], "p1", "{event}");
        assert!(event[action].is_string(), "{event}");
    }
    Ok(())
}

async fn opted_in_aggregate_rows_carry_a_null_source_entity(db: &Database) -> TestResult {
    insert_parent(db, "a1", "aggregated").await?;
    insert_parent(db, "a2", "aggregated").await?;
    let query = format!(
        "{{ {root}({group_by}: [{label}], {metrics}: [{{ {operator}: COUNT }}], {group_limit}: 5) {{ {groups} {{ {value} }} {metrics_field} {{ {value} }} {source} {{ {id} }} }} }}",
        root = resolver_name("metaParentsAggregate"),
        group_by = argument_name("groupBy"),
        label = field_name("label"),
        groups = field_name("groups"),
        metrics = argument_name("metrics"),
        operator = field_name("operator"),
        group_limit = argument_name("groupLimit"),
        metrics_field = field_name("metrics"),
        value = field_name("value"),
        source = field_name("sourceEntity"),
        id = field_name("id"),
    );
    let response = schema_builder(db.clone())
        .finish()
        .execute(Request::new(query).data(AuthSubject::new("reader")))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let rows = response.data.into_json()?[resolver_name("metaParentsAggregate")].clone();
    let rows = rows.as_array().expect("rows");
    assert!(
        rows.iter()
            .all(|row| row[field_name("sourceEntity")].is_null()),
        "{rows:?}"
    );
    let row = rows
        .iter()
        .find(|row| row[field_name("groups")][0][field_name("value")] == "aggregated")
        .expect("the aggregated group");
    assert_eq!(row[field_name("metrics")][0][field_name("value")], "2");
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn started_frame_on_sqlite() -> TestResult {
    started_frame_precedes_events_only_when_requested(&fixture().await?).await
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn aggregate_source_row_on_sqlite() -> TestResult {
    opted_in_aggregate_rows_carry_a_null_source_entity(&fixture().await?).await
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "starts a test-owned PostgreSQL container"]
async fn database_scenarios_on_owned_postgres() -> TestResult {
    let db = fixture().await?;
    started_frame_precedes_events_only_when_requested(&db).await?;
    opted_in_aggregate_rows_carry_a_null_source_entity(&db).await
}
