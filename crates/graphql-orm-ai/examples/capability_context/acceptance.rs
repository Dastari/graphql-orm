//! Deterministic question-to-execution driver for the context benchmark.

use super::{
    CachedSet, LARGE_INDEX_LIMITS, LARGE_SET_LIMITS, broker, bytes, compile_target,
    compile_target_sdl, elapsed_micros, principal, production_describe, production_type_page,
    run_binding, split_template, template,
};
use async_graphql::{Request, Variables};
use graphql_orm::prelude::{AuthSubject, Database};
use graphql_orm_ai::*;
use graphql_orm_operation_catalog::GeneratedGraphqlOperationCategory;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error, sync::Arc, time::Instant};

type BenchResult<T> = Result<T, Box<dyn Error>>;

const TARGETS: usize = 2;
const FAMILY: &str = "Zqxwide";
const DISTRACTORS_PER_TARGET: usize = 100;
const MAXIMUM_RESULTS: usize = 8;
const CASES: &str = include_str!("questions.json");

mod distractor {
    use graphql_orm::prelude::*;

    // Give the distractors real generated query operations and overlapping
    // search terms, without duplicating the wide schemas 40,000 times.
    #[derive(
        GraphQLEntity,
        GraphQLRelations,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
    )]
    #[graphql_entity(
        table = "archive_records",
        plural = "Zqxwides",
        default_sort = "id ASC",
        description = "Archived record identity and related part references"
    )]
    pub struct Zqxwide {
        #[primary_key]
        #[graphql_orm(auto_generated = false, description = "Stable record identity")]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
    }

    schema_roots! {
        entities: [Zqxwide],
    }
}

#[derive(Deserialize)]
struct Question {
    text: String,
    expected: Vec<Answer>,
}

#[derive(Deserialize)]
struct Answer {
    target: String,
    entity: String,
    category: GeneratedGraphqlOperationCategory,
    result: Value,
}

/// The driver reads the public question only. The expected coordinates and
/// resolver results are consumed by the scorer after execution.
struct Intent<'a> {
    text: &'a str,
    namespace: &'a str,
    entity: &'a str,
    shape: &'static str,
    selections: &'static [&'static str],
}

impl<'a> Intent<'a> {
    fn parse(text: &'a str) -> BenchResult<Self> {
        let (namespace, request) = text
            .trim_end_matches('?')
            .strip_prefix("In ")
            .and_then(|text| text.split_once(", "))
            .ok_or("invalid question syntax")?;
        let (prefix, shape, selections) = [
            ("list IDs for ", "list", &["id"][..]),
            ("page through IDs for ", "keyset_list", &["id"][..]),
            ("how many ", "aggregate", &[][..]),
        ]
        .into_iter()
        .find(|(prefix, _, _)| request.starts_with(prefix))
        .ok_or("unknown benchmark question intent")?;
        let entity = request[prefix.len()..]
            .split_whitespace()
            .next()
            .ok_or("question has no entity")?;
        Ok(Self {
            text,
            namespace,
            entity,
            shape,
            selections,
        })
    }

    fn select<'b>(&self, result: &'b Value) -> Option<&'b Value> {
        result["candidates"].as_array()?.iter().find(|candidate| {
            candidate["namespace"] == self.namespace && candidate["entity"] == self.entity
        })
    }
}

struct Fixture {
    catalog: AiToolCatalog,
    indexes: Arc<AiCapabilityIndexSet>,
    schemas: BTreeMap<GraphqlExecutionTargetId, template::AppSchema>,
}

impl Fixture {
    async fn compile(entries: usize) -> BenchResult<Self> {
        let semantics = template::graphql_orm_semantic_catalog();
        let mut catalog = AiToolCatalog::new();
        let mut schemas = BTreeMap::new();
        let mut indexes = Vec::new();
        let mut count = 0;
        for target in 0..TARGETS {
            let schema = seeded_schema(target).await?;
            let compiled =
                compile_target_sdl(target, &schema.sdl(), semantics, LARGE_INDEX_LIMITS)?;
            if !compiled.catalogue.diagnostics().is_empty()
                || !compiled.catalogue.excluded_capabilities().is_empty()
            {
                return Err("executable fixture excluded generated query roots".into());
            }
            count += compiled.index.entries().len();
            catalog.register_query_capability_catalog(&compiled.catalogue)?;
            schemas.insert(compiled.index.target_id().clone(), schema);
            indexes.push(compiled.index);
        }

        let database = Database::connect_sqlite("sqlite::memory:").await?;
        let lean = split_template(
            &distractor::schema_builder(database).finish().sdl(),
            distractor::graphql_orm_semantic_catalog(),
        )?;
        for (batch, start) in (count..entries).step_by(DISTRACTORS_PER_TARGET).enumerate() {
            let names = (start..entries.min(start + DISTRACTORS_PER_TARGET))
                .enumerate()
                .map(|(offset, index)| match offset {
                    0 => FAMILY.to_owned(),
                    1 => format!("{FAMILY}PartA"),
                    2 => format!("{FAMILY}PartB"),
                    _ => format!("{FAMILY}Archive{index}"),
                })
                .collect::<Vec<_>>();
            let compiled =
                compile_target(&lean, TARGETS + batch, &names, false, LARGE_INDEX_LIMITS)?;
            count += compiled.index.entries().len();
            indexes.push(compiled.index);
        }
        if count != entries {
            return Err(format!("requested {entries} entries, compiled {count}").into());
        }
        let indexes = Arc::new(AiCapabilityIndexSet::compile_with_limits(
            indexes,
            LARGE_SET_LIMITS,
        )?);
        Ok(Self {
            catalog,
            indexes,
            schemas,
        })
    }

