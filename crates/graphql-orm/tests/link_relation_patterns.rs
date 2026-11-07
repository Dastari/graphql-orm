// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
//! Patterns over a general link table: typed optional relations selected by a
//! string kind, an existential filter over the link rows, and a nested create
//! written as one custom mutation over `Database::transaction`.
//!
//! - Link rows store `target_kind` and `target_id`. `ProbeLink` exposes
//!   `TargetPerson` and `TargetDevice` as optional relations, each selected by
//!   `source_condition(field = "target_kind", equals = "...")`. The target's
//!   own row policy applies, and the loads stay batched.
//! - `ProbeOwner` opts its `linked_to` collection into `filter`. One
//!   alternative matches one link row, so `TargetKind` and `TargetId` must hold
//!   on the same row. An empty list matches no owners; omitting the member
//!   applies no filter. Under `field-case-pascal` the member is `LinkedTo`.
//! - No generated mutation creates a parent with typed child rows. The
//!   supported way is a custom mutation that opens `Database::transaction`,
//!   shown by `NestedCreate`; a failure on any row rolls the whole create back.
//!
//! The relation `filter` limits are proven in `static_related_filter.rs`:
//! traversed entities need `ReadVisibility::Complete` or `Unrestricted`, and
//! bulk writes refuse relation filters. `conditional_relations.rs` proves the
//! integer, boolean and floating discriminators.
//!
//! The ORM query counter is process-global, so every test here holds
//! `QUERY_COUNT_LOCK` for its whole run.

#[path = "support/graphql_case.rs"]
mod graphql_case;

use graphql_case::{argument_name as a, field_name as f, resolver_name as r};
use graphql_orm::async_graphql::{Context, InputObject, Object, Request, SimpleObject};
use graphql_orm::prelude::*;
use serde_json::{Value, json};

static QUERY_COUNT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(
    backend = "sqlite",
    schema_policy = "managed",
    table = "probe_people",
    plural = "ProbePeople",
    default_sort = "id ASC"
)]
pub struct ProbePerson {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub name: String,
    #[filterable(type = "boolean")]
    pub hidden: bool,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(
    backend = "sqlite",
    schema_policy = "managed",
    table = "probe_devices",
    plural = "ProbeDevices",
    default_sort = "id ASC"
)]
pub struct ProbeDevice {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub label: String,
}

/// One link entity, typed optional relations selected by the string kind.
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
#[graphql_entity(
    backend = "sqlite",
    schema_policy = "managed",
    table = "probe_links",
    plural = "ProbeLinks",
    default_sort = "id ASC"
)]
pub struct ProbeLink {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    #[filterable(type = "string")]
    pub owner_id: String,
    #[filterable(type = "string")]
    pub target_kind: String,
    #[filterable(type = "string")]
    pub target_id: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "ProbePerson",
        from = "target_id",
        to = "id",
        source_condition(field = "target_kind", equals = "person"),
        emit_fk = false
    )]
    pub target_person: Option<ProbePerson>,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "ProbeDevice",
        from = "target_id",
        to = "id",
        source_condition(field = "target_kind", equals = "device"),
        emit_fk = false
    )]
    pub target_device: Option<ProbeDevice>,
}

/// The existential filter and the identity declarations on one entity.
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
#[graphql_entity(
    backend = "sqlite",
    schema_policy = "managed",
    node_id,
    table = "probe_owners",
    plural = "ProbeOwners",
    default_sort = "id ASC",
    keyset = "id asc"
)]
pub struct ProbeOwner {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub label: String,
    #[graphql_orm(version = "managed", default = "0")]
    pub row_version: i64,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "ProbeLink",
        from = "id",
        to = "owner_id",
        multiple,
        filter,
        emit_fk = false
    )]
    pub linked_to: Vec<ProbeLink>,
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
                use graphql_orm::sqlx::Row;
                row.try_get("id")
            }
        }
    )+};
}
batch_by_id!(ProbePerson, ProbeDevice, ProbeLink);

