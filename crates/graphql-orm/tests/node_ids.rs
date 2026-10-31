#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! `#[graphql_entity(node_id)]`: the generated global `NodeId` field and its
//! resolvable `@key(fields: "NodeId")`.
//!
//! The entity and key names reproduce the encoding vectors shared with
//! graphql-orm-client, so a stored row's `NodeId` must equal the client's
//! encoding byte for byte.
//!
//! SQLite runs every scenario as its own test. PostgreSQL and SQL Server run
//! all scenarios in one ignored test against a single test-owned container:
//!
//! ```sh
//! cargo test -p graphql-orm --no-default-features --features postgres --test node_ids -- --ignored
//! cargo test -p graphql-orm --no-default-features --features mssql --test node_ids -- --ignored
//! ```

use graphql_orm::prelude::*;

#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{argument_name, field_name, resolver_name};

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

type TestResult = Result<(), Box<dyn std::error::Error>>;

macro_rules! node_entity {
    ($name:ident, $table:literal, $plural:literal, [$($extra:tt)*], { $($body:tt)* }) => {
        #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(
            feature = "sqlite",
            graphql_entity(backend = "sqlite", table = $table, plural = $plural, node_id $($extra)*)
        )]
        #[cfg_attr(
            feature = "postgres",
            graphql_entity(backend = "postgres", table = $table, plural = $plural, node_id $($extra)*)
        )]
        #[cfg_attr(
            feature = "mssql",
            graphql_entity(
                backend = "mssql",
                schema_policy = "external_writable",
                table = $table,
                plural = $plural,
                node_id
                $($extra)*
            )
        )]
        struct $name { $($body)* }
    };
}

node_entity!(Ticket, "node_tickets", "Tickets", [], {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "number")]
    #[sortable]
    id: i64,
    owner: String,
    title: String,
});

node_entity!(Note, "node_notes", "Notes", [], {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    owner: String,
    title: String,
});

node_entity!(Quote, "node_quotes", "Quotes", [], {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    owner: String,
    title: String,
});

node_entity!(FameDevice, "node_devices", "FameDevices", [], {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "uuid")]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    owner: String,
    title: String,
});

node_entity!(ManagedSlot, "node_slots", "ManagedSlots", [, repository_mutations = true], {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    site: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    name: String,
    owner: String,
    title: String,
});

