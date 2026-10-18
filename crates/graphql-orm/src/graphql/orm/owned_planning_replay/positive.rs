use super::*;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use crate::db::Database;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "../../../../tests/support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn col(name: &str, pk: bool) -> ColumnModel {
    ColumnModel {
        name: name.into(),
        sql_type: "TEXT".into(),
        spatial: None,
        nullable: false,
        is_primary_key: pk,
        is_unique: false,
        default: None,
    }
}
#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn table(name: &str, keys: &[&str], cols: &[&str]) -> OwnedTableModel {
    OwnedTableModel {
        entity_name: name.into(),
        table_name: name.into(),
        primary_key: keys[0].into(),
        primary_keys: keys.iter().map(|x| (*x).into()).collect(),
        default_sort: keys[0].into(),
        columns: cols.iter().map(|x| col(x, keys.contains(x))).collect(),
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
        search_indexes: vec![],
        append_only: false,
        retention_purge: false,
        check_constraints: vec![],
    }
}
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn replay_positive_sqlite() -> Result<(), Box<dyn std::error::Error>> {
    let db =
        Database::<super::super::super::SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    run_sqlite(&db).await?;
    db.pool().close().await;
    assert!(db.pool().is_closed());
    replay_positive_offline_original_fixtures()?;
    Ok(())
}
#[cfg(feature = "sqlite")]
async fn run_sqlite(
    db: &Database<super::super::super::SqliteBackend>,
) -> Result<(), Box<dyn std::error::Error>> {
    for sql in [
        "CREATE TABLE replay_notes (id TEXT NOT NULL PRIMARY KEY, label TEXT NOT NULL)",
        "CREATE TABLE external_notes (payload TEXT DEFAULT 'private ''quoted'' 雪')",
        "CREATE INDEX external_payload ON external_notes(payload)",
    ] {
        sqlx::query(sql).execute(db.pool()).await?;
    }
    let own = ManagedTableSet::new(["replay_notes".into()])?;
    let base = OwnedSchemaModel {
        tables: vec![table("replay_notes", &["id"], &["id", "label"])],
        ..Default::default()
    };
    for (tag, options) in [
        ("strict", PlanOptions::strict()),
        ("managed", PlanOptions::managed_tables_only()),
    ] {
        let (p, w) = capture(
            db,
            format!("noop-{tag}"),
            "untouched unowned metadata",
            &base,
            &own,
            options,
        )
        .await?;
        check(&p, w)?;
        assert!(p.steps().is_empty());
        let mut target = base.clone();
        let mut c = col("extra", false);
        c.nullable = true;
        c.default = Some("'literal ''雪''\\path'".into());
        target.tables[0].columns.push(c);
        let (p, w) = capture(
            db,
            format!("add-{tag}"),
            "byte-sensitive '雪' \\ newline\n",
            &target,
            &own,
            options,
        )
        .await?;
        check(&p, w)?;
        assert!(!p.steps().is_empty());
    }
    let mut child = table("replay_child", &["id"], &["id", "a", "b"]);
    child.foreign_keys.push(ForeignKeyModel {
        constraint_name: None,
        column_pairs: vec![
            ForeignKeyColumnPairModel {
                source_column: "a".into(),
                target_column: "a".into(),
            },
            ForeignKeyColumnPairModel {
                source_column: "b".into(),
                target_column: "b".into(),
            },
        ],
        target_table: "replay_parent".into(),
        is_multiple: false,
        on_delete: DeletePolicy::Restrict,
    });
    let target = OwnedSchemaModel {
        tables: vec![child, table("replay_parent", &["a", "b"], &["a", "b"])],
        ..Default::default()
    };
    let own = ManagedTableSet::new(["replay_parent".into(), "replay_child".into()])?;
    for (tag, options) in [
        ("strict", PlanOptions::strict()),
        ("managed", PlanOptions::managed_tables_only()),
    ] {
        let (p, w) = capture(
            db,
            format!("compound-{tag}"),
            "compound One/Restrict",
            &target,
            &own,
            options,
        )
        .await?;
        check(&p, w)?;
        assert!(!p.steps().is_empty());
    }
    Ok(())
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[tokio::test]
async fn replay_positive_postgres() -> Result<(), Box<dyn std::error::Error>> {
    let mut owned = owned_postgres::OwnedPostgres::start("replay-positive")?;
    let db = Database::<super::super::super::PostgresBackend>::connect_postgres(&owned.url).await?;
    let identity: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(Some(identity.as_str()), owned.url.rsplit('/').next());
    run_postgres(&db).await?;
    let identity: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(Some(identity.as_str()), owned.url.rsplit('/').next());
    db.pool().close().await;
    assert!(db.pool().is_closed());
    owned.cleanup()?;
    replay_positive_offline_original_fixtures()?;
    Ok(())
}
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
async fn run_postgres(
    db: &Database<super::super::super::PostgresBackend>,
) -> Result<(), Box<dyn std::error::Error>> {
    for sql in [
        "CREATE TABLE replay_notes (id TEXT NOT NULL PRIMARY KEY, label TEXT NOT NULL)",
        "CREATE TABLE external_notes (payload TEXT DEFAULT 'private ''quoted'' 雪')",
        "CREATE INDEX external_payload ON external_notes(payload)",
    ] {
        sqlx::query(sql).execute(db.pool()).await?;
    }
    let own = ManagedTableSet::new(["replay_notes".into()])?;
    let base = OwnedSchemaModel {
        tables: vec![table("replay_notes", &["id"], &["id", "label"])],
        ..Default::default()
    };
    for (tag, options) in [
        ("strict", PlanOptions::strict()),
        ("managed", PlanOptions::managed_tables_only()),
    ] {
        let (p, w) = capture(
            db,
            format!("noop-{tag}"),
            "untouched unowned metadata",
            &base,
            &own,
            options,
        )
        .await?;
        check(&p, w)?;
        assert!(p.steps().is_empty());
        let mut target = base.clone();
        let mut c = col("extra", false);
        c.nullable = true;
        c.default = Some("'literal ''雪''\\path'".into());
        target.tables[0].columns.push(c);
        let (p, w) = capture(
            db,
            format!("add-{tag}"),
            "byte-sensitive '雪' \\ newline\n",
            &target,
            &own,
            options,
        )
        .await?;
        check(&p, w)?;
        assert!(!p.steps().is_empty());
    }
    let mut child = table("replay_child", &["id"], &["id", "a", "b"]);
    child.foreign_keys.push(ForeignKeyModel {
        constraint_name: None,
        column_pairs: vec![
            ForeignKeyColumnPairModel {
                source_column: "a".into(),
                target_column: "a".into(),
            },
            ForeignKeyColumnPairModel {
                source_column: "b".into(),
                target_column: "b".into(),
            },
        ],
        target_table: "replay_parent".into(),
        is_multiple: false,
        on_delete: DeletePolicy::Restrict,
    });
    let target = OwnedSchemaModel {
        tables: vec![child, table("replay_parent", &["a", "b"], &["a", "b"])],
        ..Default::default()
    };
    let own = ManagedTableSet::new(["replay_parent".into(), "replay_child".into()])?;
    for (tag, options) in [
        ("strict", PlanOptions::strict()),
        ("managed", PlanOptions::managed_tables_only()),
    ] {
        let (p, w) = capture(
            db,
            format!("compound-{tag}"),
            "compound One/Restrict",
            &target,
            &own,
            options,
        )
        .await?;
        check(&p, w)?;
        assert!(!p.steps().is_empty());
    }
    Ok(())
}

fn check(
    plan: &OwnedPlannedMigration,
    w: OwnedPlanningReplayWitness,
) -> Result<(), Box<dyn std::error::Error>> {
    let limits = OwnedPlanningReplayLimits::default();
    let bytes = w.to_json(limits)?;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/owned-planning-replay-8e1")
        .join(format!("{}-{}.json", plan.backend(), plan.version()));
    let expected = std::fs::read(path)?;
    assert_eq!(bytes, expected, "complete source-qualified 8e1 output");
    let decoded = OwnedPlanningReplayWitness::from_json(&bytes, limits)?;
    assert_eq!(decoded.to_json(limits)?, bytes);
    let facts = decoded.verify()?;
    assert_eq!(facts.plan_hash(), plan.plan_hash());
    assert_eq!(facts.source_schema_hash(), plan.source_schema_hash());
    assert_eq!(facts.target_schema_hash(), plan.target_schema_hash());
    assert_eq!(facts.statements(), plan.statements());
    assert_eq!(facts.steps().len(), plan.steps().len());
    assert_eq!(facts.format_version(), 1);
    assert_eq!(facts.recipe(), RECIPE);
    assert_eq!(facts.version(), plan.version());
    assert_eq!(facts.description(), plan.description());
    assert_eq!(facts.ownership(), plan.ownership());
    fn owned_send_sync<T: Send + Sync>() {}
    owned_send_sync::<OwnedPlanningReplayWitness>();
    owned_send_sync::<VerifiedOwnedPlanningReplay>();
    Ok(())
}
#[test]
fn replay_positive_offline_original_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/owned-planning-replay-8e1");
    for file in std::fs::read_dir(root)? {
        let path = file?.path();
        if path.extension().is_some_and(|x| x == "json") {
            let bytes = std::fs::read(path)?;
            let w = OwnedPlanningReplayWitness::from_json(
                &bytes,
                OwnedPlanningReplayLimits::default(),
            )?;
            w.verify()?;
            assert_eq!(bytes, w.to_json(OwnedPlanningReplayLimits::default())?);
            // Valid alternative object order, including nested compound FK
            // payloads buffered before the internally tagged discriminant.
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            let kind_last = serde_json::to_vec(&KindLast(&value))?;
            let reordered = OwnedPlanningReplayWitness::from_json(
                &kind_last,
                OwnedPlanningReplayLimits::default(),
            )?;
            reordered.verify()?;
            assert_eq!(
                bytes,
                reordered.to_json(OwnedPlanningReplayLimits::default())?
            );
        }
    }
    Ok(())
}

