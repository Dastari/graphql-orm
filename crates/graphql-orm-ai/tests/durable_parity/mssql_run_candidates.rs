//! Owned SQL Server evidence for run candidate selection.

use super::*;

const RUNS: usize = 4096;
const QUEUED: usize = 320;
const EXPIRED: usize = 64;
const CLAIM_WORKERS: usize = 3;
const RECOVERY_WORKERS: usize = 2;
const CLAIM_CALLS: usize = 96;
const RECOVERY_CALLS: usize = 32;

const DEADLOCK_COUNTER: &str = "SELECT cntr_value FROM sys.dm_os_performance_counters
    WHERE RTRIM(object_name) LIKE N'%:Locks' AND counter_name = N'Number of Deadlocks/sec'
    AND instance_name = N'_Total'";

async fn deadlocks(database: &graphql_orm::prelude::Database<AiOrmBackend>) -> i64 {
    database
        .pool()
        .fetch_rows(DEADLOCK_COUNTER, &[])
        .await
        .unwrap()[0]
        .try_get(0)
        .unwrap()
}

async fn run_candidate_plans(
    database: &graphql_orm::prelude::Database<AiOrmBackend>,
) -> Vec<(String, String)> {
    database
        .pool()
        .fetch_rows(
            "SELECT DISTINCT SUBSTRING(s.text, q.statement_start_offset / 2 + 1,
                (CASE q.statement_end_offset WHEN -1 THEN DATALENGTH(s.text)
                 ELSE q.statement_end_offset END - q.statement_start_offset) / 2 + 1),
                 p.query_plan
             FROM sys.dm_exec_query_stats AS q
             CROSS APPLY sys.dm_exec_sql_text(q.sql_handle) AS s
             CROSS APPLY sys.dm_exec_text_query_plan(q.plan_handle,
                q.statement_start_offset, q.statement_end_offset) AS p
             WHERE s.text LIKE N'%graphql_orm_ai_runs%'
                AND s.text LIKE N'%ORDER BY%recovery_examined_at%'
                AND s.text NOT LIKE N'%dm_exec_query_stats%'
                AND p.query_plan IS NOT NULL",
            &[],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.try_get(0).unwrap(), row.try_get(1).unwrap()))
        .collect()
}

#[derive(Default)]
struct Calls {
    successes: usize,
    empty_claims: usize,
    persistence_failures: usize,
    recovered: usize,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 5)]
#[ignore = "bounded SQL Server load measurement; run explicitly with --ignored --nocapture"]
async fn owned_sql_server_measures_concurrent_run_candidates() {
    for layout in ["legacy", "indexed"] {
        tokio::time::timeout(StdDuration::from_secs(240), measure(layout))
            .await
            .expect("owned candidate load exceeded its time budget");
    }
}