/// An entity without `node_id` keeps its schema unchanged.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "node_plain_rows", plural = "PlainRows")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "node_plain_rows", plural = "PlainRows")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "node_plain_rows",
        plural = "PlainRows"
    )
)]
struct PlainRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    title: String,
}

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Ticket, Note, Quote, FameDevice, ManagedSlot, PlainRow],
}
#[cfg(feature = "postgres")]
schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [Ticket, Note, Quote, FameDevice, ManagedSlot, PlainRow],
}
// SQL Server generates no GraphQL mutations for composite keys.
#[cfg(feature = "mssql")]
schema_roots! {
    backend: "mssql",
    schema_policy: "external_writable",
    query_custom_ops: [],
    entities: [Ticket, Note, Quote, FameDevice, ManagedSlot, PlainRow],
    generated_mutations: "denylist",
    generated_mutation_denylist: [ManagedSlot],
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

/// Rows owned by `hidden` are unreadable.
struct HiddenRowPolicy;

fn owner_of(row: &(dyn std::any::Any + Send + Sync)) -> Option<&str> {
    row.downcast_ref::<Ticket>()
        .map(|row| row.owner.as_str())
        .or_else(|| row.downcast_ref::<Note>().map(|row| row.owner.as_str()))
        .or_else(|| row.downcast_ref::<Quote>().map(|row| row.owner.as_str()))
        .or_else(|| {
            row.downcast_ref::<FameDevice>()
                .map(|row| row.owner.as_str())
        })
        .or_else(|| {
            row.downcast_ref::<ManagedSlot>()
                .map(|row| row.owner.as_str())
        })
}

impl RowPolicy for HiddenRowPolicy {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        let readable = owner_of(row) != Some("hidden");
        Box::pin(async move { Ok(readable) })
    }

    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

const DEVICE: &str = "9b2c0f2e-1d1a-4f6e-9c1e-0a1b2c3d4e5f";
const QUOTE_ID: &str = "say \"hi\"\\n";

/// The vectors shared with graphql-orm-client, as stored rows.
const TICKET_42: &str = "WyJUaWNrZXQiLDQyXQ";
const NOTE_UNICODE: &str = "WyJOb3RlIiwiw7xuw69jw7hkw6kg4pyTIl0";
const QUOTE_ESCAPES: &str = "WyJRdW90ZSIsInNheSBcImhpXCJcXG4iXQ";
const DEVICE_UUID: &str = "WyJGYW1lRGV2aWNlIiwiOWIyYzBmMmUtMWQxYS00ZjZlLTljMWUtMGExYjJjM2Q0ZTVmIl0";
const SLOT_NORTH_A: &str = "WyJNYW5hZ2VkU2xvdCIsIm5vcnRoIiwiYSJd";

#[cfg(not(feature = "sqlite"))]
const TABLES: [&str; 6] = [
    "node_tickets",
    "node_notes",
    "node_quotes",
    "node_devices",
    "node_slots",
    "node_plain_rows",
];

struct Fixture {
    db: Database,
    #[cfg(feature = "mssql")]
    owner: owned_mssql::OwnedSqlServer,
}

impl Fixture {
    fn finish(self) -> TestResult {
        #[cfg(feature = "mssql")]
        {
            let Self { db, mut owner } = self;
            drop(db);
            owner.cleanup()?;
        }
        Ok(())
    }
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    // The pool keeps the owned container alive; it is removed when the last clone drops.
    #[cfg(feature = "postgres")]
    let mut db = Database::new(owned_postgres::OwnedPostgres::pool("node-ids", 4).await?);
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "node-ids-init",
                "node id test",
                &[
                    Ticket::metadata(),
                    Note::metadata(),
                    Quote::metadata(),
                    FameDevice::metadata(),
                    ManagedSlot::metadata(),
                    PlainRow::metadata(),
                ],
            )
            .await?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    #[cfg(feature = "mssql")]
    let (mut db, owner) = {
        let owner = owned_mssql::OwnedSqlServer::start().await?;
        owned_mssql::execute_batch(
            &owner.connection_string,
            "CREATE TABLE node_tickets (id BIGINT NOT NULL PRIMARY KEY, \
             owner NVARCHAR(64) NOT NULL, title NVARCHAR(64) NOT NULL); \
             CREATE TABLE node_notes (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             owner NVARCHAR(64) NOT NULL, title NVARCHAR(64) NOT NULL); \
             CREATE TABLE node_quotes (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             owner NVARCHAR(64) NOT NULL, title NVARCHAR(64) NOT NULL); \
             CREATE TABLE node_devices (id UNIQUEIDENTIFIER NOT NULL PRIMARY KEY, \
             owner NVARCHAR(64) NOT NULL, title NVARCHAR(64) NOT NULL); \
             CREATE TABLE node_slots (site NVARCHAR(64) NOT NULL, name NVARCHAR(64) NOT NULL, \
             owner NVARCHAR(64) NOT NULL, title NVARCHAR(64) NOT NULL, PRIMARY KEY (site, name)); \
             CREATE TABLE node_plain_rows (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             title NVARCHAR(64) NOT NULL);",
        )
        .await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        let db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        (db, owner)
    };
    db.set_entity_policy(AllowEntityPolicy);
    db.set_row_policy(HiddenRowPolicy);
    Ok(Fixture {
        db,
        #[cfg(feature = "mssql")]
        owner,
    })
}

#[cfg(not(feature = "sqlite"))]
async fn reset(db: &Database) -> graphql_orm::Result<()> {
    for table in TABLES {
        <DefaultBackend as WriteBackend>::execute_with_binds(
            db.pool(),
            &format!("DELETE FROM {table}"),
            &[],
        )
        .await?;
    }
    Ok(())
}

async fn seed(db: &Database) -> graphql_orm::Result<()> {
    for (id, owner, title) in [
        (42, "alice", "answer"),
        (7, "alice", "seven"),
        (9, "hidden", "secret"),
    ] {
        Ticket::insert(
            db,
            CreateTicketInput {
                id,
                owner: owner.into(),
                title: title.into(),
            },
        )
        .await?;
    }
    // A Note shares the Ticket's key value: the type keeps the identities apart.
    for (id, title) in [("ünïcødé ✓", "unicode"), ("42", "same key as a ticket")] {
        Note::insert(
            db,
            CreateNoteInput {
                id: id.into(),
                owner: "alice".into(),
                title: title.into(),
            },
        )
        .await?;
    }
    Quote::insert(
        db,
        CreateQuoteInput {
            id: QUOTE_ID.into(),
            owner: "alice".into(),
            title: "escapes".into(),
        },
    )
    .await?;
    FameDevice::insert(
        db,
        CreateFameDeviceInput {
            id: DEVICE.parse().expect("uuid"),
            owner: "alice".into(),
            title: "device".into(),
        },
    )
    .await?;
    for (site, name) in [("north", "a"), ("north", "b"), ("south", "a")] {
        ManagedSlot::insert(
            db,
            CreateManagedSlotInput {
                site: site.into(),
                name: name.into(),
                owner: "alice".into(),
                title: format!("{site}-{name}"),
            },
        )
        .await?;
    }
    PlainRow::insert(
        db,
        CreatePlainRowInput {
            id: "plain".into(),
            title: "plain".into(),
        },
    )
    .await?;
    Ok(())
}

