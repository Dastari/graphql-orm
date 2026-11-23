//! Model-context benchmark for the fixed capability broker.
//!
//! Stamps out copies of one realistic generated entity family (a wide record
//! with forty public scalar fields and five to-many relationships, plus its
//! five related entities, each with its generated query roots) across `T`
//! independent logical targets, compiles every target's
//! query catalogue and capability index, registers the catalogues in an
//! [`AiToolCatalog`], and drives the frozen version 2 broker with the
//! coordinator's own describe steps: the exact model definition and its
//! fingerprint check, the signature, and the describe byte bounds, which
//! also refuse an oversized `describe_type` page. For each `T` it reports what the
//! model receives: the first-turn definition bytes, discover, describe and
//! `describe_type` result bytes, discover latency, a restricted caller's
//! candidates, and whether an unrelated target's schema change rotates a
//! retained session binding. The eager reference is what one exact
//! definition per root would cost. A separate single-target point compares
//! a host that supplies one index with a host that caches its index set,
//! after a first pass that meets cold per-capability caches.
//!
//! ```text
//! cargo run --release -p graphql-orm-ai \
//!     --example capability_context_benchmark -- 1 10 100
//! ```
//!
//! `--single-target-families <n>` sizes the single-target point (default
//! 10 families, 60 entities). Each point runs in its own child process so
//! its peak memory is its own; at 100 targets that is about 7.3 GiB, since
//! compiled wide-entity roots take about 1.2 MiB each.
//! Byte sizes are deterministic; times compare only between runs of the
//! same build profile on the same machine. Approximate tokens are bytes / 4.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
};

