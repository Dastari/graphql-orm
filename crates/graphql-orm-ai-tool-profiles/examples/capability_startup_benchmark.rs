//! Startup benchmark for generated capability catalogues and indexes.
//!
//! Compiles a synthetic target of `N` entities, one read root each, then
//! measures catalogue compilation, index compilation, search latency and peak
//! resident memory. Each `N` runs in its own child process so its peak memory
//! is its own. The result is one JSON document on standard output.
//!
//! ```text
//! cargo run --release -p graphql-orm-ai-tool-profiles \
//!     --example capability_startup_benchmark -- 250 500 1000
//! ```
//!
//! The synthetic entity has twelve public scalar fields and one bounded
//! to-many relationship. Numbers are comparable between runs of the same
//! build profile on the same machine only; the published value is the slope
//! against `N`, not any single point.

use std::{process::Command, time::Instant};

use graphql_orm_ai_tool_profiles::{
    AiCapabilityIndex, AiCapabilityIndexLimits, AiCapabilitySearchQuery,
    AiGraphqlQueryCapabilityCatalog, AiGraphqlQueryCapabilityLimits, GraphqlExecutionTargetId,
};
use graphql_orm_operation_catalog::{
    GraphqlEntitySemanticMetadata, GraphqlOperationCatalog, GraphqlOperationKind,
    GraphqlSemanticArgumentDescriptor, GraphqlSemanticCatalog, GraphqlSemanticClassification,
    GraphqlSemanticCollectionBound, GraphqlSemanticExport, GraphqlSemanticFieldMetadata,
    GraphqlSemanticOperationDescriptor, GraphqlSemanticRelationshipCardinality,
    GraphqlSemanticRelationshipDescriptor, GraphqlSemanticTypeKind, GraphqlSemanticTypeRef,
};
use serde_json::{Value, json};