fn schema(db: &Database) -> AppSchema {
    schema_builder(db.clone())
        .data(AuthSubject::new("alice"))
        .finish()
}

async fn data(db: &Database, query: String) -> serde_json::Value {
    let response = schema(db).execute(query).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().expect("json")
}

fn string_literal(value: &str) -> String {
    serde_json::to_string(value).expect("string literal")
}

/// `_entities` over `NodeId` representations, selecting `selection` on `typename`.
fn entities_query(representations: &[(&str, &str)], fragments: &str) -> String {
    let representations = representations
        .iter()
        .map(|(typename, node_id)| {
            format!(
                "{{ __typename: {}, NodeId: {} }}",
                string_literal(typename),
                string_literal(node_id)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("query {{ _entities(representations: [{representations}]) {{ {fragments} }} }}")
}

async fn stored_rows_carry_the_shared_encoding(db: &Database) -> TestResult {
    seed(db).await?;
    let id = argument_name("id");
    let site = argument_name("site");
    let name = argument_name("name");
    let reads = [
        (resolver_name("ticket"), format!("{id}: 42"), TICKET_42),
        (
            resolver_name("note"),
            format!("{id}: {}", string_literal("ünïcødé ✓")),
            NOTE_UNICODE,
        ),
        (
            resolver_name("quote"),
            format!("{id}: {}", string_literal(QUOTE_ID)),
            QUOTE_ESCAPES,
        ),
        (
            resolver_name("fameDevice"),
            format!("{id}: {}", string_literal(DEVICE)),
            DEVICE_UUID,
        ),
        (
            resolver_name("managedSlot"),
            format!(r#"{site}: "north", {name}: "a""#),
            SLOT_NORTH_A,
        ),
    ];
    for (field, arguments, expected) in reads {
        let json = data(db, format!("{{ {field}({arguments}) {{ NodeId }} }}")).await;
        assert_eq!(json[field]["NodeId"], expected, "{field}");
    }
    Ok(())
}

async fn node_ids_resolve_through_entities(db: &Database) -> TestResult {
    seed(db).await?;
    let title = field_name("title");
    let site = field_name("site");
    let fragments = format!(
        "__typename ... on Ticket {{ NodeId {title} }} ... on Note {{ NodeId {title} }} \
         ... on Quote {{ {title} }} ... on FameDevice {{ {title} }} ... on ManagedSlot {{ {site} {title} }}"
    );
    let json = data(
        db,
        entities_query(
            &[
                ("Ticket", TICKET_42),
                ("Note", NOTE_UNICODE),
                ("Quote", QUOTE_ESCAPES),
                ("FameDevice", DEVICE_UUID),
                ("ManagedSlot", SLOT_NORTH_A),
            ],
            &fragments,
        ),
    )
    .await;
    let entities = &json["_entities"];
    assert_eq!(entities[0]["__typename"], "Ticket");
    assert_eq!(entities[0][title], "answer");
    assert_eq!(entities[0]["NodeId"], TICKET_42);
    assert_eq!(entities[1][title], "unicode");
    assert_eq!(entities[2][title], "escapes");
    assert_eq!(entities[3][title], "device");
    assert_eq!(entities[4][site], "north");
    assert_eq!(entities[4][title], "north-a");

    // A Ticket and a Note with the same key value are different records.
    let note_42 =
        graphql_orm::graphql::federation::encode_node_id("Note", vec![serde_json::json!("42")]);
    let json = data(
        db,
        entities_query(&[("Ticket", TICKET_42), ("Note", &note_42)], &fragments),
    )
    .await;
    assert_eq!(json["_entities"][0][title], "answer");
    assert_eq!(json["_entities"][1][title], "same key as a ticket");
    Ok(())
}

async fn malformed_mismatched_missing_and_denied_ids_are_all_null(db: &Database) -> TestResult {
    seed(db).await?;
    let title = field_name("title");
    let encode = |typename: &str, part: serde_json::Value| {
        graphql_orm::graphql::federation::encode_node_id(typename, vec![part]).0
    };
    let missing = encode("Ticket", serde_json::json!(1000));
    let denied = encode("Ticket", serde_json::json!(9));
    let wrong_part_type = encode("Ticket", serde_json::json!("42"));
    let slot_missing_part = encode("ManagedSlot", serde_json::json!("north"));
    let device_uppercase = encode("FameDevice", serde_json::json!(DEVICE.to_uppercase()));
    let cases = [
        ("Ticket", "not base64!"),
        ("Ticket", "WyJUaWNrZXQiLDQyXQ=="),
        // A Note's identity presented as a Ticket representation.
        ("Ticket", NOTE_UNICODE),
        ("Ticket", wrong_part_type.as_str()),
        ("ManagedSlot", slot_missing_part.as_str()),
        // Not the canonical spelling of the stored UUID.
        ("FameDevice", device_uppercase.as_str()),
        ("Ticket", missing.as_str()),
        ("Ticket", denied.as_str()),
    ];
    let json = data(
        db,
        entities_query(
            &cases,
            &format!(
                "... on Ticket {{ {title} }} ... on ManagedSlot {{ {title} }} ... on FameDevice {{ {title} }}"
            ),
        ),
    )
    .await;
    let entities = json["_entities"].as_array().expect("entities");
    assert_eq!(entities.len(), cases.len());
    for (case, entity) in cases.iter().zip(entities) {
        assert!(entity.is_null(), "{case:?} resolved to {entity}");
    }
    Ok(())
}

async fn one_type_resolves_in_one_statement(db: &Database) -> TestResult {
    seed(db).await?;
    let title = field_name("title");
    let seven =
        graphql_orm::graphql::federation::encode_node_id("Ticket", vec![serde_json::json!(7)]);
    let query = entities_query(
        &[
            ("Ticket", TICKET_42),
            ("Ticket", &seven),
            ("Ticket", TICKET_42),
        ],
        &format!("... on Ticket {{ {title} }}"),
    );
    let schema = schema(db);
    graphql_orm::graphql::orm::reset_query_count();
    let response = schema.execute(query).await;
    let statements = graphql_orm::graphql::orm::query_count();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json()?;
    assert_eq!(json["_entities"][1][title], "seven");
    assert_eq!(statements, 1, "three NodeIds of one type are one statement");
    Ok(())
}

async fn the_sdl_declares_node_id_keys_without_a_node_interface(db: &Database) -> TestResult {
    let sdl = schema(db).sdl_with_options(async_graphql::SDLExportOptions::new().federation());
    for typename in ["Ticket", "Note", "Quote", "FameDevice", "ManagedSlot"] {
        let definition = sdl
            .split("\ntype ")
            .find(|definition| definition.starts_with(&format!("{typename} ")))
            .unwrap_or_else(|| panic!("{typename} in SDL"));
        assert!(
            definition.contains("@key(fields: \"NodeId\""),
            "{typename}: {definition}"
        );
        assert!(
            definition.contains("NodeId: ID!"),
            "{typename}: {definition}"
        );
        assert!(
            !definition.contains("implements"),
            "{typename}: {definition}"
        );
    }
    let plain = sdl
        .split("\ntype ")
        .find(|definition| definition.starts_with("PlainRow "))
        .expect("PlainRow in SDL");
    assert!(!plain.contains("NodeId"), "{plain}");
    assert!(!sdl.contains("interface Node"));
    Ok(())
}

/// Serializes SQLite scenarios, which share the process-global statement counter.
#[cfg(feature = "sqlite")]
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

macro_rules! scenarios {
    ($($scenario:ident),* $(,)?) => {
        #[cfg(feature = "sqlite")]
        mod sqlite_scenarios {
            $(
                #[tokio::test]
                async fn $scenario() -> super::TestResult {
                    let _serial = super::SERIAL.lock().await;
                    let fixture = super::fixture().await?;
                    super::$scenario(&fixture.db).await?;
                    fixture.finish()
                }
            )*
        }

        #[cfg(not(feature = "sqlite"))]
        #[tokio::test]
        #[ignore = "starts a test-owned database container"]
        async fn every_scenario_on_an_owned_database() -> TestResult {
            let fixture = fixture().await?;
            $(
                reset(&fixture.db).await?;
                $scenario(&fixture.db)
                    .await
                    .map_err(|error| format!("{}: {error}", stringify!($scenario)))?;
                println!("node id scenario passed: {}", stringify!($scenario));
            )*
            fixture.finish()
        }
    };
}

scenarios![
    stored_rows_carry_the_shared_encoding,
    node_ids_resolve_through_entities,
    malformed_mismatched_missing_and_denied_ids_are_all_null,
    one_type_resolves_in_one_statement,
    the_sdl_declares_node_id_keys_without_a_node_interface,
];