use agql_auth::{
    AccessTokenMetadata, AuthPrincipal, AuthUser, CurrentPrincipalResolver, PrincipalReference,
    ResolvedPrincipal, SessionContext, SystemClock,
};
use async_trait::async_trait;
use graphql_orm_ai::*;
use graphql_orm_operation_catalog::{
    GraphqlEntitySemanticMetadata, GraphqlSemanticCatalog, GraphqlSemanticOperationDescriptor,
};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// One family of generated entities: a wide record with forty public scalar
/// fields and five to-many relationships, plus the five related entities.
/// Every generated name contains the token `Zqxwide`, which the harness
/// replaces to stamp out independent copies.
mod template {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity,
        GraphQLRelations,
        GraphQLOperations,
        async_graphql::SimpleObject,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
    )]
    #[graphql_entity(
        table = "zqxwides",
        plural = "Zqxwides",
        default_sort = "id ASC",
        keyset = "id asc",
        aggregate = true,
        description = "A reviewed Zqxwide record with forty public fields"
    )]
    #[graphql(complex)]
    pub struct Zqxwide {
        #[primary_key]
        #[graphql_orm(auto_generated = false, description = "Stable record identity")]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql_orm(description = "Reviewed label 01 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label01: String,
        #[graphql_orm(description = "Reviewed label 02 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label02: String,
        #[graphql_orm(description = "Reviewed label 03 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label03: String,
        #[graphql_orm(description = "Reviewed label 04 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label04: String,
        #[graphql_orm(description = "Reviewed label 05 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label05: String,
        #[graphql_orm(description = "Reviewed label 06 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label06: String,
        #[graphql_orm(description = "Reviewed label 07 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label07: String,
        #[graphql_orm(description = "Reviewed label 08 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label08: String,
        #[graphql_orm(description = "Reviewed label 09 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label09: String,
        #[graphql_orm(description = "Reviewed label 10 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label10: String,
        #[graphql_orm(description = "Reviewed label 11 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label11: String,
        #[graphql_orm(description = "Reviewed label 12 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label12: String,
        #[graphql_orm(description = "Reviewed label 13 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label13: String,
        #[graphql_orm(description = "Reviewed label 14 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label14: String,
        #[graphql_orm(description = "Reviewed label 15 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label15: String,
        #[graphql_orm(description = "Reviewed label 16 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label16: String,
        #[graphql_orm(description = "Reviewed label 17 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label17: String,
        #[graphql_orm(description = "Reviewed label 18 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label18: String,
        #[graphql_orm(description = "Reviewed label 19 of the record")]
        #[filterable(type = "string")]
        #[sortable]
        pub label19: String,
        #[graphql_orm(description = "Reviewed amount 01 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount01: i64,
        #[graphql_orm(description = "Reviewed amount 02 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount02: i64,
        #[graphql_orm(description = "Reviewed amount 03 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount03: i64,
        #[graphql_orm(description = "Reviewed amount 04 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount04: i64,
        #[graphql_orm(description = "Reviewed amount 05 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount05: i64,
        #[graphql_orm(description = "Reviewed amount 06 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount06: i64,
        #[graphql_orm(description = "Reviewed amount 07 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount07: i64,
        #[graphql_orm(description = "Reviewed amount 08 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount08: i64,
        #[graphql_orm(description = "Reviewed amount 09 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount09: i64,
        #[graphql_orm(description = "Reviewed amount 10 of the record")]
        #[filterable(type = "number")]
        #[sortable]
        pub amount10: i64,
        #[graphql_orm(description = "Reviewed flag 01 of the record")]
        #[filterable(type = "boolean")]
        pub flag01: bool,
        #[graphql_orm(description = "Reviewed flag 02 of the record")]
        #[filterable(type = "boolean")]
        pub flag02: bool,
        #[graphql_orm(description = "Reviewed flag 03 of the record")]
        #[filterable(type = "boolean")]
        pub flag03: bool,
        #[graphql_orm(description = "Reviewed flag 04 of the record")]
        #[filterable(type = "boolean")]
        pub flag04: bool,
        #[graphql_orm(description = "Reviewed flag 05 of the record")]
        #[filterable(type = "boolean")]
        pub flag05: bool,
        #[graphql_orm(description = "Optional reviewed note 01 of the record")]
        #[filterable(type = "string")]
        pub note01: Option<String>,
        #[graphql_orm(description = "Optional reviewed note 02 of the record")]
        #[filterable(type = "string")]
        pub note02: Option<String>,
        #[graphql_orm(description = "Optional reviewed note 03 of the record")]
        #[filterable(type = "string")]
        pub note03: Option<String>,
        #[graphql_orm(description = "Optional reviewed note 04 of the record")]
        #[filterable(type = "string")]
        pub note04: Option<String>,
        #[graphql_orm(description = "Optional reviewed note 05 of the record")]
        #[filterable(type = "string")]
        pub note05: Option<String>,
        #[graphql(skip)]
        #[relation(target = "ZqxwidePartA", from = "id", to = "zqxwide_id", multiple)]
        pub parts_a: Vec<ZqxwidePartA>,
        #[graphql(skip)]
        #[relation(target = "ZqxwidePartB", from = "id", to = "zqxwide_id", multiple)]
        pub parts_b: Vec<ZqxwidePartB>,
        #[graphql(skip)]
        #[relation(target = "ZqxwidePartC", from = "id", to = "zqxwide_id", multiple)]
        pub parts_c: Vec<ZqxwidePartC>,
        #[graphql(skip)]
        #[relation(target = "ZqxwidePartD", from = "id", to = "zqxwide_id", multiple)]
        pub parts_d: Vec<ZqxwidePartD>,
        #[graphql(skip)]
        #[relation(target = "ZqxwidePartE", from = "id", to = "zqxwide_id", multiple)]
        pub parts_e: Vec<ZqxwidePartE>,
    }

    #[derive(
        GraphQLEntity,
        GraphQLRelations,
        GraphQLOperations,
        async_graphql::SimpleObject,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
    )]
    #[graphql_entity(
        table = "zqxwide_parts_a",
        plural = "ZqxwidePartAs",
        default_sort = "id ASC",
        keyset = "id asc",
        aggregate = true,
        description = "A reviewed part A line of one Zqxwide record"
    )]
    #[graphql(complex)]
    pub struct ZqxwidePartA {
        #[primary_key]
        #[graphql_orm(auto_generated = false, description = "Stable part identity")]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql_orm(description = "Owning Zqxwide record")]
        #[filterable(type = "string")]
        #[sortable]
        pub zqxwide_id: String,
        #[graphql_orm(description = "Reviewed part title")]
        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
        #[graphql_orm(description = "Reviewed part quantity")]
        #[filterable(type = "number")]
        #[sortable]
        pub quantity: i64,
        #[graphql(skip)]
        #[relation(target = "Zqxwide", from = "zqxwide_id", to = "id")]
        pub zqxwide: Option<Zqxwide>,
    }

    #[derive(
        GraphQLEntity,
        GraphQLRelations,
        GraphQLOperations,
        async_graphql::SimpleObject,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
    )]
    #[graphql_entity(
        table = "zqxwide_parts_b",
        plural = "ZqxwidePartBs",
        default_sort = "id ASC",
        keyset = "id asc",
        aggregate = true,
        description = "A reviewed part B line of one Zqxwide record"
    )]
    #[graphql(complex)]
    pub struct ZqxwidePartB {
        #[primary_key]
        #[graphql_orm(auto_generated = false, description = "Stable part identity")]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql_orm(description = "Owning Zqxwide record")]
        #[filterable(type = "string")]
        #[sortable]
        pub zqxwide_id: String,
        #[graphql_orm(description = "Reviewed part title")]
        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
        #[graphql_orm(description = "Reviewed part quantity")]
        #[filterable(type = "number")]
        #[sortable]
        pub quantity: i64,
        #[graphql(skip)]
        #[relation(target = "Zqxwide", from = "zqxwide_id", to = "id")]
        pub zqxwide: Option<Zqxwide>,
    }

    #[derive(
        GraphQLEntity,
        GraphQLRelations,
        GraphQLOperations,
        async_graphql::SimpleObject,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
    )]
    #[graphql_entity(
        table = "zqxwide_parts_c",
        plural = "ZqxwidePartCs",
        default_sort = "id ASC",
        keyset = "id asc",
        aggregate = true,
        description = "A reviewed part C line of one Zqxwide record"
    )]
    #[graphql(complex)]
    pub struct ZqxwidePartC {
        #[primary_key]
        #[graphql_orm(auto_generated = false, description = "Stable part identity")]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql_orm(description = "Owning Zqxwide record")]
        #[filterable(type = "string")]
        #[sortable]
        pub zqxwide_id: String,
        #[graphql_orm(description = "Reviewed part title")]
        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
        #[graphql_orm(description = "Reviewed part quantity")]
        #[filterable(type = "number")]
        #[sortable]
        pub quantity: i64,
        #[graphql(skip)]
        #[relation(target = "Zqxwide", from = "zqxwide_id", to = "id")]
        pub zqxwide: Option<Zqxwide>,
    }

    #[derive(
        GraphQLEntity,
        GraphQLRelations,
        GraphQLOperations,
        async_graphql::SimpleObject,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
    )]
    #[graphql_entity(
        table = "zqxwide_parts_d",
        plural = "ZqxwidePartDs",
        default_sort = "id ASC",
        keyset = "id asc",
        aggregate = true,
        description = "A reviewed part D line of one Zqxwide record"
    )]
    #[graphql(complex)]
    pub struct ZqxwidePartD {
        #[primary_key]
        #[graphql_orm(auto_generated = false, description = "Stable part identity")]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql_orm(description = "Owning Zqxwide record")]
        #[filterable(type = "string")]
        #[sortable]
        pub zqxwide_id: String,
        #[graphql_orm(description = "Reviewed part title")]
        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
        #[graphql_orm(description = "Reviewed part quantity")]
        #[filterable(type = "number")]
        #[sortable]
        pub quantity: i64,
        #[graphql(skip)]
        #[relation(target = "Zqxwide", from = "zqxwide_id", to = "id")]
        pub zqxwide: Option<Zqxwide>,
    }

    #[derive(
        GraphQLEntity,
        GraphQLRelations,
        GraphQLOperations,
        async_graphql::SimpleObject,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
    )]
    #[graphql_entity(
        table = "zqxwide_parts_e",
        plural = "ZqxwidePartEs",
        default_sort = "id ASC",
        keyset = "id asc",
        aggregate = true,
        description = "A reviewed part E line of one Zqxwide record"
    )]
    #[graphql(complex)]
    pub struct ZqxwidePartE {
        #[primary_key]
        #[graphql_orm(auto_generated = false, description = "Stable part identity")]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql_orm(description = "Owning Zqxwide record")]
        #[filterable(type = "string")]
        #[sortable]
        pub zqxwide_id: String,
        #[graphql_orm(description = "Reviewed part title")]
        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
        #[graphql_orm(description = "Reviewed part quantity")]
        #[filterable(type = "number")]
        #[sortable]
        pub quantity: i64,
        #[graphql(skip)]
        #[relation(target = "Zqxwide", from = "zqxwide_id", to = "id")]
        pub zqxwide: Option<Zqxwide>,
    }

    schema_roots! {
        entities: [Zqxwide, ZqxwidePartA, ZqxwidePartB, ZqxwidePartC, ZqxwidePartD, ZqxwidePartE],
    }
}