    async fn execute(&self, execution: AiCapabilityExecution) -> BenchResult<ExecutionResult> {
        let (capability, fingerprint) = (
            execution.capability_id().clone(),
            execution.capability_fingerprint().to_owned(),
        );
        let paged = self
            .catalog
            .query_capability_supports_continuation(&capability);
        let compiled = if paged {
            self.catalog.compile_query_capability_page(
                &capability,
                &fingerprint,
                execution.plan().clone(),
                execution.continuation_cursor(),
            )?
        } else {
            self.catalog.compile_query_capability(
                &capability,
                &fingerprint,
                execution.into_plan(),
            )?
        };
        let root = self
            .catalog
            .query_capabilities()
            .find(|root| root.id() == &capability)
            .ok_or("compiled root missing")?;
        let contract = compiled
            .descriptor()
            .graphql_contract
            .as_ref()
            .ok_or("no GraphQL binding")?;
        let schema = self
            .schemas
            .get(&contract.target_id)
            .ok_or("no executable fixture for target")?;
        let response = schema
            .execute(
                Request::new(&compiled.descriptor().document)
                    .operation_name(&contract.operation_name)
                    .variables(Variables::from_json(compiled.variables().clone()))
                    .data(AuthSubject::new(principal().subject())),
            )
            .await;
        if !response.errors.is_empty() {
            return Err(format!("resolver: {:?}", response.errors).into());
        }
        let mut data = response.data.into_json()?;
        compiled.take_page_info(&mut data)?;
        let result = data
            .as_object()
            .filter(|object| object.len() == 1)
            .and_then(|object| object.values().next())
            .ok_or("expected one GraphQL result")?;
        Ok(ExecutionResult {
            target: contract.target_id.as_str().to_owned(),
            entity: root
                .generated_entity_name()
                .ok_or("no generated entity")?
                .to_owned(),
            category: root.generated_category().ok_or("no generated category")?,
            result: result.clone(),
        })
    }
}

async fn seeded_schema(target: usize) -> BenchResult<template::AppSchema> {
    use graphql_orm::graphql::orm::{DatabaseBackend, SchemaModel, build_migration_plan};
    use graphql_orm::sqlx;

    let db = Database::connect_sqlite("sqlite::memory:").await?;
    let desired = SchemaModel::from_entities(&template::graphql_orm_entity_metadata());
    let empty = SchemaModel {
        tables: Vec::new(),
        extensions: Vec::new(),
    };
    let plan = build_migration_plan(DatabaseBackend::Sqlite, &empty, &desired);
    for statement in &plan.statements {
        sqlx::query(statement).execute(db.pool()).await?;
    }
    // The generated wide schema requires all non-null scalar columns. Seed
    // their neutral values once; the broker only reads the stable IDs.
    let labels = (1..=19).map(|i| format!("label{i:02}"));
    let numbers = (1..=10)
        .map(|i| format!("amount{i:02}"))
        .chain((1..=5).map(|i| format!("flag{i:02}")));
    let columns = labels.chain(numbers).collect::<Vec<_>>().join(", ");
    let values = ["''"; 19]
        .into_iter()
        .chain(["0"; 15])
        .collect::<Vec<_>>()
        .join(", ");
    for row in 1..=2 + target {
        sqlx::query(&format!(
            "INSERT INTO zqxwides (id, {columns}) VALUES (?, {values})"
        ))
        .bind(format!("t{target}-record-{row}"))
        .execute(db.pool())
        .await?;
    }
    for (part_index, part) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
        for row in 1..=2 + target + part_index {
            sqlx::query(&format!("INSERT INTO zqxwide_parts_{part} (id, zqxwide_id, title, quantity) VALUES (?, ?, '', 1)"))
                .bind(format!("t{target}-{part}-{row}")).bind(format!("t{target}-record-1"))
                .execute(db.pool()).await?;
        }
    }
    Ok(template::schema_builder(db).finish())
}

#[derive(serde::Serialize)]
struct ExecutionResult {
    target: String,
    entity: String,
    category: GeneratedGraphqlOperationCategory,
    result: Value,
}

#[derive(Clone, Copy, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum DiscoveryMode {
    Default,
    Scoped,
    Inline,
}

struct Trial<'a> {
    fixture: &'a Fixture,
    broker: &'a AiCapabilityDiscoveryBroker,
    run: AiCapabilityRunBinding,
    session: AiCapabilityBrokerSession,
    mode: DiscoveryMode,
    calls: Vec<Value>,
    unscoped_discover_micros: Vec<u64>,
    first_execute: Option<usize>,
    selection: Option<Value>,
}

