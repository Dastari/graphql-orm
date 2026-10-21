//! Byte-level compatibility goldens for runtime schemas without format
//! descriptors that use only defaults supported before typed literals.
//!
//! The goldens in `tests/goldens/` were captured by running this exact file
//! against graphql-orm 0.50.2 (`8e1e5698437d9ea5c0d6ff2c0f29cf7af58d45a0`),
//! whose owned planner is the pinned `8e1` comparator. The current owned
//! planner selects the typed-literal comparator, so equal reports prove that
//! it yields byte-identical serialized schemas, canonical bytes, fingerprints,
//! physical hashes, plans, plan hashes, live replans after apply, certified
//! baselines and SDL for these schemas.
//!
//! Fixtures are JSON so this file compiles unchanged against both releases.
//! Set `GRAPHQL_ORM_CAPTURE_GOLDENS=1` only when deliberately recapturing at
//! the pinned historical commit.
#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::db::Database;
use graphql_orm::graphql::orm::*;
use std::fmt::Write as _;
use std::sync::Arc;

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

fn fixtures() -> Vec<(&'static str, serde_json::Value)> {
    let field = |id: &str, api: &str, kind: &str, nullable: bool, default: serde_json::Value| {
        serde_json::json!({
            "id": id, "api_name": api, "physical_column": api, "value_kind": kind,
            "nullable": nullable, "unique": false, "filterable": true, "sortable": true,
            "generated": false, "default": default
        })
    };
    let mut generated_id = field(
        "gold_items.id",
        "id",
        "uuid",
        false,
        serde_json::Value::Null,
    );
    generated_id["generated"] = serde_json::json!(true);
    let defaults = serde_json::json!({
        "collections": [{
            "id": "gold_items",
            "api_type_name": "GoldItem",
            "api_plural_name": "GoldItems",
            "physical_table": "gold_items",
            "primary_key": ["gold_items.id"],
            "append_only": false,
            "fields": [
                generated_id,
                field("gold_items.label", "label", "string", false, serde_json::json!({"literal": "plain"})),
                field("gold_items.quoted", "quoted", "string", false, serde_json::json!({"literal": "O'Reilly 雪🦀"})),
                field("gold_items.empty", "empty", "string", false, serde_json::json!({"literal": ""})),
                field("gold_items.flag", "flag", "boolean", false, serde_json::json!({"literal": "true"})),
                field("gold_items.off", "off", "boolean", true, serde_json::json!({"literal": "false"})),
                field("gold_items.count", "count", "integer", false, serde_json::json!({"literal": "7"})),
                field("gold_items.ratio", "ratio", "float", false, serde_json::json!({"literal": "1.5"})),
                field("gold_items.created", "created", "date_time", false, serde_json::json!("current_timestamp")),
                field("gold_items.epoch", "epoch", "integer", false, serde_json::json!("current_timestamp")),
                field("gold_items.note", "note", "string", true, serde_json::Value::Null),
                field("gold_items.payload", "payload", "json", true, serde_json::Value::Null),
                field("gold_items.blob", "blob", "bytes", true, serde_json::Value::Null),
                field("gold_items.tag", "tag", "uuid", true, serde_json::Value::Null)
            ],
            "relations": [{
                "id": "gold_items.children", "api_name": "children", "target": "gold_children",
                "key_pairs": [{"source": "gold_items.id", "target": "gold_children.parent"}],
                "cardinality": "many", "enforce_foreign_key": false, "on_delete": null
            }],
            "indexes": [{
                "id": "gold_items_label", "name": "idx_gold_items_label",
                "fields": ["gold_items.label"], "unique": false
            }],
            "composite_unique": [["gold_items.label", "gold_items.count"]],
            "default_order": [{"field": "gold_items.created", "direction": "desc"}]
        }, {
            "id": "gold_children",
            "api_type_name": "GoldChild",
            "api_plural_name": "GoldChildren",
            "physical_table": "gold_children",
            "primary_key": ["gold_children.id"],
            "append_only": true,
            "retention_purge": true,
            "fields": [
                field("gold_children.id", "id", "integer", false, serde_json::Value::Null),
                field("gold_children.parent", "parent", "uuid", false, serde_json::Value::Null),
                field("gold_children.weight", "weight", "float", true, serde_json::json!({"literal": "2"}))
            ],
            "relations": [{
                "id": "gold_children.parent_item", "api_name": "parentItem", "target": "gold_items",
                "key_pairs": [{"source": "gold_children.parent", "target": "gold_items.id"}],
                "cardinality": "one", "enforce_foreign_key": true, "on_delete": "Restrict"
            }],
            "indexes": [],
            "composite_unique": [],
            "default_order": []
        }]
    });
    let minimal = serde_json::json!({
        "format_version": 1,
        "collections": [{
            "id": "gold_minimal",
            "api_type_name": "GoldMinimal",
            "api_plural_name": "GoldMinimals",
            "physical_table": "gold_minimal",
            "primary_key": ["gold_minimal.key"],
            "append_only": false,
            "retention_purge": false,
            "fields": [
                field("gold_minimal.key", "key", "string", false, serde_json::Value::Null),
                field("gold_minimal.score", "score", "float", false, serde_json::json!({"literal": "0.25"}))
            ],
            "relations": [],
            "indexes": [],
            "composite_unique": [],
            "default_order": []
        }]
    });
    vec![("defaults", defaults), ("minimal", minimal)]
}

fn ownership(schema: &ValidatedRuntimeSchema) -> ManagedTableSet {
    ManagedTableSet::new(
        schema
            .schema()
            .collections
            .iter()
            .map(|c| c.physical_table.clone()),
    )
    .unwrap()
}