/// The custom mutation: the nested create the generated roots do not offer.
#[derive(InputObject)]
pub struct NestedLinkInput {
    pub id: String,
    pub target_kind: String,
    pub target_id: String,
}

#[derive(Default)]
pub struct NestedCreate;

#[Object]
impl NestedCreate {
    /// Create an owner and its link rows in one transaction. Any failure rolls
    /// every row back and releases no event. The host authorizes the caller
    /// here, as for any custom root field.
    async fn create_owner_with_links(
        &self,
        ctx: &Context<'_>,
        id: String,
        label: String,
        links: Vec<NestedLinkInput>,
    ) -> async_graphql::Result<ProbeOwner> {
        let db = ctx.data::<Database>()?;
        let owner_key = id.clone();
        db.transaction(TransactionMode::Default, move |tx| {
            Box::pin(async move {
                let owner = tx
                    .insert::<ProbeOwner>(CreateProbeOwnerInput { id, label })
                    .await?;
                for link in links {
                    tx.insert::<ProbeLink>(CreateProbeLinkInput {
                        id: link.id,
                        owner_id: owner_key.clone(),
                        target_kind: link.target_kind,
                        target_id: link.target_id,
                    })
                    .await?;
                }
                Ok(owner)
            })
        })
        .await
        .map_err(|error| async_graphql::Error::new(format!("{error}")))
    }
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    generated_mutations: "none",
    extra_mutation_types: [NestedCreate],
    entities: [ProbeOwner, ProbeLink, ProbePerson, ProbeDevice],
}

/// Hides people flagged `hidden`; every other row is readable.
struct HiddenPeople;
impl RowPolicy for HiddenPeople {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(row.downcast_ref::<ProbePerson>().is_none_or(|p| !p.hidden)) })
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

type TestResult = Result<(), Box<dyn std::error::Error>>;
type TestSchema = async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

async fn database(policy: bool) -> Result<Database, Box<dyn std::error::Error>> {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    for statement in [
        "CREATE TABLE probe_people (id TEXT PRIMARY KEY, name TEXT NOT NULL, hidden INTEGER NOT NULL)",
        "CREATE TABLE probe_devices (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
        "CREATE TABLE probe_links (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, target_kind TEXT NOT NULL, target_id TEXT NOT NULL)",
        "CREATE TABLE probe_owners (id TEXT PRIMARY KEY, label TEXT NOT NULL, row_version INTEGER NOT NULL DEFAULT 0)",
        "INSERT INTO probe_people VALUES ('a', 'visible person', 0), ('h', 'hidden person', 1)",
        "INSERT INTO probe_devices VALUES ('a', 'device sharing a key with a person'), ('d', 'device')",
        // o1 links a person and a device; o2 a hidden person; o3 a device only;
        // o4 has no links. 'a' is both a person key and a device key.
        "INSERT INTO probe_owners VALUES ('o1', 'one', 0), ('o2', 'two', 0), ('o3', 'three', 0), ('o4', 'four', 0)",
        "INSERT INTO probe_links VALUES ('l1', 'o1', 'person', 'a'), ('l2', 'o1', 'device', 'd'), ('l3', 'o2', 'person', 'h'), ('l4', 'o3', 'device', 'a'), ('l5', 'o3', 'person', 'missing')",
    ] {
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await?;
    }
    if policy {
        db.set_row_policy(HiddenPeople);
        db = db.with_authorized_scan_config(AuthorizedScanConfig::new([17; 32], 8, 64));
    }
    Ok(db)
}

async fn schema(policy: bool) -> Result<TestSchema, Box<dyn std::error::Error>> {
    Ok(schema_builder(database(policy).await?).finish())
}

/// Spell connection members the way `field-case-*` generates them.
fn request(query: impl Into<String>) -> Request {
    let query = query
        .into()
        .replace("edges {", &format!("{} {{", f("edges")))
        .replace("node {", &format!("{} {{", f("node")));
    Request::new(query).data("test-user".to_owned())
}