const TEMPLATE_TOKEN: &str = "Zqxwide";
const TEMPLATE_ROOT_TOKEN: &str = "zqxwide";
const TEMPLATE_DESCRIPTION: &str = "A reviewed Zqxwide record with forty public fields";
const FAMILIES_PER_TARGET: usize = 2;
const DEFAULT_TARGET_COUNTS: [usize; 3] = [1, 10, 100];
const DEFAULT_SINGLE_TARGET_FAMILIES: usize = 10;
const DISCOVER_SAMPLES: usize = 200;
const COMPILE_THREADS: usize = 6;
const DESCRIBE_SAMPLES: usize = 20;
const WORDS: [&str; 24] = [
    "Account", "Asset", "Batch", "Booking", "Contract", "Device", "Document", "Event", "Order",
    "Parcel", "Plan", "Project", "Report", "Route", "Schedule", "Shipment", "Station", "Task",
    "Ticket", "Vendor", "Ledger", "Meter", "Region", "Sample",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    match arguments.as_slice() {
        [flag, targets] if flag == "--point" => {
            let point = runtime.block_on(measure_federated(targets.parse()?))?;
            println!("{point}");
            return Ok(());
        }
        [flag, families] if flag == "--single-target" => {
            let point = runtime.block_on(measure_single_target(families.parse()?))?;
            println!("{point}");
            return Ok(());
        }
        _ => {}
    }
    let mut target_counts = Vec::new();
    let mut single_target_families = DEFAULT_SINGLE_TARGET_FAMILIES;
    let mut values = arguments.iter();
    while let Some(value) = values.next() {
        if value == "--single-target-families" {
            single_target_families = values.next().ok_or("missing family count")?.parse()?;
        } else {
            target_counts.push(value.parse::<usize>()?);
        }
    }
    if target_counts.is_empty() {
        target_counts = DEFAULT_TARGET_COUNTS.to_vec();
    }
    let executable = std::env::current_exe()?;
    let child = |arguments: [String; 2]| -> Result<Value, Box<dyn std::error::Error>> {
        let output = std::process::Command::new(&executable)
            .args(&arguments)
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "benchmark point {arguments:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let point = serde_json::from_slice::<Value>(&output.stdout)?;
        // Progress for long runs: each point as soon as it is measured.
        eprintln!("{point}");
        Ok(point)
    };
    let mut points = Vec::new();
    for targets in target_counts {
        points.push(child(["--point".to_owned(), targets.to_string()])?);
    }
    let single_target = child([
        "--single-target".to_owned(),
        single_target_families.to_string(),
    ])?;
    let report = json!({
        "benchmark": "capability_context",
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "brokerVersion": AI_CAPABILITY_BROKER_VERSION,
        "familiesPerTarget": FAMILIES_PER_TARGET,
        "entitiesPerFamily": 6,
        "tokenEstimate": "bytes / 4",
        "points": points,
        "growth": growth(&points),
        "singleTarget": single_target,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Ratio of the largest point to the smallest for each model-facing size.
fn growth(points: &[Value]) -> Value {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Value::Null;
    };
    let ratio = |path: &[&str]| {
        let read = |point: &Value| {
            path.iter()
                .try_fold(point, |value, key| value.get(*key))
                .and_then(Value::as_f64)
        };
        match (read(first), read(last)) {
            (Some(first), Some(last)) if first > 0.0 => {
                json!((last / first * 1000.0).round() / 1000.0)
            }
            _ => Value::Null,
        }
    };
    json!({
        "roots": ratio(&["roots"]),
        "firstTurnDefinitionBytes": ratio(&["firstTurn", "definitionBytes"]),
        "eagerDefinitionBytes": ratio(&["eagerReference", "definitionBytes"]),
        "discoverResultBytesP95": ratio(&["discover", "resultBytesP95"]),
        "describeResultBytesP95": ratio(&["describe", "resultBytesP95"]),
        "describeTypePageBytesP95": ratio(&["describeType", "pageBytesP95"]),
        "discoverMicrosecondsP95": ratio(&["discover", "microsecondsP95"]),
    })
}

/// One generated family stamped from the template under a new name.
fn family_name(target: usize, family: usize) -> String {
    format!(
        "{}{target}x{family}",
        WORDS[(target * 7 + family * 5) % WORDS.len()]
    )
}

fn root_token(name: &str) -> String {
    let mut root = name.to_owned();
    root[..1].make_ascii_lowercase();
    root
}

/// The template's finished SDL split into shared definitions and per-family
/// definitions, with root-type fields split the same way.
struct Template {
    shared: Vec<String>,
    family: Vec<String>,
    roots: Vec<RootTemplate>,
    semantics: &'static GraphqlSemanticCatalog,
}

struct RootTemplate {
    header: String,
    shared_fields: Vec<String>,
    family_fields: Vec<String>,
}

fn is_family(text: &str) -> bool {
    text.contains(TEMPLATE_TOKEN) || text.contains(TEMPLATE_ROOT_TOKEN)
}

fn rename(text: &str, name: &str, description: Option<&str>) -> String {
    let text = match description {
        Some(description) => text.replace(TEMPLATE_DESCRIPTION, description),
        None => text.to_owned(),
    };
    text.replace(TEMPLATE_TOKEN, name)
        .replace(TEMPLATE_ROOT_TOKEN, &root_token(name))
}

async fn template() -> Result<Template, Box<dyn std::error::Error>> {
    use async_graphql_parser::types::{TypeKind, TypeSystemDefinition};

    let database = graphql_orm::prelude::Database::connect_sqlite("sqlite::memory:").await?;
    let sdl = template::schema_builder(database).finish().sdl();
    let document = async_graphql_parser::parse_schema(&sdl)?;
    let lines = sdl.lines().collect::<Vec<_>>();
    let start_line = |description: Option<usize>, definition: usize| {
        description.map_or(definition, |line| line.min(definition))
    };
    let mut starts = Vec::new();
    for definition in &document.definitions {
        match definition {
            TypeSystemDefinition::Schema(schema) => starts.push((schema.pos.line, None)),
            TypeSystemDefinition::Directive(directive) => starts.push((
                start_line(
                    directive
                        .node
                        .description
                        .as_ref()
                        .map(|value| value.pos.line),
                    directive.pos.line,
                ),
                None,
            )),
            TypeSystemDefinition::Type(definition) => {
                let start = start_line(
                    definition
                        .node
                        .description
                        .as_ref()
                        .map(|value| value.pos.line),
                    definition.pos.line,
                );
                let root = matches!(
                    definition.node.name.node.as_str(),
                    "Query" | "Mutation" | "Subscription"
                );
                let fields = match (&definition.node.kind, root) {
                    (TypeKind::Object(object), true) => Some(
                        object
                            .fields
                            .iter()
                            .map(|field| {
                                start_line(
                                    field.node.description.as_ref().map(|value| value.pos.line),
                                    field.pos.line,
                                )
                            })
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                };
                starts.push((start, fields));
            }
        }
    }
    starts.sort_by_key(|(start, _)| *start);
    let chunk = |from: usize, to: usize| lines[from - 1..to - 1].join("\n");
    let mut template = Template {
        shared: Vec::new(),
        family: Vec::new(),
        roots: Vec::new(),
        semantics: template::graphql_orm_semantic_catalog(),
    };
    for (position, (start, fields)) in starts.iter().enumerate() {
        let end = starts
            .get(position + 1)
            .map_or(lines.len() + 1, |(next, _)| *next);
        match fields {
            None => {
                let text = chunk(*start, end);
                if is_family(&text) {
                    template.family.push(text);
                } else {
                    template.shared.push(text);
                }
            }
            Some(fields) => {
                let close = (*start..end)
                    .rev()
                    .find(|line| lines[line - 1].trim() == "}")
                    .ok_or("root type has no closing brace")?;
                let first = *fields.first().ok_or("root type has no fields")?;
                let mut root = RootTemplate {
                    header: chunk(*start, first),
                    shared_fields: Vec::new(),
                    family_fields: Vec::new(),
                };
                for (index, field) in fields.iter().enumerate() {
                    let next = fields.get(index + 1).copied().unwrap_or(close);
                    let text = chunk(*field, next);
                    if is_family(&text) {
                        root.family_fields.push(text);
                    } else {
                        root.shared_fields.push(text);
                    }
                }
                template.roots.push(root);
            }
        }
    }
    Ok(template)
}

impl Template {
    /// SDL and semantic catalogue of one target holding the named families.
    /// `revised` changes the wide entity's description of the first family.
    fn target(
        &self,
        families: &[String],
        revised: bool,
    ) -> Result<(String, GraphqlSemanticCatalog), Box<dyn std::error::Error>> {
        let description = |index: usize| {
            (revised && index == 0)
                .then(|| format!("A revised {} record with forty public fields", families[0]))
        };
        let mut sdl = self.shared.join("\n");
        for (index, name) in families.iter().enumerate() {
            for definition in &self.family {
                sdl.push('\n');
                sdl.push_str(&rename(definition, name, description(index).as_deref()));
            }
        }
        for root in &self.roots {
            sdl.push('\n');
            sdl.push_str(&root.header);
            for field in &root.shared_fields {
                sdl.push('\n');
                sdl.push_str(field);
            }
            for (index, name) in families.iter().enumerate() {
                for field in &root.family_fields {
                    sdl.push('\n');
                    sdl.push_str(&rename(field, name, description(index).as_deref()));
                }
            }
            sdl.push_str("\n}\n");
        }
        let entities_json = serde_json::to_string(&self.semantics.entities)?;
        let operations_json = serde_json::to_string(&self.semantics.operations)?;
        let mut entities = Vec::new();
        let mut operations = Vec::new();
        for (index, name) in families.iter().enumerate() {
            let description = description(index);
            entities.extend(serde_json::from_str::<Vec<GraphqlEntitySemanticMetadata>>(
                &rename(&entities_json, name, description.as_deref()),
            )?);
            for operation in serde_json::from_str::<Vec<GraphqlSemanticOperationDescriptor>>(
                &rename(&operations_json, name, description.as_deref()),
            )? {
                operations.push(rebuild_operation(operation)?);
            }
        }
        Ok((
            sdl,
            GraphqlSemanticCatalog::compose_descriptors(entities, operations)?,
        ))
    }
}

/// Recomputes a renamed generated operation through its public constructor,
/// so its fingerprint covers the new names.
fn rebuild_operation(
    operation: GraphqlSemanticOperationDescriptor,
) -> Result<GraphqlSemanticOperationDescriptor, Box<dyn std::error::Error>> {
    let mut rebuilt = GraphqlSemanticOperationDescriptor::generated(
        operation.kind,
        operation
            .generated_category
            .ok_or("template operation is not generated")?,
        operation
            .generated_entity_name
            .ok_or("template operation has no entity")?,
        operation.field_name,
        operation.description,
        operation.arguments,
        operation.result_type,
        operation.has_authorization_policy,
    )?;
    if let Some(policy) = operation.ai_mutation_execution {
        rebuilt = rebuilt.with_ai_mutation_execution(policy)?;
    }
    if let Some(observation) = operation.subscription_observation {
        rebuilt = rebuilt.with_subscription_observation(observation)?;
    }
    Ok(rebuilt)
}

/// One compiled target: its query catalogue and its index.
struct CompiledTarget {
    catalogue: AiGraphqlQueryCapabilityCatalog,
    index: Arc<AiCapabilityIndex>,
    fits_default_index_limits: bool,
}

fn compile_target(
    template: &Template,
    target: usize,
    families: &[String],
    revised: bool,
    index_limits: AiCapabilityIndexLimits,
) -> Result<CompiledTarget, Box<dyn std::error::Error>> {
    let (sdl, semantics) = template.target(families, revised)?;
    let target_id = GraphqlExecutionTargetId::parse(format!("target-{target:04}"))?;
    let catalogue = AiGraphqlQueryCapabilityCatalog::compile(
        &format!("target{target:04}"),
        target_id.clone(),
        &sdl,
        &semantics,
        AiGraphqlQueryCapabilityLimits::default(),
    )?;
    let compile = |limits| {
        AiCapabilityIndex::compile(
            target_id.clone(),
            catalogue.finished_schema_fingerprint().to_owned(),
            &semantics,
            Some(&catalogue),
            None,
            None,
            [],
            "benchmark-target-policy-v1",
            limits,
        )
    };
    let fits_default_index_limits = compile(AiCapabilityIndexLimits::default()).is_ok();
    let index = Arc::new(compile(index_limits)?);
    Ok(CompiledTarget {
        catalogue,
        index,
        fits_default_index_limits,
    })
}

const LARGE_INDEX_LIMITS: AiCapabilityIndexLimits = AiCapabilityIndexLimits {
    maximum_entries: 8_192,
    maximum_entry_bytes: 256 * 1_024,
    maximum_total_bytes: 16 * 1_024 * 1_024,
    maximum_search_results: 32,
};

const LARGE_SET_LIMITS: AiCapabilityIndexSetLimits = AiCapabilityIndexSetLimits {
    maximum_indexes: 1_024,
    maximum_entries: 1_048_576,
    maximum_total_bytes: 4 * 1_024 * 1_024 * 1_024,
    maximum_search_results: 32,
};

struct Resolver(AuthPrincipal);

#[async_trait]
impl CurrentPrincipalResolver for Resolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        ResolvedPrincipal::new(reference.clone(), self.0.clone(), OffsetDateTime::now_utc())
    }
}

/// A host that keeps one compiled index set and returns it on every call.
struct CachedSet(Arc<AiCapabilityIndexSet>);

impl AiCurrentCapabilityIndexSet for CachedSet {
    fn current_index_set(
        &self,
        _run: &AiCapabilityRunBinding,
    ) -> Result<Arc<AiCapabilityIndexSet>, AiError> {
        Ok(Arc::clone(&self.0))
    }
}

/// A host that supplies one current index, the documented single-target
/// path: the crate forms the one-member set on each call.
struct SingleIndex(Arc<AiCapabilityIndex>);

impl AiCurrentCapabilityIndex for SingleIndex {
    fn current_index(
        &self,
        _run: &AiCapabilityRunBinding,
    ) -> Result<Arc<AiCapabilityIndex>, AiError> {
        Ok(Arc::clone(&self.0))
    }
}

/// Admits every read, or only the reads of one target.
struct Policy(Option<GraphqlExecutionTargetId>);

#[async_trait]
impl AiCapabilityAuthorityPolicy for Policy {
    async fn authorize(
        &self,
        _principal: &ResolvedPrincipal,
        owning_index: &AiCapabilityIndex,
        entry: &AiCapabilityIndexEntry,
        _run: &AiCapabilityRunBinding,
    ) -> Result<AiCapabilityAuthorityDecision, AiError> {
        let target = self
            .0
            .as_ref()
            .is_none_or(|target| owning_index.target_id() == target);
        Ok(AiCapabilityAuthorityDecision {
            allowed: target && entry.risk == AiToolRisk::ReadOnly,
            policy_fingerprint: "benchmark-policy-v1".to_owned(),
        })
    }
}

fn principal() -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: "benchmark-user".to_owned(),
        session_id: Uuid::from_u128(551),
        roles: Vec::new(),
        scopes: Vec::new(),
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata::default(),
    })
}

