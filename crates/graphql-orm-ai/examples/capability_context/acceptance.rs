//! Deterministic question-to-execution driver for the context benchmark.

use super::{
    CachedSet, LARGE_INDEX_LIMITS, LARGE_SET_LIMITS, broker, compile_target, compile_target_sdl,
    elapsed_micros, principal, production_describe, run_binding, split_template, template,
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

struct Trial<'a> {
    fixture: &'a Fixture,
    broker: &'a AiCapabilityDiscoveryBroker,
    run: AiCapabilityRunBinding,
    session: AiCapabilityBrokerSession,
    calls: Vec<Value>,
    unscoped_discover_micros: Vec<u64>,
    first_execute: Option<usize>,
    selection: Option<Value>,
}

impl<'a> Trial<'a> {
    fn new(fixture: &'a Fixture, broker: &'a AiCapabilityDiscoveryBroker) -> BenchResult<Self> {
        Ok(Self {
            fixture,
            broker,
            run: run_binding(),
            session: AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())?,
            calls: Vec::new(),
            unscoped_discover_micros: Vec::new(),
            first_execute: None,
            selection: None,
        })
    }

    fn record<T>(&mut self, operation: &str, result: &BenchResult<T>) {
        self.calls.push(json!({
            "operation": operation,
            "error": result.as_ref().err().map(ToString::to_string),
        }));
    }

    async fn discover(&mut self, arguments: Value) -> BenchResult<Value> {
        let started = Instant::now();
        let result = self
            .broker
            .dispatch_discover(
                &principal().reference(),
                &self.run,
                &self.session,
                &arguments,
            )
            .await
            .map_err(Into::into);
        if arguments["namespace"].is_null() {
            self.unscoped_discover_micros.push(elapsed_micros(started));
        }
        self.calls.push(json!({
            "operation": "discover",
            "namespace": arguments["namespace"],
            "candidates": result.as_ref().ok().and_then(|result| result["candidates"].as_array()).map(Vec::len),
            "error": result.as_ref().err().map(ToString::to_string),
        }));
        result
    }

    async fn describe(&mut self, candidate: &Value) -> BenchResult<Value> {
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
        self.record("describe", &result);
        result
    }

    async fn execute(&mut self, arguments: &Value) -> BenchResult<ExecutionResult> {
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
        self.record("execute", &result);
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
        let result = self
            .discover(json!({
                "text": intent.text, "maximumResults": MAXIMUM_RESULTS,
            }))
            .await;
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
        let description = self.describe(&candidate).await?;
        let operation = description["operations"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|operation| operation["shape"] == intent.shape)
            .ok_or("requested shape missing from description")?;
        self.selection = Some(json!({
            "namespace": candidate["namespace"],
            "entity": candidate["entity"],
            "shape": operation["shape"],
        }));
        let mut args = json!({
            "loadedReference": description["loadedReference"],
            "operation": operation["operation"],
            "selections": intent.selections,
            "maximumItems": 1,
        });
        if intent.shape == "aggregate" {
            // An ungrouped COUNT omits the grouping argument on the broker wire.
            args["arguments"] = json!([
                {"name": "metrics.0.operator", "value": "COUNT"},
            ]);
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
}

fn nearest_rank(samples: &mut [u64], percent: usize) -> Option<u64> {
    samples.sort_unstable();
    samples
        .get((samples.len() * percent).div_ceil(100).saturating_sub(1))
        .copied()
}

pub(super) async fn measure(entries: usize) -> BenchResult<Value> {
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
        let mut trial = Trial::new(&fixture, &host)?;
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

    let mut control = Trial::new(&fixture, &host)?;
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
    let mut restricted = Trial::new(&fixture, &restricted_host)?;
    let permitted = restricted
        .discover(json!({
            "text": "record identity", "maximumResults": MAXIMUM_RESULTS,
        }))
        .await?;
    let candidates = permitted["candidates"]
        .as_array()
        .ok_or("missing candidate list")?;

    let p50 = nearest_rank(&mut call_samples, 50);
    let p95 = nearest_rank(&mut call_samples, 95);
    let shape_rate = correct_shapes as f64 / questions.len() as f64;
    let discover_p95 = nearest_rank(&mut discovery, 95);
    Ok(json!({
        "mode": "deterministic", "brokerVersion": AI_CAPABILITY_BROKER_VERSION,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "modelRuns": 0, "indexEntries": entries, "targets": fixture.indexes.indexes().len(),
        "startupMicroseconds": startup, "peakResidentKibibytes": super::peak_resident_kibibytes(),
        "questions": questions.len(), "correctShape": correct_shapes, "correctShapeRate": shape_rate,
        "taskSuccesses": successful_tasks,
        "firstSuccessfulExecute": {
            "samples": call_samples.len(), "withoutSuccess": questions.len() - call_samples.len(),
            "brokerCallsP50": p50, "brokerCallsP95": p95,
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