async fn run(schema: &TestSchema, query: String) -> Value {
    let response = schema.execute(request(query)).await;
    assert!(response.errors.is_empty(), "{response:?}");
    response.data.into_json().unwrap()
}

async fn owners_where(schema: &TestSchema, filter: Value) -> Vec<String> {
    let response = schema
        .execute(
            request(format!(
                "query($w: ProbeOwnerWhereInput!) {{ rows: {}({}: $w, {}: {{ {}: ASC }}) {{ edges {{ node {{ Id }} }} }} }}",
                r("probeOwners"),
                a("where"),
                a("orderBy"),
                "Id"
            ))
            .variables(async_graphql::Variables::from_json(json!({ "w": filter }))),
        )
        .await;
    assert!(response.errors.is_empty(), "{response:?}");
    response.data.into_json().unwrap()["rows"][f("edges")]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge[f("node")]["Id"].as_str().unwrap().to_owned())
        .collect()
}

// ---------------------------------------------------------------- question 1

#[tokio::test]
async fn string_kinds_select_typed_optional_relations_and_the_targets_row_policy_applies()
-> TestResult {
    let _guard = QUERY_COUNT_LOCK.lock().await;
    let schema = schema(true).await?;
    let sdl = schema.sdl();
    let link = sdl
        .split("type ProbeLink ")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    assert!(link.contains("TargetPerson: ProbePerson"), "{link}");
    assert!(link.contains("TargetDevice: ProbeDevice"), "{link}");

    graphql_orm::graphql::orm::reset_query_count();
    let data = run(
        &schema,
        format!(
            "{{ {}({}: {{ Id: ASC }}) {{ edges {{ node {{ Id TargetKind TargetPerson {{ Id Name }} TargetDevice {{ Id Label }} }} }} }} }}",
            r("probeLinks"),
            a("orderBy")
        ),
    )
    .await;
    let statements = graphql_orm::graphql::orm::query_count();
    let rows: Vec<&Value> = data[r("probeLinks")][f("edges")]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| &edge[f("node")])
        .collect();
    assert_eq!(rows.len(), 5);
    let person = |row: &Value| row["TargetPerson"]["Id"].as_str().map(str::to_owned);
    let device = |row: &Value| row["TargetDevice"]["Id"].as_str().map(str::to_owned);
    // l1: person 'a' resolves only as a person, even though a device 'a' exists.
    assert_eq!((person(rows[0]), device(rows[0])), (Some("a".into()), None));
    // l2: device 'd'.
    assert_eq!((person(rows[1]), device(rows[1])), (None, Some("d".into())));
    // l3: the person is hidden by the target's own row policy.
    assert_eq!((person(rows[2]), device(rows[2])), (None, None));
    // l4: device 'a' resolves only as a device.
    assert_eq!((person(rows[3]), device(rows[3])), (None, Some("a".into())));
    // l5: a person key with no row stays null instead of failing.
    assert_eq!((person(rows[4]), device(rows[4])), (None, None));
    assert!(
        statements <= 4,
        "one link read plus one batched read per relation, got {statements}"
    );
    Ok(())
}

// ---------------------------------------------------------------- question 2