fn run_binding() -> AiCapabilityRunBinding {
    AiCapabilityRunBinding {
        session_id: AiSessionId::new(),
        run_id: AiRunId::new(),
        attempt_id: Uuid::new_v4(),
        lease_generation: 1,
        provider_kind: ProviderKind::OpenAi,
        provider_session_fingerprint: "benchmark-session-v1".to_owned(),
    }
}

fn broker(
    indexes: Arc<dyn AiCurrentCapabilityIndexSet>,
    restrict_to: Option<GraphqlExecutionTargetId>,
) -> Result<AiCapabilityDiscoveryBroker, AiError> {
    AiCapabilityDiscoveryBroker::new(
        Arc::new(Resolver(principal())),
        indexes,
        Arc::new(Policy(restrict_to)),
        Arc::new(SystemClock),
        Duration::seconds(300),
    )
}

fn percentile(values: &mut [u64], percent: usize) -> u64 {
    values.sort_unstable();
    values
        .get((values.len() * percent / 100).min(values.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0)
}

fn bytes(value: &Value) -> u64 {
    serde_json::to_vec(value).map_or(0, |encoded| encoded.len() as u64)
}

fn elapsed_micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn fixed_broker_binding(index_set_fingerprint: &str) -> Result<String, AiError> {
    binding(AiCapabilityDeliveryMode::FixedBroker, index_set_fingerprint)
}

fn binding(mode: AiCapabilityDeliveryMode, index_set_fingerprint: &str) -> Result<String, AiError> {
    Ok(AiProviderCapabilitySessionBinding::new(
        mode,
        index_set_fingerprint,
        BTreeSet::new(),
        "benchmark-projection-v1",
        "benchmark-model",
        ModelReasoningEffort::Unspecified,
        "a".repeat(64),
    )?
    .fingerprint()
    .to_owned())
}

const TYPE_PAGE_OVER_CEILING: &str =
    "broker capability type page exceeds its bound; request a smaller limit";

/// The coordinator's signature step for one described operation
/// (`operation_signature` in `orm_tools.rs`): build and validate the exact
/// model definition, require its fingerprint, then render the signature.
fn production_signature(
    catalog: &AiToolCatalog,
    operation: &AiCapabilityDescribedOperation,
) -> Result<AiCapabilitySignature, AiError> {
    let definition = catalog.query_capability_model_definition(
        operation.capability_id(),
        capability_provider_alias(operation.capability_id()),
    )?;
    if definition.fingerprint != operation.capability_fingerprint() {
        return Err(AiError::Forbidden);
    }
    catalog.query_capability_signature(operation.capability_id())
}

/// The coordinator's describe result: every operation's signature summary,
/// merged under the describe byte bound.
fn production_describe(
    catalog: &AiToolCatalog,
    description: AiCapabilityDescription,
    limits: AiCapabilityDeliveryLimits,
) -> Result<Value, AiError> {
    let summaries = description
        .operations()
        .iter()
        .map(|operation| {
            production_signature(catalog, operation).map(|signature| signature.summary())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(description
        .with_operation_signatures(summaries, limits.maximum_describe_bytes)?
        .into_model_result())
}

/// The coordinator's `describe_type` page, refused above the describe byte
/// bound exactly as the coordinator refuses it.
fn production_type_page(
    catalog: &AiToolCatalog,
    request: &AiCapabilityTypeRequest,
    limits: AiCapabilityDeliveryLimits,
) -> Result<Value, AiError> {
    let page = request.render(&production_signature(catalog, request.operation())?)?;
    if bytes(&page) > u64::from(limits.maximum_describe_bytes) {
        return Err(AiError::InvalidInput(TYPE_PAGE_OVER_CEILING.to_owned()));
    }
    Ok(page)
}

/// Discovers, describes and expands the wide entity of each sampled family
/// through the fixed broker and the coordinator's own describe steps.
async fn broker_round_trips(
    broker: &AiCapabilityDiscoveryBroker,
    families: &[String],
    catalog: &AiToolCatalog,
) -> Result<Value, Box<dyn std::error::Error>> {
    let reference = principal().reference();
    let run = run_binding();
    let limits = AiCapabilityDeliveryLimits::default();
    let session = AiCapabilityBrokerSession::new(limits)?;
    let mut discover_bytes = Vec::new();
    let mut discover_micros = Vec::new();
    let mut entity_hits = 0_usize;
    let mut aggregate_hits = 0_usize;
    for sample in 0..DISCOVER_SAMPLES {
        let family = &families[(sample * 7919) % families.len()];
        let (text, expect_aggregate) = match sample % 4 {
            0 => (format!("list {family} records"), false),
            1 => (format!("how many {family} records"), true),
            2 => (format!("{family} part a lines by quantity"), false),
            _ => (format!("find {family} by label 07"), false),
        };
        let started = Instant::now();
        let result = broker
            .dispatch_discover(
                &reference,
                &run,
                &session,
                &json!({"text": text, "maximumResults": 8}),
            )
            .await?;
        discover_micros.push(elapsed_micros(started));
        discover_bytes.push(bytes(&result));
        if let Some(top) = result["candidates"]
            .as_array()
            .and_then(|values| values.first())
        {
            let entity = top["entity"].as_str().unwrap_or_default();
            if entity == family.as_str() || (sample % 4 == 2 && entity == format!("{family}PartA"))
            {
                entity_hits += 1;
            }
            if expect_aggregate
                && top["operations"].as_array().is_some_and(|operations| {
                    operations
                        .iter()
                        .any(|operation| operation["shape"] == "aggregate")
                })
            {
                aggregate_hits += 1;
            }
        }
    }

    let mut describe_bytes = Vec::new();
    let mut describe_micros = Vec::new();
    let mut type_page_bytes = Vec::new();
    let mut type_errors = Vec::new();
    let mut pages_over_ceiling = 0_usize;
    let mut warm_describe_micros = Vec::new();
    let mut type_page_micros = Vec::new();
    let mut keyset_example = Value::Null;
    let mut operations_per_describe = 0_usize;
    let mut keyset_continuation = false;
    for sample in 0..DESCRIBE_SAMPLES.min(families.len()) {
        let family = &families[(sample * 104_729) % families.len()];
        let discovered = broker
            .dispatch_discover(
                &reference,
                &run,
                &session,
                &json!({
                    "text": family,
                    "kind": "generated_entity",
                    "entityOrClass": family,
                    "maximumResults": 1,
                }),
            )
            .await?;
        let candidate = discovered["candidates"]
            .get(0)
            .ok_or("wide entity was not discovered")?;
        let describe = json!({
            "capabilityId": candidate["capabilityId"],
            "candidateFingerprint": candidate["candidateFingerprint"],
        });
        let started = Instant::now();
        let description = broker
            .dispatch_describe(&reference, &run, &session, &describe)
            .await?;
        let model_result = production_describe(catalog, description.clone(), limits)?;
        describe_micros.push(elapsed_micros(started));
        describe_bytes.push(bytes(&model_result));
        // The same entity described again, as a later turn would.
        let started = Instant::now();
        let again = broker
            .dispatch_describe(&reference, &run, &session, &describe)
            .await?;
        production_describe(catalog, again, limits)?;
        warm_describe_micros.push(elapsed_micros(started));
        operations_per_describe = operations_per_describe.max(description.operations().len());
        for operation in description.operations() {
            let signature = production_signature(catalog, operation)?;
            let operation = operation.operation_name();
            if signature.signature().contains("after?:")
                && signature
                    .type_names()
                    .iter()
                    .any(|name| name == "select.pageInfo")
            {
                keyset_continuation = true;
            }
            let summary = signature.summary();
            if keyset_example.is_null() && operation.ends_with("Keyset") {
                keyset_example = json!({
                    "signature": summary["signature"],
                    "select": summary["select"],
                    "typeNames": signature.type_names(),
                });
            }
            let mut names = Vec::new();
            if summary.get("select").is_some() {
                names.push("select".to_owned());
            }
            names.extend(
                summary["inputTypes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|name| name.ends_with("WhereInput"))
                    .map(str::to_owned),
            );
            for name in names {
                let started = Instant::now();
                let page = broker
                    .dispatch_describe_type(
                        &reference,
                        &run,
                        &session,
                        &json!({
                            "loadedReference": description.loaded_reference(),
                            "operation": operation,
                            "type": name,
                        }),
                    )
                    .await
                    .and_then(|request| production_type_page(catalog, &request, limits));
                type_page_micros.push(elapsed_micros(started));
                match page {
                    Ok(page) => type_page_bytes.push(bytes(&page)),
                    Err(AiError::InvalidInput(message)) if message == TYPE_PAGE_OVER_CEILING => {
                        pages_over_ceiling += 1;
                    }
                    Err(error) => type_errors.push(format!("{operation} {name}: {error}")),
                }
            }
        }
    }
    let samples = DISCOVER_SAMPLES as f64;
    Ok(json!({
        "discover": {
            "samples": DISCOVER_SAMPLES,
            "resultBytesP50": percentile(&mut discover_bytes.clone(), 50),
            "resultBytesP95": percentile(&mut discover_bytes, 95),
            "microsecondsP50": percentile(&mut discover_micros.clone(), 50),
            "microsecondsP95": percentile(&mut discover_micros, 95),
            "topEntityHitRate": entity_hits as f64 / samples,
            "countQueryTopHasAggregateRate": aggregate_hits as f64 / (samples / 4.0),
        },
        "describe": {
            "samples": describe_bytes.len(),
            "operationsPerEntity": operations_per_describe,
            "resultBytesP50": percentile(&mut describe_bytes.clone(), 50),
            "resultBytesP95": percentile(&mut describe_bytes.clone(), 95),
            "resultBytesMaximum": describe_bytes.iter().copied().max().unwrap_or(0),
            "microsecondsP95": percentile(&mut describe_micros, 95),
            "repeatMicrosecondsP95": percentile(&mut warm_describe_micros, 95),
            "keysetContinuationAvailable": keyset_continuation,
            "keysetExample": keyset_example,
        },
        "describeType": {
            "pages": type_page_bytes.len(),
            "pageBytesP50": percentile(&mut type_page_bytes.clone(), 50),
            "pageBytesP95": percentile(&mut type_page_bytes.clone(), 95),
            "pageBytesMaximum": type_page_bytes.iter().copied().max().unwrap_or(0),
            "microsecondsP95": percentile(&mut type_page_micros, 95),
            "pagesOverCeiling": pages_over_ceiling,
            "errors": type_errors,
        },
    }))
}

async fn measure_federated(targets: usize) -> Result<Value, Box<dyn std::error::Error>> {
    let template = template().await?;
    let families = (0..targets)
        .map(|target| {
            (0..FAMILIES_PER_TARGET)
                .map(|family| family_name(target, family))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    // The first target alone gives the per-target cost; the rest compile on
    // a few threads so a large point finishes in reasonable time.
    let started = Instant::now();
    let first = compile_target(&template, 0, &families[0], false, LARGE_INDEX_LIMITS)?;
    let first_target_micros = elapsed_micros(started);
    let started = Instant::now();
    let threads = COMPILE_THREADS.min(targets.saturating_sub(1)).max(1);
    // The runtime tool catalogue holds the registered capabilities, as in
    // production. Each compiled catalogue is registered as soon as a worker
    // finishes it and is then dropped, so at most one copy of each target's
    // capabilities is alive at a time.
    let mut catalog = AiToolCatalog::new();
    let mut excluded = 0_usize;
    let mut fits_default_index_limits = true;
    let mut indexes = BTreeMap::new();
    let mut register =
        |target: usize, compiled: CompiledTarget| -> Result<(), Box<dyn std::error::Error>> {
            excluded += compiled.catalogue.excluded_capabilities().len();
            fits_default_index_limits &= compiled.fits_default_index_limits;
            catalog.register_query_capability_catalog(&compiled.catalogue)?;
            indexes.insert(target, compiled.index);
            Ok(())
        };
    register(0, first)?;
    let (sender, receiver) = std::sync::mpsc::channel::<Result<(usize, CompiledTarget), String>>();
    std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        for worker in 0..threads {
            let template = &template;
            let families = &families;
            let sender = sender.clone();
            scope.spawn(move || {
                for target in (1..targets).filter(|target| target % threads == worker) {
                    let compiled = compile_target(
                        template,
                        target,
                        &families[target],
                        false,
                        LARGE_INDEX_LIMITS,
                    )
                    .map(|compiled| (target, compiled))
                    .map_err(|error| error.to_string());
                    let failed = compiled.is_err();
                    if sender.send(compiled).is_err() || failed {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for compiled in receiver {
            let (target, compiled) = compiled?;
            register(target, compiled)?;
        }
        Ok(())
    })?;
    let compile_micros = elapsed_micros(started);
    let indexes = indexes.into_values().collect::<Vec<_>>();
    if indexes.len() != targets {
        return Err("a target did not compile".into());
    }
    let started = Instant::now();
    let set = Arc::new(AiCapabilityIndexSet::compile_with_limits(
        indexes.iter().map(Arc::clone),
        LARGE_SET_LIMITS,
    )?);
    let set_compile_micros = elapsed_micros(started);
    let fits_default_set_limits = fits_default_index_limits
        && AiCapabilityIndexSet::compile(indexes.iter().map(Arc::clone)).is_ok();
    let roots = catalog.query_capabilities().count();
    let entries = set
        .indexes()
        .map(|index| index.entries().len())
        .sum::<usize>();
    let eager_bytes = catalog
        .query_capabilities()
        .map(|capability| {
            bytes(&json!({
                "name": capability_provider_alias(capability.id()),
                "description": capability.description(),
                "parameters": capability.compact_argument_schema(),
            }))
        })
        .sum::<u64>();
    let definitions = capability_broker_definitions();
    let definition_bytes = capability_definition_bytes(&definitions)?;

    let all_families = families.concat();
    let round_trips = broker_round_trips(
        &broker(Arc::new(CachedSet(Arc::clone(&set))), None)?,
        &all_families,
        &catalog,
    )
    .await?;

    // A caller restricted to one target still receives that target's
    // candidates for a broad query that matches every target.
    let restricted_target = indexes[targets - 1].target_id().clone();
    let restricted = broker(
        Arc::new(CachedSet(Arc::clone(&set))),
        Some(restricted_target.clone()),
    )?;
    let restricted_result = restricted
        .dispatch_discover(
            &principal().reference(),
            &run_binding(),
            &AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())?,
            &json!({"text": "list part lines by quantity", "maximumResults": 8}),
        )
        .await?;
    let restricted_candidates = restricted_result["candidates"]
        .as_array()
        .map_or(0, Vec::len);

    // Revise one wide entity's description in the first target only.
    let mut revised = indexes.clone();
    revised[0] = compile_target(&template, 0, &families[0], true, LARGE_INDEX_LIMITS)?.index;
    let revised_set = AiCapabilityIndexSet::compile_with_limits(revised, LARGE_SET_LIMITS)?;

    let mut report = json!({
        "targets": targets,
        "entities": targets * FAMILIES_PER_TARGET * 6,
        "roots": roots,
        "excludedRoots": excluded,
        "indexEntries": entries,
        "fitsDefaultIndexLimits": fits_default_set_limits,
        "firstTurn": {
            "definitions": definitions.len(),
            "definitionBytes": definition_bytes,
            "approximateTokens": definition_bytes / 4,
        },
        "eagerReference": {
            "definitions": roots,
            "definitionBytes": eager_bytes,
            "approximateTokens": eager_bytes / 4,
        },
        "restrictedCaller": {
            "permittedTargets": 1,
            "candidates": restricted_candidates,
            "allFromPermittedTarget": restricted_result["candidates"].as_array().is_some_and(|candidates| {
                candidates.iter().all(|candidate| {
                    candidate["capabilityId"]
                        .as_str()
                        .is_some_and(|id| set.owning_index(&AiToolId::parse(id).expect("id")).is_some_and(|index| index.target_id() == &restricted_target))
                })
            }),
        },
        "unrelatedTargetChange": {
            "indexSetFingerprintChanged": revised_set.fingerprint() != set.fingerprint(),
            "fixedBrokerBindingChanged": fixed_broker_binding(set.fingerprint())? != fixed_broker_binding(revised_set.fingerprint())?,
            "eagerBindingChanged": binding(AiCapabilityDeliveryMode::EagerExact, set.fingerprint())? != binding(AiCapabilityDeliveryMode::EagerExact, revised_set.fingerprint())?,
        },
        "startup": {
            "firstTargetCompileMicroseconds": first_target_micros,
            "remainingTargetsCompileMicroseconds": compile_micros,
            "compileThreads": threads,
            "indexSetCompileMicroseconds": set_compile_micros,
            "peakResidentKibibytes": peak_resident_kibibytes(),
        },
    });
    if let (Some(report), Some(round_trips)) = (report.as_object_mut(), round_trips.as_object()) {
        report.extend(round_trips.clone());
    }
    Ok(report)
}

/// One large target supplied through the single-index path, compared with
/// the same index supplied as a host-cached set. A first pass meets cold
/// per-capability caches; both hosts are then measured warm.
async fn measure_single_target(families: usize) -> Result<Value, Box<dyn std::error::Error>> {
    let template = template().await?;
    let names = (0..families)
        .map(|family| family_name(0, family))
        .collect::<Vec<_>>();
    let started = Instant::now();
    let target = compile_target(&template, 0, &names, false, LARGE_INDEX_LIMITS)?;
    let compile_micros = elapsed_micros(started);
    let mut catalog = AiToolCatalog::new();
    catalog.register_query_capability_catalog(&target.catalogue)?;
    let roots = catalog.query_capabilities().count();
    let single = broker(Arc::new(SingleIndex(Arc::clone(&target.index))), None)?;
    let cached_set = Arc::new(AiCapabilityIndexSet::compile_with_limits(
        [Arc::clone(&target.index)],
        LARGE_SET_LIMITS,
    )?);
    let cached = broker(Arc::new(CachedSet(cached_set)), None)?;
    // The first pass meets cold per-capability caches: its describe is each
    // capability's first. Both hosts are then measured on equally warm
    // caches, the single-index host first and the cached-set host second.
    let cold_trips = broker_round_trips(&single, &names, &catalog).await?;
    let single_trips = broker_round_trips(&single, &names, &catalog).await?;
    let cached_trips = broker_round_trips(&cached, &names, &catalog).await?;
    let host = |trips: &Value| {
        json!({
            "discoverMicrosecondsP50": trips["discover"]["microsecondsP50"],
            "discoverMicrosecondsP95": trips["discover"]["microsecondsP95"],
            "describeMicrosecondsP95": trips["describe"]["microsecondsP95"],
            "repeatDescribeMicrosecondsP95": trips["describe"]["repeatMicrosecondsP95"],
            "describeTypeMicrosecondsP95": trips["describeType"]["microsecondsP95"],
        })
    };
    Ok(json!({
        "entities": families * 6,
        "roots": roots,
        "indexEntries": target.index.entries().len(),
        "fitsDefaultIndexLimits": target.fits_default_index_limits,
        "targetCompileMicroseconds": compile_micros,
        "coldPassSingleIndexHost": host(&cold_trips),
        "singleIndexHost": host(&single_trips),
        "cachedSetHost": host(&cached_trips),
        "describe": cold_trips["describe"],
        "describeType": cold_trips["describeType"],
        "peakResidentKibibytes": peak_resident_kibibytes(),
    }))
}

/// Peak resident set size of this process, where the platform reports it.
fn peak_resident_kibibytes() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}