async fn measure(layout: &str) {
    let (server, database) = owned_database().await.unwrap();
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-candidate-load",
            "Candidate load",
            AiSchemaModule.entities(),
        )
        .await
        .unwrap();
    apply_parity_migration(&server, &database, &plan).await;
    if layout == "legacy" {
        restore_legacy_run_state(&server).await;
    }
    let now = OffsetDateTime::now_utc();
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(now));
    let epoch = OrmAiRuntimeRecoveryService::new(database.clone(), clock.clone())
        .readiness_proof()
        .await
        .unwrap()
        .epoch_id();
    let reference = serde_json::to_string(&principal(now).reference()).unwrap();
    let reference = reference.replace('\'', "''");
    let cutoff = now.unix_timestamp() - 3600;
    let active = QUEUED + EXPIRED;
    durable_database::execute_sql_server_batch(
        &server,
        &format!(
            "WITH digits AS (SELECT n FROM (VALUES (0),(1),(2),(3),(4),(5),(6),(7)) AS t(n)),
             numbers AS (SELECT ROW_NUMBER() OVER (ORDER BY a.n,b.n,c.n,d.n) AS n
                 FROM digits a CROSS JOIN digits b CROSS JOIN digits c CROSS JOIN digits d)
             INSERT dbo.graphql_orm_ai_runs (id, session_id, input_message_id, principal_reference,
                 state, lease_generation, retry_count, created_at, attempt_id, lease_owner,
                 recovery_epoch_id, lease_expires_at, lease_heartbeat_at)
             SELECT NEWID(), NEWID(), NEWID(), N'{reference}',
                 CASE WHEN n <= {QUEUED} THEN N'queued' WHEN n <= {active} THEN N'leased'
                      ELSE N'completed' END,
                 CASE WHEN n BETWEEN {QUEUED} + 1 AND {active} THEN 1 ELSE 0 END,
                 0, n,
                 CASE WHEN n BETWEEN {QUEUED} + 1 AND {active} THEN NEWID() END,
                 CASE WHEN n BETWEEN {QUEUED} + 1 AND {active} THEN N'seed' END,
                 CASE WHEN n BETWEEN {QUEUED} + 1 AND {active} THEN '{epoch}' END,
                 CASE WHEN n BETWEEN {QUEUED} + 1 AND {active} THEN {cutoff} END,
                 CASE WHEN n BETWEEN {QUEUED} + 1 AND {active} THEN {cutoff} - 60 END
             FROM numbers WHERE n <= {RUNS};
             INSERT dbo.graphql_orm_ai_run_attempts (id, run_id, lease_generation, worker_id, claimed_at)
             SELECT attempt_id, id, lease_generation, lease_owner, lease_heartbeat_at
             FROM dbo.graphql_orm_ai_runs WHERE state = N'leased';
             UPDATE STATISTICS dbo.graphql_orm_ai_runs WITH FULLSCAN;"
        ),
    )
    .await
    .unwrap();
    let runs = Arc::new(OrmAiRunService::new(
        database.clone(),
        clock,
        AiRunServiceLimits::new(Duration::minutes(1), Duration::minutes(5), 16, 2, 4).unwrap(),
    ));
    let before = deadlocks(&database).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(CLAIM_WORKERS + RECOVERY_WORKERS));
    let mut tasks = tokio::task::JoinSet::new();
    for worker in 0..CLAIM_WORKERS {
        let (runs, barrier) = (runs.clone(), barrier.clone());
        tasks.spawn(async move {
            let worker_id = format!("claim-worker-{worker}");
            let mut calls = Calls::default();
            barrier.wait().await;
            for _ in 0..CLAIM_CALLS {
                match runs.claim_next(&worker_id).await {
                    Ok(Some(_)) => calls.successes += 1,
                    Ok(None) => calls.empty_claims += 1,
                    Err(AiError::PersistenceFailed) => calls.persistence_failures += 1,
                    Err(error) => panic!("unexpected claim error: {error}"),
                }
            }
            (true, calls)
        });
    }
    for _ in 0..RECOVERY_WORKERS {
        let (runs, barrier) = (runs.clone(), barrier.clone());
        tasks.spawn(async move {
            let mut calls = Calls::default();
            barrier.wait().await;
            for _ in 0..RECOVERY_CALLS {
                match runs.recover_expired_leases().await {
                    Ok(report) => {
                        calls.successes += 1;
                        calls.recovered += report.requeued as usize;
                    }
                    Err(AiError::PersistenceFailed) => calls.persistence_failures += 1,
                    Err(error) => panic!("unexpected recovery error: {error}"),
                }
            }
            (false, calls)
        });
    }
    let mut claim = Calls::default();
    let mut recovery = Calls::default();
    while let Some(result) = tasks.join_next().await {
        let (is_claim, calls) = result.unwrap();
        let total = if is_claim { &mut claim } else { &mut recovery };
        total.successes += calls.successes;
        total.empty_claims += calls.empty_claims;
        total.persistence_failures += calls.persistence_failures;
        total.recovered += calls.recovered;
    }
    let victim_count = deadlocks(&database).await - before;
    println!(
        "layout={layout}; 1205 victims: {victim_count}; claims leased/empty/PersistenceFailed: {}/{}/{};          recovery calls successful/PersistenceFailed: {}/{}; leases requeued: {}",
        claim.successes,
        claim.empty_claims,
        claim.persistence_failures,
        recovery.successes,
        recovery.persistence_failures,
        recovery.recovered
    );
    assert!(claim.successes > 0);
    assert!(recovery.recovered > 0);

    let plans = run_candidate_plans(&database).await;
    assert!(!plans.is_empty(), "no candidate plans captured");
    for (sql, plan) in plans {
        let eligibility = [
            "[lease_expires_at] IS NULL",
            "[lease_expires_at] <=",
            "[next_attempt_at] IS NULL",
            "[next_attempt_at] <=",
        ]
        .into_iter()
        .find(|condition| sql.contains(condition))
        .expect("candidate eligibility condition");
        let indexes = plan
            .split("Index=\"")
            .skip(1)
            .filter_map(|index| index.split_once('\"').map(|(index, _)| index))
            .collect::<std::collections::BTreeSet<_>>();
        let state_seek = plan.split("<SeekPredicates>").skip(1).any(|seek| {
            seek.split_once("</SeekPredicates>")
                .is_some_and(|(seek, _)| seek.contains("Column=\"state\""))
        });
        println!(
            "{layout}: {eligibility}; state seek={state_seek}; scan={}; indexes={indexes:?}",
            plan.contains("PhysicalOp=\"Clustered Index Scan\"")
                || plan.contains("PhysicalOp=\"Index Scan\""),
        );
    }
}