#[tokio::test]
async fn existential_filter_matches_kind_and_id_on_one_link_row() -> TestResult {
    let _guard = QUERY_COUNT_LOCK.lock().await;
    let schema = schema(false).await?;
    let sdl = schema.sdl();
    let input = sdl
        .split("input ProbeOwnerWhereInput {")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    assert!(
        input.contains("LinkedTo: [ProbeLinkWhereInput!]"),
        "the member is the relation's name, so the relation must be called linked_to: {input}"
    );

    let kind_and_id = |kind: &str, ids: Value| {
        json!({ "LinkedTo": [{
            "TargetKind": { f("eq"): kind },
            "TargetId": { f("inList"): ids },
        }] })
    };
    // Kind and Id are checked on the same link row: o1 has the person link 'a',
    // and o3 only a device link 'a', so they differ by kind.
    assert_eq!(
        owners_where(&schema, kind_and_id("person", json!(["a"]))).await,
        ["o1"]
    );
    assert_eq!(
        owners_where(&schema, kind_and_id("device", json!(["a"]))).await,
        ["o3"]
    );
    // o1 has a person link and a device 'd' link, but no person 'd' link.
    assert!(
        owners_where(&schema, kind_and_id("person", json!(["d"])))
            .await
            .is_empty()
    );
    assert_eq!(
        owners_where(&schema, kind_and_id("person", json!(["a", "h"]))).await,
        ["o1", "o2"]
    );
    // An alternative list is a disjunction; the empty list matches nothing.
    let either = json!({ "LinkedTo": [
        { "TargetKind": { f("eq"): "device" }, "TargetId": { f("eq"): "d" } },
        { "TargetKind": { f("eq"): "person" }, "TargetId": { f("eq"): "h" } },
    ] });
    assert_eq!(owners_where(&schema, either).await, ["o1", "o2"]);
    assert!(
        owners_where(&schema, json!({ "LinkedTo": [] }))
            .await
            .is_empty()
    );
    // Omission is no filter, so owners without any link are listed as well.
    assert_eq!(
        owners_where(&schema, json!({})).await,
        ["o1", "o2", "o3", "o4"]
    );
    // An explicitly empty id set inside an alternative matches no link row.
    assert!(
        owners_where(&schema, kind_and_id("person", json!([])))
            .await
            .is_empty()
    );
    Ok(())
}

// ---------------------------------------------------------------- question 3

#[tokio::test]
async fn a_nested_create_is_one_transaction() -> TestResult {
    let _guard = QUERY_COUNT_LOCK.lock().await;
    let db = database(false).await?;
    let schema = schema_builder(db.clone()).finish();
    assert!(
        !schema.sdl().contains("createProbeOwner("),
        "no generated nested create exists"
    );
    let mutation = |owner: &str, links: &str| {
        format!(
            r#"mutation {{ createOwnerWithLinks(id: "{owner}", label: "new", links: [{links}]) {{ Id NodeId RecordVersion }} }}"#
        )
    };
    let link = |id: &str, kind: &str, target: &str| {
        format!(r#"{{ id: "{id}", targetKind: "{kind}", targetId: "{target}" }}"#)
    };

    let created = schema
        .execute(request(mutation(
            "o9",
            &[link("n1", "person", "a"), link("n2", "device", "d")].join(","),
        )))
        .await;
    assert!(created.errors.is_empty(), "{created:?}");
    let created = created.data.into_json()?;
    assert_eq!(created["createOwnerWithLinks"]["Id"], "o9");
    assert_eq!(
        created["createOwnerWithLinks"]["NodeId"],
        graphql_orm::graphql::federation::encode_node_id("ProbeOwner", vec![json!("o9")]).as_str()
    );
    assert_eq!(count(&db, "probe_links WHERE owner_id = 'o9'").await?, 2);

    // The second link repeats an existing id: nothing from this call stays.
    let refused = schema
        .execute(request(mutation(
            "o8",
            &[link("n3", "person", "a"), link("l1", "device", "d")].join(","),
        )))
        .await;
    assert!(!refused.errors.is_empty(), "{refused:?}");
    assert_eq!(count(&db, "probe_owners WHERE id = 'o8'").await?, 0);
    assert_eq!(count(&db, "probe_links WHERE id = 'n3'").await?, 0);
    // The created owner is found through the existential filter.
    assert_eq!(
        owners_where(
            &schema_builder(db).finish(),
            json!({ "LinkedTo": [{ "TargetId": { f("eq"): "d" } }] })
        )
        .await,
        ["o1", "o9"]
    );
    Ok(())
}

async fn count(db: &Database, from: &str) -> Result<i64, Box<dyn std::error::Error>> {
    use graphql_orm::sqlx::Row;
    Ok(
        graphql_orm::sqlx::query(&format!("SELECT COUNT(*) AS n FROM {from}"))
            .fetch_one(db.pool())
            .await?
            .try_get("n")?,
    )
}