// Test-host serialization only: all objects/enum strings remain valid and the
// actual historical facts are unchanged. No malformed/refusal case is executed.
struct KindLast<'a>(&'a serde_json::Value);
impl Serialize for KindLast<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self.0 {
            serde_json::Value::Object(m) => {
                let mut out = s.serialize_map(Some(m.len()))?;
                for (k, v) in m.iter().filter(|(k, _)| k.as_str() != "kind") {
                    out.serialize_entry(k, &KindLast(v))?;
                }
                if let Some(kind) = m.get("kind") {
                    out.serialize_entry("kind", kind)?;
                }
                out.end()
            }
            serde_json::Value::Array(a) => {
                let mut out = s.serialize_seq(Some(a.len()))?;
                for v in a {
                    out.serialize_element(&KindLast(v))?;
                }
                out.end()
            }
            value => value.serialize(s),
        }
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn capture<B: RuntimeMigrationBackend>(
    db: &Database<B>,
    version: String,
    description: &str,
    target: &OwnedSchemaModel,
    ownership: &ManagedTableSet,
    options: PlanOptions,
) -> Result<(OwnedPlannedMigration, OwnedPlanningReplayWitness), Box<dyn std::error::Error>> {
    use std::sync::atomic::Ordering::SeqCst;
    REPLAY_OBSERVATIONS.store(0, SeqCst);
    let ordinary = db
        .schema()
        .plan_owned_migration(version.clone(), description, target, ownership, options)
        .await?;
    let count = REPLAY_OBSERVATIONS.swap(0, SeqCst);
    let pair = db
        .schema()
        .plan_owned_migration_with_replay_witness(
            version,
            description,
            target,
            ownership,
            options,
            OwnedPlanningReplayLimits::default(),
            OwnedPlanningReplayCaptureConsent::accept_sensitive_schema_metadata_and_private_custody(
            ),
        )
        .await?;
    assert_eq!(
        REPLAY_OBSERVATIONS.load(SeqCst),
        count,
        "unchanged original observations, including SQLite live validation"
    );
    let expected = Facts {
        source_schema_hash: ordinary.source_schema_hash,
        target_schema_hash: ordinary.target_schema_hash,
        plan_hash: ordinary.plan_hash,
        steps: ordinary.steps,
        statements: ordinary.statements,
    };
    assert!(exact_facts(&expected, &pair.1.data.expected));
    Ok(pair)
}