async fn report<B>(db: &Database<B>, lane: &str) -> Result<String, Box<dyn std::error::Error>>
where
    B: RuntimeMigrationBackend + RuntimeMutationBackend + RuntimeRowDecoder,
{
    let mut out = String::new();
    for (name, json) in fixtures() {
        let schema: RuntimeSchema = serde_json::from_value(json)?;
        let schema = schema.validate()?;
        writeln!(out, "== fixture {name} ({lane})")?;
        writeln!(
            out,
            "serialized={}",
            serde_json::to_string(schema.schema())?
        )?;
        writeln!(
            out,
            "canonical={}",
            serde_json::to_string(&String::from_utf8(schema.canonical_bytes())?)?
        )?;
        writeln!(out, "fingerprint={}", schema.fingerprint())?;
        writeln!(out, "structural={}", schema.structural_fingerprint())?;
        let target = schema.physical_schema::<B>(RuntimeMigrationLimits::default())?;
        writeln!(out, "physical={}", target.stable_hash())?;
        let ownership = ownership(&schema);
        let manager = db.schema();
        for (step, description) in [("v1", "create"), ("v2", "replan")] {
            let plan = manager
                .plan_owned_migration(
                    format!("{name}-{step}"),
                    description,
                    &target,
                    &ownership,
                    PlanOptions::strict(),
                )
                .await?;
            writeln!(out, "plan.{step}.hash={}", plan.plan_hash())?;
            writeln!(out, "plan.{step}.source={}", plan.source_schema_hash())?;
            writeln!(out, "plan.{step}.target={}", plan.target_schema_hash())?;
            writeln!(out, "plan.{step}.steps={}", plan.steps().len())?;
            writeln!(
                out,
                "plan.{step}.statements={}",
                serde_json::to_string(plan.statements())?
            )?;
            manager
                .apply_owned_migration(&plan, Default::default())
                .await?;
        }
        let environment = manager
            .runtime_mutation_environment(Arc::new(schema.clone()), &target, &ownership)
            .await?;
        writeln!(out, "baseline={}", environment.physical_baseline_hash())?;
        #[cfg(feature = "runtime-graphql")]
        {
            use graphql_orm::graphql::runtime::*;
            let module = RuntimeGraphqlModule::compile(
                Arc::new(schema.clone()),
                RuntimeGraphqlOptions::default()
                    .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                    .with_mutations(Default::default()),
            )?;
            let sdl = RuntimeGraphqlComposer::new(db.clone(), "Query", Default::default())?
                .mutation_root("Mutation")?
                .install(module)?
                .finish()?
                .sdl();
            writeln!(out, "sdl={}", serde_json::to_string(&sdl)?)?;
        }
    }
    Ok(out)
}

/// SDL with block descriptions and all whitespace removed, so only the
/// described shape is compared.
fn without_descriptions(sdl: &str) -> String {
    let mut out = String::new();
    let mut parts = sdl.split("\"\"\"");
    while let Some(text) = parts.next() {
        out.extend(text.chars().filter(|c| !c.is_whitespace()));
        parts.next();
    }
    out
}

fn compare(report: &str, golden: &str) {
    let path = format!("{}/tests/goldens/{golden}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var_os("GRAPHQL_ORM_CAPTURE_GOLDENS").is_some() {
        std::fs::create_dir_all(format!("{}/tests/goldens", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::write(&path, report).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    // Goldens were captured with `runtime-graphql`; without that feature the
    // report has no SDL lines, and every other line must still match exactly.
    let expected = expected
        .lines()
        .filter(|line| cfg!(feature = "runtime-graphql") || !line.starts_with("sdl="))
        .collect::<Vec<_>>();
    let actual = report.lines().collect::<Vec<_>>();
    for (line, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        // 0.56 describes every runtime type, field and root. The 0.50.2 SDL
        // must be unchanged apart from those descriptions.
        if let (Some(actual), Some(expected)) =
            (actual.strip_prefix("sdl="), expected.strip_prefix("sdl="))
        {
            let actual: String = serde_json::from_str(actual).unwrap();
            let expected: String = serde_json::from_str(expected).unwrap();
            assert!(actual.contains("\"\"\""), "runtime SDL is described");
            assert_eq!(
                without_descriptions(&actual),
                without_descriptions(&expected),
                "golden {golden} line {} differs beyond descriptions",
                line + 1
            );
            continue;
        }
        assert_eq!(actual, expected, "golden {golden} line {}", line + 1);
    }
    assert_eq!(actual.len(), expected.len(), "golden {golden}");
    if cfg!(feature = "runtime-graphql") {
        assert!(actual.iter().any(|line| line.starts_with("sdl=")));
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_reports_match_0_50_2_goldens() -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    compare(
        &report(&db, "sqlite").await?,
        "runtime_compat_0_50_2_sqlite.txt",
    );
    Ok(())
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[tokio::test]
async fn postgres_reports_match_0_50_2_goldens() -> Result<(), Box<dyn std::error::Error>> {
    let mut owned = owned_postgres::OwnedPostgres::start("runtime-compat-goldens")?;
    let db = Database::<PostgresBackend>::connect_postgres(&owned.url).await?;
    // Prove the pool reached the test-owned database before any DDL.
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(db.pool())
        .await?;
    assert!(
        owned.url.ends_with(&format!("/{database}")),
        "unexpected database"
    );
    let report = report(&db, "postgres").await?;
    db.pool().close().await;
    owned.cleanup()?;
    compare(&report, "runtime_compat_0_50_2_postgres.txt");
    Ok(())
}