impl<'a> Trial<'a> {
    fn new(
        fixture: &'a Fixture,
        broker: &'a AiCapabilityDiscoveryBroker,
        mode: DiscoveryMode,
    ) -> BenchResult<Self> {
        let mut session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())?;
        if mode == DiscoveryMode::Inline {
            session = session.with_scoped_discovery_descriptions();
        }
        Ok(Self {
            fixture,
            broker,
            run: run_binding(),
            session,
            mode,
            calls: Vec::new(),
            unscoped_discover_micros: Vec::new(),
            first_execute: None,
            selection: None,
        })
    }

    fn record<T>(
        &mut self,
        operation: &str,
        result: &BenchResult<T>,
        started: Instant,
        response_bytes: u64,
    ) {
        self.calls.push(json!({
            "operation": operation,
            "microseconds": elapsed_micros(started),
            "metadataBytes": response_bytes,
            "error": result.as_ref().err().map(ToString::to_string),
        }));
    }

    async fn discover(&mut self, arguments: Value) -> BenchResult<Value> {
        let started = Instant::now();
        let result = self
            .broker
            .dispatch_discover_with_catalog(
                &principal().reference(),
                &self.run,
                &self.session,
                &arguments,
                &self.fixture.catalog,
                64 * 1024,
            )
            .await
            .map_err(Into::into);
        if arguments["namespace"].is_null() {
            self.unscoped_discover_micros.push(elapsed_micros(started));
        }
        self.calls.push(json!({
            "operation": "discover",
            "namespace": arguments["namespace"],
            "microseconds": elapsed_micros(started),
            "metadataBytes": result.as_ref().map_or(0, bytes),
            "inlineDescription": result.as_ref().is_ok_and(|v| v["candidates"][0]["inlineDescription"].is_object()),
            "candidates": result.as_ref().ok().and_then(|result| result["candidates"].as_array()).map(Vec::len),
            "error": result.as_ref().err().map(ToString::to_string),
        }));
        result
    }

    async fn describe(&mut self, candidate: &Value) -> BenchResult<Value> {
        let started = Instant::now();
        let result = self
            .broker
            .dispatch_describe(
                &principal().reference(),
                &self.run,
                &self.session,
                &json!({
                    "capabilityId": candidate["capabilityId"],
                    "candidateReference": candidate["candidateReference"],
                }),
            )
            .await
            .and_then(|description| {
                production_describe(
                    &self.fixture.catalog,
                    description,
                    AiCapabilityDeliveryLimits::default(),
                )
            })
            .map_err(Into::into);
        self.record(
            "describe",
            &result,
            started,
            result.as_ref().map_or(0, bytes),
        );
        result
    }

    async fn describe_type(
        &mut self,
        description: &Value,
        operation: &Value,
        name: &str,
    ) -> BenchResult<Value> {
        let started = Instant::now();
        let result = self.broker.dispatch_describe_type(
            &principal().reference(), &self.run, &self.session,
            &json!({"loadedReference": description["loadedReference"], "operation": operation["operation"], "type": name}),
        ).await.and_then(|request| {
            production_type_page(&self.fixture.catalog, &request, AiCapabilityDeliveryLimits::default())
        }).map_err(Into::into);
        self.record(
            "describe_type",
            &result,
            started,
            result.as_ref().map_or(0, bytes),
        );
        result
    }

    async fn execute(&mut self, arguments: &Value) -> BenchResult<ExecutionResult> {
        let started = Instant::now();
        let result = match self
            .broker
            .authorize_broker_execution(
                &principal().reference(),
                &self.run,
                &self.session,
                arguments,
            )
            .await
        {
            Ok(execution) => self.fixture.execute(execution).await,
            Err(error) => Err(error.into()),
        };
        self.record("execute", &result, started, 0);
        if result.is_ok() && self.first_execute.is_none() {
            self.first_execute = Some(self.calls.len());
        }
        result
    }

    async fn answer(
        &mut self,
        intent: Intent<'_>,
        stale_control: bool,
    ) -> BenchResult<ExecutionResult> {
        let scoped = self.mode != DiscoveryMode::Default && intent.shape != "aggregate";
        let mut query = json!({"text": intent.text, "maximumResults": MAXIMUM_RESULTS});
        if scoped {
            query["namespace"] = json!(intent.namespace);
            query["entityOrClass"] = json!(intent.entity);
            query["kind"] = json!("generated_entity");
        }
        let result = self.discover(query).await;
        let candidate = if let Some(candidate) =
            result.as_ref().ok().and_then(|value| intent.select(value))
        {
            candidate.clone()
        } else {
            let result = self.discover(json!({
                "text": intent.text, "namespace": intent.namespace, "maximumResults": MAXIMUM_RESULTS,
            })).await?;
            intent
                .select(&result)
                .cloned()
                .ok_or("requested entity missing from scoped discovery")?
        };
        let inline = &candidate["inlineDescription"];
        if scoped
            && let Ok(operation) = select_operation(&intent, inline)
            && let Some(recipe) = operation["idRead"].as_object()
            && inline["loadedReference"].is_string()
        {
            let mut args = Value::Object(recipe.clone());
            args["loadedReference"] = inline["loadedReference"].clone();
            args["operation"] = operation["operation"].clone();
            self.record_selection(&candidate, operation);
            if let Ok(result) = self.execute(&args).await {
                return Ok(result);
            }
        }

        let description = self.describe(&candidate).await?;
        let operation = select_operation(&intent, &description)?;
        self.record_selection(&candidate, operation);
        let selections = intent
            .selections
            .iter()
            .map(|requested| {
                operation["select"]
                    .as_str()
                    .into_iter()
                    .flat_map(|preview| preview.split(", "))
                    .find(|path| path.eq_ignore_ascii_case(requested))
                    .ok_or("requested scalar path missing from description")
            })
            .collect::<Result<Vec<_>, _>>()?;
        operation["maximumItems"]
            .as_object()
            .ok_or("no advertised row bound")?;
        let mut args = json!({
            "loadedReference": description["loadedReference"],
            "operation": operation["operation"],
            "selections": selections,
            "maximumItems": 1,
        });
        if intent.shape == "aggregate" {
            if let Some(example) = operation["ungroupedCount"].as_object() {
                let mut count_args = Value::Object(example.clone());
                count_args["loadedReference"] = args["loadedReference"].clone();
                count_args["operation"] = args["operation"].clone();
                args = count_args;
            } else {
                args["arguments"] = self.count_arguments(&description, operation).await?;
            }
        }
        if stale_control {
            let mut stale = args.clone();
            stale["loadedReference"] = json!("0".repeat(64));
            let error = self
                .execute(&stale)
                .await
                .err()
                .ok_or("stale reference executed")?;
            if !error.to_string().contains(AI_CAPABILITY_SELECTION_STALE) {
                return Err(error);
            }
        }
        self.execute(&args).await
    }

    fn record_selection(&mut self, candidate: &Value, operation: &Value) {
        self.selection = Some(json!({
            "namespace": candidate["namespace"],
            "entity": candidate["entity"],
            "shape": operation["shape"],
        }));
    }

    async fn count_arguments(
        &mut self,
        description: &Value,
        operation: &Value,
    ) -> BenchResult<Value> {
        let signature = operation["signature"]
            .as_str()
            .ok_or("no aggregate signature")?;
        let (argument, metric_type) = signature
            .split_once('(')
            .and_then(|(_, args)| args.split_once(')'))
            .into_iter()
            .flat_map(|(args, _)| args.split(", "))
            .filter_map(|arg| arg.split_once(": ["))
            .find_map(|(arg, kind)| {
                let kind = kind.strip_suffix(']')?;
                (arg == "metrics").then_some((arg, kind))
            })
            .ok_or("no metrics array in signature")?;
        let metric = self
            .describe_type(description, operation, metric_type)
            .await?;
        let operator = metric["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|field| field["name"] == "operator")
            .ok_or("no metric operator field in expanded type")?;
        let operator_name = operator["name"].as_str().ok_or("no operator name")?;
        let operator_type = operator["type"].as_str().ok_or("no operator type")?;
        let operators = self
            .describe_type(description, operation, operator_type)
            .await?;
        let count = operators["values"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|value| **value == "COUNT")
            .ok_or("COUNT not advertised by expanded operator type")?;
        Ok(json!([{"name": format!("{argument}.0.{operator_name}"), "value": count}]))
    }
}