#[tokio::test]
async fn owned_sql_server_upgrades_existing_run_state_without_truncation() {
    use graphql_orm::graphql::orm::MigrationStep;

    let (server, database) = owned_database().await.unwrap();
    let plan = database
        .schema()
        .plan_migration_to_entities("ai-runs-fresh", "AI schema", AiSchemaModule.entities())
        .await
        .unwrap();
    apply_parity_migration(&server, &database, &plan).await;
    restore_legacy_run_state(&server).await;

    durable_database::execute_sql_server_batch(
        &server,
        "INSERT dbo.graphql_orm_ai_runs (id, session_id, input_message_id, principal_reference,
            state, lease_generation, retry_count, created_at)
         SELECT NEWID(), NEWID(), NEWID(), N'{}', state, 0, 0, created_at
         FROM (VALUES (N'queued', 1), (N'QUEUED ', 2),
                      (N'queued' + REPLICATE(N' ', 445), 3)) AS seeded(state, created_at);",
    )
    .await
    .unwrap();
    let upgrade = database
        .schema()
        .plan_migration_to_entities(
            "ai-runs-upgraded",
            "Run candidate index",
            AiSchemaModule.entities(),
        )
        .await
        .unwrap();
    assert!(upgrade.steps.iter().any(|step| matches!(
        &step.step, MigrationStep::AlterColumn { before, after, .. }
            if after.name == "state" && before.sql_type.eq_ignore_ascii_case("NVARCHAR(MAX)")
                && after.sql_type == "NVARCHAR(450) COLLATE Latin1_General_100_BIN2"
    )));
    assert!(upgrade.steps.iter().any(|step| matches!(
        &step.step, MigrationStep::CreateIndex { index, .. }
            if index.name == "idx_graphql_orm_ai_runs_recovery_candidates"
    )));

    // Exercise the host's documented external upgrade in one transaction.
    let (_, migration) = include_str!("../../MIGRATION.md")
        .split_once("The check counts UTF-16 bytes")
        .unwrap();
    let (_, preflight) = migration.split_once("```sql\n").unwrap();
    let (preflight, _) = preflight.split_once("```").unwrap();
    let batch = format!(
        "SET XACT_ABORT ON; BEGIN TRANSACTION; {preflight}\n{}; COMMIT TRANSACTION;",
        upgrade.statements.join(";\n")
    );
    let error = durable_database::execute_sql_server_batch(&server, &batch)
        .await
        .expect_err("oversized legacy state must refuse the upgrade");
    assert!(
        error
            .to_string()
            .contains("a stored value exceeds 900 bytes")
    );
    let bytes: i64 = database
        .pool()
        .fetch_rows(
            "SELECT CONVERT(bigint, DATALENGTH(state)) FROM dbo.graphql_orm_ai_runs WHERE created_at = 3",
            &[],
        )
        .await
        .unwrap()[0]
        .try_get(0)
        .unwrap();
    assert_eq!(bytes, 902);
    durable_database::execute_sql_server_batch(
        &server,
        "DELETE FROM dbo.graphql_orm_ai_runs WHERE created_at = 3;",
    )
    .await
    .unwrap();
    durable_database::execute_sql_server_batch(&server, &batch)
        .await
        .expect("a clean store should upgrade");
    let states: Vec<String> = database
        .pool()
        .fetch_rows(
            "SELECT state FROM dbo.graphql_orm_ai_runs ORDER BY created_at",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.try_get(0).unwrap())
        .collect();
    assert_eq!(states, ["queued", "QUEUED "]);
    let replan = database
        .schema()
        .plan_migration_to_entities(
            "ai-runs-replan",
            "Run candidate index",
            AiSchemaModule.entities(),
        )
        .await
        .unwrap();
    assert!(replan.statements.is_empty(), "{:?}", replan.statements);
}

async fn restore_legacy_run_state(server: &durable_database::OwnedServer) {
    durable_database::execute_sql_server_batch(
        server,
        "DROP INDEX IF EXISTS [idx_graphql_orm_ai_runs_claim_candidates] ON dbo.graphql_orm_ai_runs;
         DROP INDEX IF EXISTS [idx_graphql_orm_ai_runs_recovery_candidates] ON dbo.graphql_orm_ai_runs;
         DROP INDEX IF EXISTS [idx_graphql_orm_ai_runs_state] ON dbo.graphql_orm_ai_runs;
         ALTER TABLE dbo.graphql_orm_ai_runs ALTER COLUMN [state] NVARCHAR(MAX) COLLATE DATABASE_DEFAULT NOT NULL;",
    )
    .await
    .unwrap();
}