const SCALAR_FIELDS: usize = 12;
const SEARCHES: usize = 200;
const DEFAULT_ENTITY_COUNTS: [usize; 3] = [250, 500, 1_000];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let [flag, entities] = arguments.as_slice()
        && flag == "--point"
    {
        println!("{}", measure(entities.parse()?)?);
        return Ok(());
    }
    let entity_counts = if arguments.is_empty() {
        DEFAULT_ENTITY_COUNTS.to_vec()
    } else {
        arguments
            .iter()
            .map(|value| value.parse())
            .collect::<Result<Vec<usize>, _>>()?
    };
    let executable = std::env::current_exe()?;
    let mut points = Vec::new();
    for entities in entity_counts {
        let output = Command::new(&executable)
            .args(["--point", &entities.to_string()])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "benchmark point {entities} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        points.push(serde_json::from_slice::<Value>(&output.stdout)?);
    }
    let report = json!({
        "benchmark": "capability_startup",
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "scalarFieldsPerEntity": SCALAR_FIELDS,
        "slopePerEntity": slope(&points),
        "points": points,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Change per added entity between the smallest and the largest point.
fn slope(points: &[Value]) -> Value {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Value::Null;
    };
    let entities =
        last["entities"].as_f64().unwrap_or(0.0) - first["entities"].as_f64().unwrap_or(0.0);
    if entities <= 0.0 {
        return Value::Null;
    }
    let per_entity = |key: &str| match (first[key].as_f64(), last[key].as_f64()) {
        (Some(first), Some(last)) => json!((last - first) / entities),
        _ => Value::Null,
    };
    json!({
        "catalogueCompileMicroseconds": per_entity("catalogueCompileMicroseconds"),
        "indexCompileMicroseconds": per_entity("indexCompileMicroseconds"),
        "searchP95Microseconds": per_entity("searchP95Microseconds"),
        "peakResidentKibibytes": per_entity("peakResidentKibibytes"),
    })
}

fn measure(entities: usize) -> Result<Value, Box<dyn std::error::Error>> {
    let (sdl, semantics) = synthetic_target(entities)?;
    let target = GraphqlExecutionTargetId::parse("synthetic.graphql")?;
    let limits = AiGraphqlQueryCapabilityLimits::new(
        4,
        64,
        64,
        8,
        4_096,
        100,
        1_000,
        1024 * 1024,
        8_192,
        16 * 1024 * 1024,
    )?;

    let started = Instant::now();
    let catalogue = AiGraphqlQueryCapabilityCatalog::compile(
        "synthetic",
        target.clone(),
        &sdl,
        &semantics,
        limits,
    )?;
    let catalogue_compile = started.elapsed();

    let started = Instant::now();
    let index = AiCapabilityIndex::compile(
        target,
        catalogue.finished_schema_fingerprint().to_owned(),
        &semantics,
        Some(&catalogue),
        None,
        None,
        [],
        "synthetic-policy",
        AiCapabilityIndexLimits {
            maximum_entries: 8_192,
            maximum_entry_bytes: 256 * 1_024,
            maximum_total_bytes: 16 * 1_024 * 1_024,
            maximum_search_results: 32,
        },
    )?;
    let index_compile = started.elapsed();

    let mut searches = Vec::with_capacity(SEARCHES);
    for search in 0..SEARCHES {
        let query = AiCapabilitySearchQuery {
            text: format!(
                "read synthetic record {} reference",
                search % entities.max(1)
            ),
            namespace: None,
            kind: None,
            entity_or_operation: None,
            maximum_results: 8,
        };
        let started = Instant::now();
        let result = index.search(&query)?;
        searches.push(started.elapsed().as_micros());
        if result.candidates.is_empty() {
            return Err("synthetic search matched nothing".into());
        }
    }
    searches.sort_unstable();

    let capability = catalogue
        .capabilities()
        .next()
        .ok_or("synthetic catalogue is empty")?;
    let compact_bytes = serde_json::to_vec(capability.compact_argument_schema())?.len();
    let shared_bytes =
        serde_json::to_vec(&capability.shared_type_plan_schema()?.to_model_schema())?.len();

    Ok(json!({
        "entities": entities,
        "capabilities": catalogue.capabilities().count(),
        "excludedCapabilities": catalogue.excluded_capabilities().len(),
        "indexEntries": index.entries().len(),
        "catalogueCompileMicroseconds": u64::try_from(catalogue_compile.as_micros())?,
        "indexCompileMicroseconds": u64::try_from(index_compile.as_micros())?,
        "searchP50Microseconds": u64::try_from(searches[searches.len() / 2])?,
        "searchP95Microseconds": u64::try_from(searches[searches.len() * 95 / 100])?,
        "describeCompactSchemaBytes": compact_bytes,
        "describeSharedTypeSchemaBytes": shared_bytes,
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

fn scalar(name: &str) -> GraphqlSemanticFieldMetadata {
    GraphqlSemanticFieldMetadata {
        field_name: name.to_owned(),
        description: format!("Public {name} reference value."),
        type_ref: GraphqlSemanticTypeRef::named("String", GraphqlSemanticTypeKind::Scalar, false),
        selectable: true,
        filter_operators: Vec::new(),
        filter_input: None,
        sortable: false,
        groupable: false,
        aggregate_operators: Vec::new(),
        aggregate_value_kind: None,
        relationship: None,
        classification: GraphqlSemanticClassification::Internal,
        export: GraphqlSemanticExport::Exportable,
        has_field_policy: false,
    }
}

fn synthetic_target(
    entities: usize,
) -> Result<(String, GraphqlSemanticCatalog), Box<dyn std::error::Error>> {
    let mut sdl = String::from("schema { query: Query }\n");
    let mut roots = String::from("type Query {\n");
    let mut types = String::from(
        "type Note { id: String!, text: String! }\n\
         type NoteConnection { edges: [NoteEdge!]!, pageInfo: PageInfo! }\n\
         type NoteEdge { node: Note!, cursor: String! }\n\
         type PageInfo { totalCount: Int!, hasNextPage: Boolean! }\n\
         input PageInput { limit: Int, offset: Int }\n",
    );
    let mut semantic_entities = vec![GraphqlEntitySemanticMetadata {
        generated_input_members: Vec::new(),
        entity_name: "Note".to_owned(),
        description: "A bounded note record.".to_owned(),
        default_classification: GraphqlSemanticClassification::Internal,
        fields: vec![scalar("id"), scalar("text")].into_boxed_slice(),
        label_fields: Vec::new(),
        record_version: None,
    }];
    let mut operations = Vec::new();
    for entity in 0..entities {
        let name = format!("SyntheticRecord{entity}");
        roots.push_str(&format!("  Read{name}(id: ID!): {name}!\n"));
        types.push_str(&format!("type {name} {{\n  id: String!\n"));
        let mut fields = vec![scalar("id")];
        for field in 1..SCALAR_FIELDS {
            let field = format!("value{field}");
            types.push_str(&format!("  {field}: String!\n"));
            fields.push(scalar(&field));
        }
        types.push_str("  notes(page: PageInput): NoteConnection!\n}\n");
        fields.push(GraphqlSemanticFieldMetadata {
            field_name: "notes".to_owned(),
            description: "Bounded related note records.".to_owned(),
            type_ref: GraphqlSemanticTypeRef::list(
                false,
                Some(10),
                GraphqlSemanticTypeRef::named("Note", GraphqlSemanticTypeKind::Object, false),
            ),
            relationship: Some(GraphqlSemanticRelationshipDescriptor {
                target: "Note".to_owned(),
                cardinality: GraphqlSemanticRelationshipCardinality::Many,
                arguments: vec![GraphqlSemanticArgumentDescriptor {
                    graphql_name: "page".to_owned(),
                    description: "Bounded relationship page.".to_owned(),
                    type_ref: GraphqlSemanticTypeRef::named(
                        "PageInput",
                        GraphqlSemanticTypeKind::Object,
                        true,
                    ),
                }],
                collection_bound: Some(GraphqlSemanticCollectionBound::pageable("page")),
            }),
            ..scalar("notes")
        });
        semantic_entities.push(GraphqlEntitySemanticMetadata {
            generated_input_members: Vec::new(),
            entity_name: name.clone(),
            description: format!("Synthetic record {entity}."),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: fields.into_boxed_slice(),
            label_fields: Vec::new(),
            record_version: None,
        });
        operations.push(GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Query,
            format!("Read{name}"),
            format!("Read one synthetic record {entity} by reference."),
            vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "id".to_owned(),
                description: "Exact public record identity.".to_owned(),
                type_ref: GraphqlSemanticTypeRef::named(
                    "ID",
                    GraphqlSemanticTypeKind::Scalar,
                    false,
                ),
            }],
            GraphqlSemanticTypeRef::named(name, GraphqlSemanticTypeKind::Object, false),
            true,
        )?);
    }
    roots.push_str("}\n");
    sdl.push_str(&roots);
    sdl.push_str(&types);
    let semantics = GraphqlSemanticCatalog::compose_with_custom(
        semantic_entities,
        &GraphqlOperationCatalog::compose(std::iter::empty::<(
            &'static [graphql_orm_operation_catalog::GeneratedGraphqlOperationDescriptor],
            bool,
            bool,
        )>()),
        operations,
    )?;
    Ok((sdl, semantics))
}