fn select_operation<'a>(intent: &Intent<'_>, description: &'a Value) -> BenchResult<&'a Value> {
    let mut operations = description["operations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|operation| operation["shape"] == intent.shape)
        .filter(|operation| intent.shape != "keyset_list" || operation["continuation"] == true);
    let operation = operations
        .next()
        .ok_or("requested operation absent from description")?;
    if operations.next().is_some() {
        return Err("requested operation is ambiguous".into());
    }
    Ok(operation)
}

fn nearest_rank(samples: &mut [u64], percent: usize) -> Option<u64> {
    samples.sort_unstable();
    samples
        .get((samples.len() * percent).div_ceil(100).saturating_sub(1))
        .copied()
}

pub(super) async fn measure(entries: usize, flag: &str) -> BenchResult<Value> {
    let mode = match flag {
        "--acceptance-inline" => DiscoveryMode::Inline,
        "--acceptance-scoped" => DiscoveryMode::Scoped,
        _ => DiscoveryMode::Default,
    };
    if !matches!(entries, 400 | 40_000) {
        return Err("--acceptance requires 400 or 40000 entries".into());
    }
    let started = Instant::now();
    let fixture = Fixture::compile(entries).await?;
    let startup = elapsed_micros(started);
    let host = broker(Arc::new(CachedSet(Arc::clone(&fixture.indexes))), None)?;
    let questions = serde_json::from_str::<Vec<Question>>(CASES)?;
    let mut cases = Vec::new();
    let mut call_samples = Vec::new();
    let mut discovery = Vec::new();
    let mut correct_shapes = 0;
    let mut successful_tasks = 0;
    for question in &questions {
        let intents = question
            .text
            .split("; then ")
            .map(Intent::parse)
            .collect::<BenchResult<Vec<_>>>()?;
        if intents.len() != question.expected.len() {
            return Err("question and answer lengths disagree".into());
        }
        let mut trial = Trial::new(&fixture, &host, mode)?;
        let mut correct_shape = true;
        let mut correct_result = true;
        let mut legs = Vec::new();
        for (intent, expected) in intents.into_iter().zip(&question.expected) {
            let result = trial.answer(intent, false).await;
            let selection = trial.selection.take();
            // Score the advertised shape even when execution refuses the plan.
            correct_shape &= selection.as_ref().is_some_and(|selected| {
                selected["namespace"] == expected.target.replace('-', "")
                    && selected["entity"] == expected.entity
                    && selected["shape"] == json!(expected.category)
            });
            match result {
                Ok(actual) => {
                    correct_result &= actual.target == expected.target
                        && actual.entity == expected.entity
                        && actual.category == expected.category
                        && actual.result == expected.result;
                    legs.push(json!({"selection": selection, "result": actual}));
                }
                Err(error) => {
                    correct_result = false;
                    legs.push(json!({"selection": selection, "error": error.to_string()}));
                }
            }
        }
        correct_shapes += usize::from(correct_shape);
        successful_tasks += usize::from(correct_shape && correct_result);
        call_samples.extend(trial.first_execute.map(|count| count as u64));
        discovery.extend(trial.unscoped_discover_micros);
        cases.push(json!({
            "question": question.text, "correctShape": correct_shape, "taskSuccess": correct_shape && correct_result,
            "callsToFirstSuccessfulExecute": trial.first_execute, "calls": trial.calls, "legs": legs,
        }));
    }

    let mut control = Trial::new(&fixture, &host, DiscoveryMode::Default)?;
    control
        .answer(Intent::parse(&questions[0].text)?, true)
        .await?;
    let control_calls = control.first_execute.ok_or("control did not execute")?;
    if control_calls != control.calls.len()
        || control.calls[control_calls - 2]["operation"] != "execute"
        || control.calls[control_calls - 2]["error"].is_null()
    {
        return Err("the failed execute was absent from the control's count".into());
    }

    let restricted_target = GraphqlExecutionTargetId::parse("target-0001")?;
    let restricted_host = broker(
        Arc::new(CachedSet(Arc::clone(&fixture.indexes))),
        Some(restricted_target),
    )?;
    let mut restricted = Trial::new(&fixture, &restricted_host, mode)?;
    let permitted = restricted
        .discover(json!({
            "text": "record identity", "maximumResults": MAXIMUM_RESULTS,
        }))
        .await?;
    let candidates = permitted["candidates"]
        .as_array()
        .ok_or("missing candidate list")?;

    let all_calls = cases
        .iter()
        .flat_map(|case| case["calls"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    let mut time_to_first = cases
        .iter()
        .filter_map(|case| {
            let count = usize::try_from(case["callsToFirstSuccessfulExecute"].as_u64()?).ok()?;
            Some(
                case["calls"]
                    .as_array()?
                    .iter()
                    .take(count)
                    .filter_map(|call| call["microseconds"].as_u64())
                    .sum::<u64>(),
            )
        })
        .collect::<Vec<_>>();
    let mut discovery_times = all_calls
        .iter()
        .filter(|call| call["operation"] == "discover")
        .filter_map(|call| call["microseconds"].as_u64())
        .collect::<Vec<_>>();
    let p50 = nearest_rank(&mut call_samples, 50);
    let p95 = nearest_rank(&mut call_samples, 95);
    let shape_rate = correct_shapes as f64 / questions.len() as f64;
    let discover_p95 = nearest_rank(&mut discovery, 95);
    Ok(json!({
        "mode": "deterministic", "brokerVersion": AI_CAPABILITY_BROKER_VERSION,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "discoveryMode": mode,
        "modelRuns": 0, "indexEntries": entries, "targets": fixture.indexes.indexes().len(),
        "startupMicroseconds": startup, "peakResidentKibibytes": super::peak_resident_kibibytes(),
        "questions": questions.len(), "correctShape": correct_shapes, "correctShapeRate": shape_rate,
        "taskSuccesses": successful_tasks,
        "firstSuccessfulExecute": {
            "samples": call_samples.len(), "withoutSuccess": questions.len() - call_samples.len(),
            "brokerCallsP50": p50, "brokerCallsP95": p95,
            "microsecondsP50": nearest_rank(&mut time_to_first, 50),
            "microsecondsP95": nearest_rank(&mut time_to_first, 95),
        },
        "corpusCalls": {
            "total": all_calls.len(),
            "failed": all_calls.iter().filter(|call| !call["error"].is_null()).count(),
            "describe": all_calls.iter().filter(|call| call["operation"] == "describe").count(),
            "describeType": all_calls.iter().filter(|call| call["operation"] == "describe_type").count(),
            "inlineDescriptions": all_calls.iter().filter(|call| call["inlineDescription"] == true).count(),
            "metadataBytes": all_calls.iter().filter_map(|call| call["metadataBytes"].as_u64()).sum::<u64>(),
            "discoverMetadataBytes": all_calls.iter().filter(|call| call["operation"] == "discover").filter_map(|call| call["metadataBytes"].as_u64()).sum::<u64>(),
            "discoverP95Microseconds": nearest_rank(&mut discovery_times, 95),
        },
        "unscopedDiscoverP95Microseconds": discover_p95,
        "restrictedCaller": {
            "candidates": candidates.len(),
            "allPermitted": !candidates.is_empty() && candidates.iter().all(|c| c["namespace"] == "target0001"),
        },
        "thresholds": {
            "correctShapeAtLeast95Percent": shape_rate >= 0.95,
            "firstExecuteP50AtMost2": call_samples.len() == questions.len() && p50.is_some_and(|p| p <= 2),
            "firstExecuteP95AtMost4": call_samples.len() == questions.len() && p95.is_some_and(|p| p <= 4),
            "discoverP95AtMost10Ms": discover_p95.is_some_and(|p| p <= 10_000),
        },
        "staleExecuteControl": {"callsToFirstSuccessfulExecute": control_calls, "calls": control.calls},
        "cases": cases,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::{Resolver, production_signature};
    use super::*;
    use agql_auth::{ResolvedPrincipal, SystemClock};
    use std::sync::atomic::{AtomicU8, Ordering};
    use time::Duration;

    fn scoped_query() -> Value {
        json!({
            "text": "list IDs for Zqxwide", "namespace": "target0000",
            "entityOrClass": "Zqxwide", "kind": "generated_entity", "maximumResults": 8,
        })
    }

    fn describe_args(candidate: &Value) -> Value {
        json!({
            "capabilityId": candidate["capabilityId"],
            "candidateReference": candidate["candidateReference"],
        })
    }

    #[tokio::test]
    async fn scoped_discovery_can_issue_a_real_description() -> BenchResult<()> {
        let fixture = Fixture::compile(400).await?;
        let host = broker(Arc::new(CachedSet(Arc::clone(&fixture.indexes))), None)?;
        let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())?;
        let mut trial = Trial::new(&fixture, &host, DiscoveryMode::Inline)?;
        let result = trial.discover(scoped_query()).await?;
        let candidate = &result["candidates"][0];
        let inline = &candidate["inlineDescription"];
        assert!(inline["loadedReference"].is_string());
        assert_ne!(inline["loadedReference"], candidate["candidateReference"]);
        assert_eq!(inline["operations"].as_array().unwrap().len(), 2);
        assert!(candidate["operations"].as_array().unwrap().len() > 2);
        assert!(!result.to_string().contains("Fingerprint"));
        assert!(bytes(&result) <= 8192);
        assert_eq!(trial.session.amplification().discover_calls, 1);
        assert_eq!(trial.session.amplification().describe_calls, 0);

        let reference = principal().reference();
        let ordinary = host
            .dispatch_discover_with_catalog(
                &reference,
                &trial.run,
                &session,
                &scoped_query(),
                &fixture.catalog,
                8192,
            )
            .await?;
        let mut inline_candidate = candidate.as_object().unwrap().clone();
        inline_candidate.remove("inlineDescription");
        inline_candidate.remove("candidateReference");
        let mut ordinary_candidate = ordinary["candidates"][0].as_object().unwrap().clone();
        ordinary_candidate.remove("candidateReference");
        assert_eq!(ordinary_candidate, inline_candidate);

        for (shape, question) in [
            ("list", "In target0000, list IDs for Zqxwide (one result)?"),
            (
                "keyset_list",
                "In target0000, page through IDs for Zqxwide (one result)?",
            ),
        ] {
            let operation = select_operation(&Intent::parse(question)?, inline)?;
            assert_eq!(operation["shape"], shape);
            assert_eq!(
                operation["idRead"],
                json!({"selections": ["id"], "maximumItems": 1})
            );
            assert_eq!(operation["continuation"] == true, shape == "keyset_list");
            let mut execute = operation["idRead"].clone();
            execute["operation"] = operation["operation"].clone();
            execute["loadedReference"] = inline["loadedReference"].clone();
            assert!(trial.execute(&execute).await.is_ok());

            let mut substituted = execute.clone();
            substituted["loadedReference"] = candidate["candidateReference"].clone();
            assert!(matches!(
                host.authorize_broker_execution(&reference, &trial.run, &trial.session, &substituted).await,
                Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE
            ));
            assert!(matches!(
                host.authorize_broker_execution(
                    &reference,
                    &run_binding(),
                    &trial.session,
                    &execute
                )
                .await,
                Err(AiError::Forbidden)
            ));
            execute.as_object_mut().unwrap().remove("loadedReference");
            execute["candidateReference"] = candidate["candidateReference"].clone();
            assert!(
                host.authorize_broker_execution(&reference, &trial.run, &trial.session, &execute)
                    .await
                    .is_err()
            );
        }
        let aggregate = candidate["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|operation| operation["shape"] == "aggregate")
            .unwrap();
        let omitted = json!({
            "loadedReference": inline["loadedReference"], "operation": aggregate["operation"],
        });
        assert!(matches!(
            host.authorize_broker_execution(&reference, &trial.run, &trial.session, &omitted).await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_OPERATION_INVALID
        ));
        Ok(())
    }

    #[tokio::test]
    async fn scoped_discovery_falls_back_at_every_budget_or_scope_boundary() -> BenchResult<()> {
        let fixture = Fixture::compile(400).await?;
        let host = broker(Arc::new(CachedSet(Arc::clone(&fixture.indexes))), None)?;
        let reference = principal().reference();
        let run = run_binding();
        for (query, describe_bytes, output_bytes) in [
            (scoped_query(), 1024, 8192),
            (scoped_query(), 512 * 1024, 1024),
            (
                json!({"text": "list IDs for Zqxwide", "namespace": "target0000", "kind": "generated_entity", "maximumResults": 8}),
                512 * 1024,
                8192,
            ),
            (
                json!({"text": "list IDs for Zqxwide", "namespace": "target0000", "entityOrClass": "Zqxwide", "kind": "generated_entity", "maximumResults": 1}),
                512 * 1024,
                8192,
            ),
        ] {
            let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits {
                maximum_describe_bytes: describe_bytes,
                ..AiCapabilityDeliveryLimits::default()
            })?
            .with_scoped_discovery_descriptions();
            let result = host
                .dispatch_discover_with_catalog(
                    &reference,
                    &run,
                    &session,
                    &query,
                    &fixture.catalog,
                    output_bytes,
                )
                .await?;
            let candidate = &result["candidates"][0];
            assert!(candidate["candidateReference"].is_string());
            assert!(candidate.get("inlineDescription").is_none());
            assert_eq!(session.amplification().describe_calls, 0);
            let fallback = host
                .dispatch_describe(&reference, &run, &session, &describe_args(candidate))
                .await?;
            assert_ne!(fallback.loaded_reference(), candidate["candidateReference"]);
        }
        let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits {
            maximum_loaded_bindings: 1,
            maximum_deferred_definitions: 1,
            ..AiCapabilityDeliveryLimits::default()
        })?
        .with_scoped_discovery_descriptions();
        let attached = host
            .dispatch_discover_with_catalog(
                &reference,
                &run,
                &session,
                &scoped_query(),
                &fixture.catalog,
                8192,
            )
            .await?;
        let inline = &attached["candidates"][0]["inlineDescription"];
        let operation = select_operation(
            &Intent::parse("In target0000, list IDs for Zqxwide (one result)?")?,
            inline,
        )?;
        let mut execute = operation["idRead"].clone();
        execute["operation"] = operation["operation"].clone();
        execute["loadedReference"] = inline["loadedReference"].clone();
        for (catalog, maximum_bytes) in [(&fixture.catalog, 1024), (&AiToolCatalog::new(), 8192)] {
            let fallback = host
                .dispatch_discover_with_catalog(
                    &reference,
                    &run,
                    &session,
                    &scoped_query(),
                    catalog,
                    maximum_bytes,
                )
                .await?;
            assert!(fallback["candidates"][0].get("inlineDescription").is_none());
            // A dropped description never evicts an existing held binding.
            host.authorize_broker_execution(&reference, &run, &session, &execute)
                .await?;
        }
        Ok(())
    }

    struct CountReads(AtomicU8);

    #[async_trait::async_trait]
    impl AiCapabilityAuthorityPolicy for CountReads {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
            _index: &AiCapabilityIndex,
            entry: &AiCapabilityIndexEntry,
            _run: &AiCapabilityRunBinding,
        ) -> Result<AiCapabilityAuthorityDecision, AiError> {
            Ok(AiCapabilityAuthorityDecision {
                allowed: entry.operation_shape != AiCapabilityOperationShape::Aggregate
                    || self.0.load(Ordering::Relaxed) == 0,
                policy_fingerprint: "count-reads-v1".to_owned(),
            })
        }
    }

    #[tokio::test]
    async fn count_example_is_bounded_and_requires_current_policy() -> BenchResult<()> {
        let fixture = Fixture::compile(400).await?;
        let policy = Arc::new(CountReads(AtomicU8::new(0)));
        let host = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal())),
            Arc::new(CachedSet(Arc::clone(&fixture.indexes))),
            policy.clone(),
            Arc::new(SystemClock),
            Duration::minutes(5),
        )?;
        let mut trial = Trial::new(&fixture, &host, DiscoveryMode::Inline)?;
        let query = scoped_query();
        let result = trial.discover(query.clone()).await?;
        let candidate = &result["candidates"][0];
        assert!(
            !candidate["inlineDescription"]
                .to_string()
                .contains("ungroupedCount")
        );
        let description = host
            .dispatch_describe(
                &principal().reference(),
                &trial.run,
                &trial.session,
                &describe_args(candidate),
            )
            .await?;
        let limits = AiCapabilityDeliveryLimits::default();
        let rendered = production_describe(&fixture.catalog, description.clone(), limits)?;
        let operation = rendered["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|operation| operation["shape"] == "aggregate")
            .unwrap();
        let mut execute = operation["ungroupedCount"].clone();
        assert!(execute.is_object());
        execute["operation"] = operation["operation"].clone();
        execute["loadedReference"] = rendered["loadedReference"].clone();
        let expected = serde_json::from_str::<Vec<Question>>(CASES)?;
        assert_eq!(
            trial.execute(&execute).await?.result,
            expected[2].expected[0].result
        );
        assert_eq!(trial.session.amplification().describe_type_calls, 0);

        let summaries = description
            .operations()
            .iter()
            .map(|operation| {
                production_signature(&fixture.catalog, operation)
                    .map(|signature| signature.summary())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let plain =
            description.with_operation_signatures(summaries, limits.maximum_describe_bytes)?;
        let plain_bytes = serde_json::to_vec(plain.contract())?.len();
        for (describe_bytes, model_bytes) in [
            (plain_bytes as u32, 64 * 1024),
            (limits.maximum_describe_bytes, plain_bytes),
        ] {
            let mut bounded = plain.clone();
            assert!(!bounded.add_ungrouped_count_example(
                &fixture.catalog,
                describe_bytes,
                model_bytes
            ));
            assert_eq!(bounded.contract(), plain.contract());
        }
        let mut missing_catalog = plain.clone();
        assert!(!missing_catalog.add_ungrouped_count_example(
            &AiToolCatalog::new(),
            limits.maximum_describe_bytes,
            64 * 1024,
        ));
        assert_eq!(missing_catalog.contract(), plain.contract());

        let plain_aggregate = plain.contract()["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|operation| operation["shape"] == "aggregate")
            .unwrap();
        let mut fallback = execute.clone();
        fallback["arguments"] = trial
            .count_arguments(plain.contract(), plain_aggregate)
            .await?;
        assert_eq!(
            trial.execute(&fallback).await?.result,
            expected[2].expected[0].result
        );
        assert_eq!(trial.session.amplification().describe_type_calls, 2);

        policy.0.store(1, Ordering::Relaxed);
        let error = trial
            .execute(&execute)
            .await
            .err()
            .ok_or("revoked COUNT executed")?;
        assert!(matches!(
            error.downcast_ref::<AiError>(),
            Some(AiError::Forbidden)
        ));
        let result = trial.discover(query).await?;
        let restricted = trial.describe(&result["candidates"][0]).await?;
        assert!(
            restricted["operations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|operation| {
                    operation["shape"] != "aggregate" && operation.get("ungroupedCount").is_none()
                })
        );
        Ok(())
    }

    struct ScopedReads(AtomicU8);

    #[async_trait::async_trait]
    impl AiCapabilityAuthorityPolicy for ScopedReads {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
            index: &AiCapabilityIndex,
            entry: &AiCapabilityIndexEntry,
            _run: &AiCapabilityRunBinding,
        ) -> Result<AiCapabilityAuthorityDecision, AiError> {
            let state = self.0.load(Ordering::Relaxed);
            let shape_allowed = match entry.operation_shape {
                AiCapabilityOperationShape::Entity | AiCapabilityOperationShape::List => true,
                AiCapabilityOperationShape::KeysetList => state == 0,
                _ => false,
            };
            Ok(AiCapabilityAuthorityDecision {
                allowed: index.target_id().as_str() == "target-0000" && state < 2 && shape_allowed,
                policy_fingerprint: "scoped-reads-v1".to_owned(),
            })
        }
    }

    #[tokio::test]
    async fn inline_references_follow_current_policy_and_hide_denied_operations() -> BenchResult<()>
    {
        let fixture = Fixture::compile(400).await?;
        let policy = Arc::new(ScopedReads(AtomicU8::new(0)));
        let host = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal())),
            Arc::new(CachedSet(Arc::clone(&fixture.indexes))),
            policy.clone(),
            Arc::new(SystemClock),
            Duration::minutes(5),
        )?;
        let mut trial = Trial::new(&fixture, &host, DiscoveryMode::Inline)?;
        let mut denied_target = scoped_query();
        denied_target["namespace"] = json!("target0001");
        assert_eq!(
            trial.discover(denied_target).await?["candidates"],
            json!([])
        );

        let result = trial.discover(scoped_query()).await?;
        let candidate = &result["candidates"][0];
        let inline = &candidate["inlineDescription"];
        let metadata = candidate["operations"].as_array().unwrap();
        assert_eq!(
            metadata
                .iter()
                .filter(|root| root["shape"] == "keyset_list")
                .count(),
            2
        );
        assert_eq!(inline["operations"].as_array().unwrap().len(), 2);
        assert!(
            metadata
                .iter()
                .all(|root| root["shape"] == "list" || root["shape"] == "keyset_list")
        );
        let operation = select_operation(
            &Intent::parse("In target0000, list IDs for Zqxwide (one result)?")?,
            inline,
        )?;
        let mut execute = operation["idRead"].clone();
        execute["operation"] = operation["operation"].clone();
        execute["loadedReference"] = inline["loadedReference"].clone();
        let reference = principal().reference();

        let mut hidden = execute.clone();
        hidden["operation"] = json!("zqxwidesAggregate");
        assert!(matches!(
            host.authorize_broker_execution(&reference, &trial.run, &trial.session, &hidden).await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_OPERATION_INVALID
        ));
        let pager = select_operation(
            &Intent::parse("In target0000, page through IDs for Zqxwide (one result)?")?,
            inline,
        )?;
        let mut revoked = execute.clone();
        revoked["operation"] = pager["operation"].clone();
        policy.0.store(1, Ordering::Relaxed);
        assert!(matches!(
            host.authorize_broker_execution(&reference, &trial.run, &trial.session, &revoked)
                .await,
            Err(AiError::Forbidden)
        ));
        assert!(matches!(
            host.authorize_broker_execution(&reference, &trial.run, &trial.session, &execute).await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_STALE
        ));
        let current = trial.discover(scoped_query()).await?;
        let inline = &current["candidates"][0]["inlineDescription"];
        assert_eq!(inline["operations"].as_array().unwrap().len(), 1);
        execute["loadedReference"] = inline["loadedReference"].clone();
        assert!(trial.execute(&execute).await.is_ok());

        policy.0.store(2, Ordering::Relaxed);
        assert!(matches!(
            host.authorize_broker_execution(&reference, &trial.run, &trial.session, &execute)
                .await,
            Err(AiError::Forbidden)
        ));
        assert_eq!(
            trial.discover(scoped_query()).await?["candidates"],
            json!([])
        );
        Ok(())
    }
}
