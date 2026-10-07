//! Runtime mutation conformance: each PostgreSQL case owns a verified disposable server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use futures::future::BoxFuture;
use graphql_orm::{graphql::errors::OrmPublicError, graphql::orm::*, prelude::*};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "runtime_journal", plural = "RuntimeJournal")]
struct Journal {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    message: String,
}
fn definition() -> RuntimeSchema {
    let mut fields = Vec::new();
    for (name, kind, nullable, default) in [
        ("id", RuntimeValueKind::Integer, false, None),
        ("tenant", RuntimeValueKind::String, false, None),
        ("label", RuntimeValueKind::String, false, None),
        (
            "score",
            RuntimeValueKind::Integer,
            false,
            Some(RuntimeDefault::Literal("7".into())),
        ),
        ("optional", RuntimeValueKind::String, true, None),
        (
            "occurred",
            RuntimeValueKind::DateTime,
            false,
            Some(RuntimeDefault::CurrentTimestamp),
        ),
    ] {
        fields.push(RuntimeField {
            id: FieldId::new(name).unwrap(),
            api_name: name.into(),
            physical_column: name.into(),
            value_kind: kind,
            nullable,
            unique: false,
            filterable: true,
            sortable: true,
            generated: false,
            default,
        });
    }
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            id: CollectionId::new("records").unwrap(),
            api_type_name: "Record".into(),
            api_plural_name: "Records".into(),
            physical_table: "runtime_records".into(),
            primary_key: vec![FieldId::new("id").unwrap()],
            append_only: false,
            retention_purge: false,
            fields,
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
        }],
    }
}
struct Fixture {
    db: Database<Backend>,
    environment: Arc<RuntimeMutationEnvironment>,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_definition(definition()).await
    }
    async fn with_definition(definition: RuntimeSchema) -> Self {
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite_with_options(
            "sqlite::memory:",
            graphql_orm::db::ConnectionOptions::default().max_connections(1),
        )
        .await
        .unwrap();
        #[cfg(feature = "postgres")]
        let owned = owned_postgres::OwnedPostgres::start("runtime-mutations").unwrap();
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres(&owned.url)
            .await
            .unwrap();
        let schema = Arc::new(definition.validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap()
            .with_static_entities(&[Journal::metadata()])
            .unwrap();
        let ownership =
            ManagedTableSet::new(["runtime_records".into(), "runtime_journal".into()]).unwrap();
        let plan = db
            .schema()
            .plan_owned_migration(
                "init",
                "runtime test",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap();
        db.schema()
            .apply_owned_migration(&plan, ApplyOptions::default())
            .await
            .unwrap();
        let environment = Arc::new(
            db.schema()
                .runtime_mutation_environment(schema, &target, &ownership)
                .await
                .unwrap(),
        );
        Self {
            db,
            environment,
            #[cfg(feature = "postgres")]
            owned,
        }
    }
    fn schema(&self) -> &ValidatedRuntimeSchema {
        self.environment.schema()
    }
    fn collection(&self) -> RuntimeCollectionHandle {
        self.schema()
            .resolve_collection(&CollectionId::new("records").unwrap())
            .unwrap()
    }
    fn field(&self, name: &str) -> RuntimeFieldHandle {
        self.schema()
            .resolve_field(&self.collection(), &FieldId::new(name).unwrap())
            .unwrap()
    }
    fn projection(&self) -> RuntimeProjection {
        self.schema()
            .resolve_projection(
                &self.collection(),
                &[
                    self.field("id"),
                    self.field("label"),
                    self.field("score"),
                    self.field("optional"),
                    self.field("occurred"),
                ],
            )
            .unwrap()
    }
    fn authority(&self) -> Authority {
        Authority {
            projection: self.projection(),
            predicate: Some(
                self.schema()
                    .runtime_compare(
                        &self.collection(),
                        &self.field("tenant"),
                        RuntimeScalarOperator::Eq,
                        RuntimeValue::String("north".into()),
                        Default::default(),
                    )
                    .unwrap(),
            ),
            expected_policy_revision: 1,
            pinned_policy_revision: 1,
            deny_intent: false,
            deny_preimage: false,
            preimage_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            deny_result: false,
            pending_result: false,
            staged: Arc::new(AtomicBool::new(false)),
            wrong_return: false,
        }
    }
    fn create(&self, id: i64) -> RuntimeMutationRequest {
        self.schema()
            .runtime_create_request(
                &self.collection(),
                &[
                    (self.field("id"), RuntimeValue::Integer(id)),
                    (self.field("tenant"), RuntimeValue::String("north".into())),
                    (self.field("label"), RuntimeValue::String("initial".into())),
                ],
                Some(self.projection()),
                Default::default(),
            )
            .unwrap()
    }
    fn key(&self, id: i64) -> RuntimeKey {
        self.schema()
            .runtime_key(
                &self.collection(),
                &[(self.field("id"), RuntimeValue::Integer(id))],
            )
            .unwrap()
    }
    async fn run(
        &self,
        request: RuntimeMutationRequest,
        authority: Authority,
    ) -> Result<RuntimeMutationEffect, TransactionError> {
        let environment = self.environment.clone();
        self.db
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    tx.mutate_runtime(&environment, &request, &authority)
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
    }
    async fn count(&self) -> i64 {
        let rows = Backend::fetch_rows(
            self.db.pool(),
            "SELECT COUNT(*) AS n FROM runtime_records",
            &[],
        )
        .await
        .unwrap();
        Backend::try_get_i64(&rows[0], "n").unwrap()
    }
    async fn finish(mut self) {
        self.db.pool().close().await;
        #[cfg(feature = "postgres")]
        self.owned.cleanup().unwrap();
        #[cfg(feature = "sqlite")]
        {
            let _ = &mut self;
        }
    }
}
struct Authority {
    projection: RuntimeProjection,
    predicate: Option<RuntimePredicate>,
    expected_policy_revision: u64,
    pinned_policy_revision: u64,
    deny_intent: bool,
    deny_preimage: bool,
    preimage_calls: Arc<std::sync::atomic::AtomicUsize>,
    deny_result: bool,
    pending_result: bool,
    staged: Arc<AtomicBool>,
    wrong_return: bool,
}
impl RuntimeWriteAuthority<Backend> for Authority {
    fn authorize_intent<'a>(
        &'a self,
        _check: RuntimeWriteIntent<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
            if self.expected_policy_revision != self.pinned_policy_revision {
                return Err(RuntimeMutationError::new(
                    RuntimeMutationErrorCode::SchemaMismatch,
                ));
            }
            if self.deny_intent {
                return Err(RuntimeMutationError::new(
                    RuntimeMutationErrorCode::FieldDenied,
                ));
            }
            RuntimeWriteGrant::new(self.projection.clone(), self.predicate.clone())
        })
    }
    fn authorize_preimage<'a>(
        &'a self,
        _check: RuntimePreimageCheck<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async move {
            self.preimage_calls.fetch_add(1, Ordering::SeqCst);
            if self.deny_preimage {
                return Err(RuntimeMutationError::new(RuntimeMutationErrorCode::Denied));
            }
            Ok(())
        })
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        Box::pin(async move {
            if self.pending_result {
                tx.insert::<Journal>(CreateJournalInput {
                    id: "pending".into(),
                    message: "staged".into(),
                })
                .await
                .map_err(|e| {
                    RuntimeMutationError::new(RuntimeMutationErrorCode::HookFailed).with_source(e)
                })?;
                tx.queue_event(19u32);
                self.staged.store(true, Ordering::SeqCst);
                std::future::pending::<()>().await;
            }
            if self.deny_result {
                return Err(RuntimeMutationError::new(RuntimeMutationErrorCode::Denied));
            }
            Ok(RuntimeReturnGrant::new(if self.wrong_return {
                None
            } else {
                check.intent.returning().cloned()
            }))
        })
    }
}
struct JournalHook {
    staged: Arc<AtomicBool>,
    pending: bool,
    fail: bool,
}
impl RuntimeMutationHook<Backend> for JournalHook {
    fn before_commit<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, Backend>,
        _pending: &'a RuntimeMutationEffect,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async move {
            tx.insert::<Journal>(CreateJournalInput {
                id: "hook".into(),
                message: "staged".into(),
            })
            .await
            .map_err(|e| {
                RuntimeMutationError::new(RuntimeMutationErrorCode::HookFailed).with_source(e)
            })?;
            tx.queue_event(20u32);
            self.staged.store(true, Ordering::SeqCst);
            if self.pending {
                std::future::pending::<()>().await;
            }
            if self.fail {
                return Err(RuntimeMutationError::new(
                    RuntimeMutationErrorCode::HookFailed,
                ));
            }
            Ok(())
        })
    }
}
#[tokio::test]
async fn create_patch_cas_delete_defaults_and_private_return_projection() {
    let f = Fixture::new().await;
    let row = f.run(f.create(1), f.authority()).await.unwrap();
    assert_eq!(row.record().unwrap().integer(&f.field("score")).unwrap(), 7);
    assert!(matches!(
        row.record().unwrap().state(&f.field("optional")).unwrap(),
        RuntimeFieldState::Null
    ));
    assert!(row.record().unwrap().datetime(&f.field("occurred")).is_ok());
    assert!(row.record().unwrap().value(&f.field("tenant")).is_err());
    let expected = f
        .schema()
        .runtime_compare(
            &f.collection(),
            &f.field("score"),
            RuntimeScalarOperator::Eq,
            RuntimeValue::Integer(7),
            Default::default(),
        )
        .unwrap();
    let patch = f
        .schema()
        .runtime_update_request(
            f.key(1),
            &[(f.field("score"), RuntimeValue::Integer(-123))],
            Some(expected.clone()),
            Some(f.projection()),
            Default::default(),
        )
        .unwrap();
    let row = f.run(patch.clone(), f.authority()).await.unwrap();
    assert_eq!(
        row.record().unwrap().integer(&f.field("score")).unwrap(),
        -123
    );
    assert_eq!(
        row.record().unwrap().string(&f.field("label")).unwrap(),
        "initial"
    );
    let conflict = f.run(patch, f.authority()).await.unwrap_err();
    assert_eq!(
        conflict.public_error().runtime_mutation_code(),
        Some("conflict")
    );
    assert_eq!(
        conflict.commit_outcome(),
        TransactionCommitOutcome::RolledBack
    );
    let delete = f
        .schema()
        .runtime_delete_request(f.key(1), None, Some(f.projection()), Default::default())
        .unwrap();
    f.run(delete, f.authority()).await.unwrap();
    assert_eq!(f.count().await, 0);
    f.finish().await;
}
#[tokio::test]
async fn caught_result_timeout_rolls_back_record_journal_events_and_reuses_connection() {
    let f = Fixture::new().await;
    let environment = f.environment.clone();
    let request = f.create(1);
    let mut authority = f.authority();
    authority.pending_result = true;
    let staged = authority.staged.clone();
    let mut events = f.db.ensure_event_sender::<u32>().subscribe();
    let result =
        f.db.transaction(TransactionMode::StateMachine, move |tx| {
            Box::pin(async move {
                assert!(
                    tokio::time::timeout(
                        Duration::from_millis(150),
                        tx.mutate_runtime(&environment, &request, &authority)
                    )
                    .await
                    .is_err()
                );
                Ok(())
            })
        })
        .await;
    assert!(staged.load(Ordering::SeqCst));
    assert!(matches!(result, Err(TransactionError::Rejected(_))));
    assert_eq!(f.count().await, 0);
    assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
    assert!(events.try_recv().is_err());
    f.run(f.create(2), f.authority()).await.unwrap();
    assert_eq!(f.count().await, 1);
    f.finish().await;
}
#[tokio::test]
async fn caught_hook_timeout_and_error_poison_successful_mutations() {
    let f = Fixture::new().await;
    for pending in [true, false] {
        let environment = f.environment.clone();
        let request = f.create(1);
        let authority = f.authority();
        let mut events = f.db.ensure_event_sender::<u32>().subscribe();
        let result =
            f.db.transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let effect = tx
                        .mutate_runtime(&environment, &request, &authority)
                        .await?;
                    let hook = JournalHook {
                        staged: Arc::new(AtomicBool::new(false)),
                        pending,
                        fail: !pending,
                    };
                    let result = tokio::time::timeout(
                        Duration::from_millis(150),
                        tx.run_runtime_before_commit(&hook, &effect),
                    )
                    .await;
                    assert!(result.is_err() || result.unwrap().is_err());
                    assert!(hook.staged.load(Ordering::SeqCst));
                    Ok(())
                })
            })
            .await;
        assert!(matches!(result, Err(TransactionError::Rejected(_))));
        assert_eq!(f.count().await, 0);
        assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
        assert!(events.try_recv().is_err());
    }
    f.finish().await;
}
#[tokio::test]
async fn denied_intent_preimage_result_and_output_are_fail_closed() {
    let f = Fixture::new().await;
    for stage in 0..3 {
        let mut authority = f.authority();
        authority.deny_intent = stage == 0;
        authority.deny_result = stage == 1;
        authority.wrong_return = stage == 2;
        assert!(f.run(f.create(1), authority).await.is_err());
        assert_eq!(f.count().await, 0);
    }
    f.run(f.create(1), f.authority()).await.unwrap();
    let request = f
        .schema()
        .runtime_delete_request(f.key(1), None, None, Default::default())
        .unwrap();
    let mut authority = f.authority();
    authority.deny_preimage = true;
    assert_eq!(
        f.run(request, authority)
            .await
            .unwrap_err()
            .public_error()
            .runtime_mutation_code(),
        Some("not_found")
    );
    assert_eq!(f.count().await, 1);
    f.finish().await;
}
#[tokio::test]
async fn journal_success_and_propagated_failure_are_atomic() {
    let f = Fixture::new().await;
    for id in [1, 2] {
        let environment = f.environment.clone();
        let request = f.create(id);
        let authority = f.authority();
        let result =
            f.db.transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let effect = tx
                        .mutate_runtime(&environment, &request, &authority)
                        .await?;
                    tx.run_runtime_before_commit(
                        &JournalHook {
                            staged: Arc::new(AtomicBool::new(false)),
                            pending: false,
                            fail: false,
                        },
                        &effect,
                    )
                    .await?;
                    Ok(effect)
                })
            })
            .await;
        assert_eq!(result.is_ok(), id == 1);
    }
    assert_eq!(f.count().await, 1);
    assert_eq!(Journal::count_all(&f.db).await.unwrap(), 1);
    f.finish().await;
}

#[tokio::test]
async fn nullable_patch_omission_and_explicit_null_remain_distinct() {
    let f = Fixture::new().await;
    f.run(f.create(1), f.authority()).await.unwrap();
    for value in [RuntimeValue::String("present".into()), RuntimeValue::Null] {
        let request = f
            .schema()
            .runtime_update_request(
                f.key(1),
                &[(f.field("optional"), value.clone())],
                None,
                Some(f.projection()),
                Default::default(),
            )
            .unwrap();
        let row = f.run(request, f.authority()).await.unwrap();
        let actual = row.record().unwrap().state(&f.field("optional")).unwrap();
        match &value {
            RuntimeValue::Null => assert!(matches!(actual, RuntimeFieldState::Null)),
            _ => assert!(matches!(actual,RuntimeFieldState::Value(v) if v==&value)),
        }
        let request = f
            .schema()
            .runtime_update_request(
                f.key(1),
                &[(f.field("label"), RuntimeValue::String("changed".into()))],
                None,
                Some(f.projection()),
                Default::default(),
            )
            .unwrap();
        let row = f.run(request, f.authority()).await.unwrap();
        let actual = row.record().unwrap().state(&f.field("optional")).unwrap();
        match &value {
            RuntimeValue::Null => assert!(matches!(actual, RuntimeFieldState::Null)),
            _ => assert!(matches!(actual,RuntimeFieldState::Value(v) if v==&value)),
        }
    }
    f.finish().await;
}
#[tokio::test]
async fn competing_structural_cas_writers_cannot_both_commit() {
    let f = Fixture::new().await;
    f.run(f.create(1), f.authority()).await.unwrap();
    let expected = f
        .schema()
        .runtime_compare(
            &f.collection(),
            &f.field("score"),
            RuntimeScalarOperator::Eq,
            RuntimeValue::Integer(7),
            Default::default(),
        )
        .unwrap();
    let request = f
        .schema()
        .runtime_update_request(
            f.key(1),
            &[(f.field("score"), RuntimeValue::Integer(8))],
            Some(expected),
            None,
            Default::default(),
        )
        .unwrap();
    let (first, second) = tokio::join!(
        f.run(request.clone(), f.authority()),
        f.run(request.clone(), f.authority())
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let failure = if let Err(e) = first {
        e
    } else {
        second.unwrap_err()
    };
    assert!(
        failure.is_retryable()
            || failure.public_error().runtime_mutation_code() == Some("conflict")
    );
    let after = f.run(request, f.authority()).await.unwrap_err();
    assert_eq!(
        after.public_error().runtime_mutation_code(),
        Some("conflict")
    );
    f.finish().await;
}
#[tokio::test]
async fn caught_denial_is_monotonic_and_unpolled_future_stages_nothing() {
    let f = Fixture::new().await;
    let environment = f.environment.clone();
    let request = f.create(1);
    let authority = f.authority();
    f.db.transaction(TransactionMode::StateMachine, move |tx| {
        Box::pin(async move {
            drop(tx.mutate_runtime(&environment, &request, &authority));
            Ok(())
        })
    })
    .await
    .unwrap();
    assert_eq!(f.count().await, 0);
    let environment = f.environment.clone();
    let request = f.create(1);
    let mut authority = f.authority();
    authority.deny_result = true;
    let good = f.authority();
    let later = f.create(2);
    let result =
        f.db.transaction(TransactionMode::StateMachine, move |tx| {
            Box::pin(async move {
                assert!(
                    tx.mutate_runtime(&environment, &request, &authority)
                        .await
                        .is_err()
                );
                // A later successful operation cannot clear the earlier poison.
                tx.mutate_runtime(&environment, &later, &good)
                    .await
                    .unwrap();
                Ok(())
            })
        })
        .await;
    assert!(result.is_err());
    assert_eq!(f.count().await, 0);
    f.finish().await;
}
#[tokio::test]
async fn row_visibility_hides_existence_before_cas_conflict_disclosure() {
    let f = Fixture::new().await;
    f.run(f.create(1), f.authority()).await.unwrap();
    let request = f
        .schema()
        .runtime_delete_request(f.key(1), None, None, Default::default())
        .unwrap();
    let mut authority = f.authority();
    authority.predicate = Some(
        f.schema()
            .runtime_compare(
                &f.collection(),
                &f.field("tenant"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::String("south".into()),
                Default::default(),
            )
            .unwrap(),
    );
    assert_eq!(
        f.run(request, authority)
            .await
            .unwrap_err()
            .public_error()
            .runtime_mutation_code(),
        Some("not_found")
    );
    assert_eq!(f.count().await, 1);
    f.finish().await;
}
#[test]
fn requests_reject_missing_keys_wrong_types_stale_handles_append_only_and_limits() {
    let schema = definition().validate().unwrap();
    let c = schema
        .resolve_collection(&CollectionId::new("records").unwrap())
        .unwrap();
    let id = schema
        .resolve_field(&c, &FieldId::new("id").unwrap())
        .unwrap();
    assert!(schema.runtime_key(&c, &[]).is_err());
    assert!(
        schema
            .runtime_key(&c, &[(id.clone(), RuntimeValue::Null)])
            .is_err()
    );
    assert!(
        schema
            .runtime_key(&c, &[(id.clone(), RuntimeValue::String("1".into()))])
            .is_err()
    );
    assert_eq!(
        schema
            .runtime_create_request(&c, &[], None, Default::default())
            .unwrap_err()
            .code(),
        RuntimeMutationErrorCode::MissingRequired
    );
    let key = schema
        .runtime_key(&c, &[(id.clone(), RuntimeValue::Integer(1))])
        .unwrap();
    assert_eq!(
        schema
            .runtime_update_request(
                key.clone(),
                &[(id.clone(), RuntimeValue::Integer(2))],
                None,
                None,
                Default::default()
            )
            .unwrap_err()
            .code(),
        RuntimeMutationErrorCode::FieldDenied
    );
    assert!(
        schema
            .runtime_update_request(key.clone(), &[], None, None, Default::default())
            .is_err()
    );
    let mut changed = definition();
    changed.collections[0].append_only = true;
    let changed = changed.validate().unwrap();
    assert!(
        changed
            .runtime_delete_request(key, None, None, Default::default())
            .is_err()
    );
    let c = changed.resolve_collection(c.id()).unwrap();
    let id = changed.resolve_field(&c, id.id()).unwrap();
    let key = changed
        .runtime_key(&c, &[(id, RuntimeValue::Integer(1))])
        .unwrap();
    assert_eq!(
        changed
            .runtime_delete_request(key, None, None, Default::default())
            .unwrap_err()
            .code(),
        RuntimeMutationErrorCode::AppendOnly
    );
}

#[tokio::test]
async fn whole_transaction_cancellation_after_staged_work_rolls_back() {
    let f = Fixture::new().await;
    let mut authority = f.authority();
    authority.pending_result = true;
    let staged = authority.staged.clone();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), f.run(f.create(1), authority))
            .await
            .is_err()
    );
    assert!(staged.load(Ordering::SeqCst));
    assert_eq!(f.count().await, 0);
    assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
    f.run(f.create(2), f.authority()).await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn denied_fields_are_checked_before_target_io() {
    let f = Fixture::new().await;
    let mut authority = f.authority();
    authority.deny_intent = true;
    <Backend as WriteBackend>::execute_with_binds(f.db.pool(), "DROP TABLE runtime_records", &[])
        .await
        .unwrap();
    // A target query here would fail. Host intent denial wins before target I/O.
    assert_eq!(
        f.run(f.create(1), authority)
            .await
            .unwrap_err()
            .public_error()
            .runtime_mutation_code(),
        Some("field_denied")
    );
    f.finish().await;
}
#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_auth_context_is_pinned_and_rls_is_not_bypassed() {
    use std::str::FromStr;
    let f = Fixture::new().await;
    for ddl in [
        "ALTER TABLE runtime_records ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE runtime_records FORCE ROW LEVEL SECURITY",
        "CREATE POLICY tenant_rows ON runtime_records USING (tenant = current_setting('app.tenant_id', true)) WITH CHECK (tenant = current_setting('app.tenant_id', true))",
        "CREATE ROLE runtime_writer LOGIN PASSWORD 'owned-fixture-only'",
        "GRANT USAGE ON SCHEMA public TO runtime_writer",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON runtime_records, runtime_journal TO runtime_writer",
    ] {
        sqlx::query(ddl).execute(f.db.pool()).await.unwrap();
    }
    let options = sqlx::postgres::PgConnectOptions::from_str(&f.owned.url)
        .unwrap()
        .username("runtime_writer")
        .password("owned-fixture-only");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let db = Database::<Backend>::new(pool.clone());
    for tenant in ["north", "south"] {
        let environment = f.environment.clone();
        let request = f.create(if tenant == "north" { 1 } else { 2 });
        let authority = f.authority();
        let auth = DbAuthContext {
            tenant_id: Some(tenant.into()),
            ..Default::default()
        };
        let result = db
            .transaction_with_auth(TransactionMode::StateMachine, Some(&auth), move |tx| {
                Box::pin(async move {
                    tx.mutate_runtime(&environment, &request, &authority)
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await;
        assert_eq!(result.is_ok(), tenant == "north");
    }
    assert_eq!(f.count().await, 1);
    let setting: Option<String> =
        sqlx::query_scalar("SELECT nullif(current_setting('app.tenant_id', true), '')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(setting.is_none());
    pool.close().await;
    f.finish().await;
}

#[tokio::test]
async fn generated_uuid_json_null_bytes_and_exact_values_use_existing_runtime_types() {
    let mut schema = definition();
    schema.collections[0].fields[0].value_kind = RuntimeValueKind::Uuid;
    schema.collections[0].fields[0].generated = true;
    for (name, kind) in [
        ("json", RuntimeValueKind::Json),
        ("bytes", RuntimeValueKind::Bytes),
    ] {
        schema.collections[0].fields.push(RuntimeField {
            id: FieldId::new(name).unwrap(),
            api_name: name.into(),
            physical_column: name.into(),
            value_kind: kind,
            nullable: true,
            unique: false,
            filterable: false,
            sortable: false,
            generated: false,
            default: None,
        });
    }
    let f = Fixture::with_definition(schema).await;
    let projection = f
        .schema()
        .resolve_projection(
            &f.collection(),
            &[
                f.field("id"),
                f.field("json"),
                f.field("bytes"),
                f.field("occurred"),
                f.field("score"),
            ],
        )
        .unwrap();
    let datetime = RuntimeDateTime::parse("2026-10-07T01:02:03.123456Z").unwrap();
    let request = f
        .schema()
        .runtime_create_request(
            &f.collection(),
            &[
                (f.field("tenant"), RuntimeValue::String("north".into())),
                (f.field("label"), RuntimeValue::String("exact".into())),
                (f.field("json"), RuntimeValue::Json(serde_json::Value::Null)),
                (f.field("bytes"), RuntimeValue::Bytes(vec![0, 255, 128])),
                (f.field("score"), RuntimeValue::Integer(i64::MIN)),
                (
                    f.field("occurred"),
                    RuntimeValue::DateTime(datetime.clone()),
                ),
            ],
            Some(projection.clone()),
            Default::default(),
        )
        .unwrap();
    let row = f.run(request, f.authority()).await.unwrap();
    let record = row.record().unwrap();
    assert!(record.uuid(&f.field("id")).is_ok());
    assert!(matches!(
        record.value(&f.field("json")).unwrap(),
        RuntimeValue::Json(serde_json::Value::Null)
    ));
    assert_eq!(record.bytes(&f.field("bytes")).unwrap(), &[0, 255, 128]);
    assert_eq!(record.datetime(&f.field("occurred")).unwrap(), &datetime);
    assert_eq!(record.integer(&f.field("score")).unwrap(), i64::MIN);
    let key = f
        .schema()
        .runtime_key(
            &f.collection(),
            &[(f.field("id"), record.value(&f.field("id")).unwrap().clone())],
        )
        .unwrap();
    let request = f
        .schema()
        .runtime_update_request(
            key,
            &[
                (f.field("json"), RuntimeValue::Null),
                (f.field("score"), RuntimeValue::Integer(i64::MAX)),
            ],
            None,
            Some(projection),
            Default::default(),
        )
        .unwrap();
    let row = f.run(request, f.authority()).await.unwrap();
    assert!(matches!(
        row.record().unwrap().state(&f.field("json")).unwrap(),
        RuntimeFieldState::Null
    ));
    assert_eq!(
        row.record().unwrap().integer(&f.field("score")).unwrap(),
        i64::MAX
    );
    f.finish().await;
}
#[tokio::test]
async fn complete_composite_keys_and_required_create_members() {
    let mut schema = definition();
    schema.collections[0]
        .primary_key
        .push(FieldId::new("tenant").unwrap());
    let f = Fixture::with_definition(schema).await;
    assert!(
        f.schema()
            .runtime_key(
                &f.collection(),
                &[(f.field("id"), RuntimeValue::Integer(1))]
            )
            .is_err()
    );
    f.run(f.create(1), f.authority()).await.unwrap();
    let key = f
        .schema()
        .runtime_key(
            &f.collection(),
            &[
                (f.field("tenant"), RuntimeValue::String("north".into())),
                (f.field("id"), RuntimeValue::Integer(1)),
            ],
        )
        .unwrap();
    assert_eq!(key.fields()[0].0.id(), f.field("id").id());
    let request = f
        .schema()
        .runtime_delete_request(key, None, None, Default::default())
        .unwrap();
    f.run(request, f.authority()).await.unwrap();
    assert_eq!(f.count().await, 0);
    f.finish().await;
}
#[tokio::test]
async fn incoming_modifying_dependencies_fail_closed_while_restrict_remains_usable() {
    for action in [
        DeletePolicy::Cascade,
        DeletePolicy::SetNull,
        DeletePolicy::Restrict,
    ] {
        let mut schema = definition();
        schema.collections[0].fields.push(RuntimeField {
            id: FieldId::new("parent").unwrap(),
            api_name: "parent".into(),
            physical_column: "parent".into(),
            value_kind: RuntimeValueKind::Integer,
            nullable: true,
            unique: false,
            filterable: false,
            sortable: false,
            generated: false,
            default: None,
        });
        schema.collections[0].relations.push(RuntimeRelation {
            id: RelationId::new("parent_link").unwrap(),
            api_name: "parentLink".into(),
            target: CollectionId::new("records").unwrap(),
            key_pairs: vec![RelationKeyPair {
                source: FieldId::new("parent").unwrap(),
                target: FieldId::new("id").unwrap(),
            }],
            cardinality: RelationCardinality::One,
            enforce_foreign_key: true,
            on_delete: Some(action.clone()),
        });
        let f = Fixture::with_definition(schema).await;
        f.run(f.create(1), f.authority()).await.unwrap();
        let request = f
            .schema()
            .runtime_delete_request(f.key(1), None, None, Default::default())
            .unwrap();
        let result = f.run(request, f.authority()).await;
        if action == DeletePolicy::Restrict {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result.unwrap_err().public_error().runtime_mutation_code(),
                Some("unsupported_cascade")
            );
            assert_eq!(f.count().await, 1);
        }
        f.finish().await;
    }
}

struct PendingIntent;
impl RuntimeWriteAuthority<Backend> for PendingIntent {
    fn authorize_intent<'a>(
        &'a self,
        _: RuntimeWriteIntent<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(std::future::pending())
    }
    fn authorize_preimage<'a>(
        &'a self,
        _: RuntimePreimageCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async { panic!("preimage must not be reached") })
    }
    fn authorize_result<'a>(
        &'a self,
        _: RuntimeResultCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        Box::pin(async { panic!("result must not be reached") })
    }
}
#[tokio::test]
async fn cancellation_before_dml_also_poisons_preceding_host_journal_work() {
    let f = Fixture::new().await;
    let environment = f.environment.clone();
    let request = f.create(1);
    let result =
        f.db.transaction(TransactionMode::StateMachine, move |tx| {
            Box::pin(async move {
                tx.insert::<Journal>(CreateJournalInput {
                    id: "before".into(),
                    message: "staged".into(),
                })
                .await?;
                assert!(
                    tokio::time::timeout(
                        Duration::from_millis(50),
                        tx.mutate_runtime(&environment, &request, &PendingIntent)
                    )
                    .await
                    .is_err()
                );
                Ok(())
            })
        })
        .await;
    assert!(matches!(result, Err(TransactionError::Rejected(_))));
    assert_eq!(f.count().await, 0);
    assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
    f.finish().await;
}
#[test]
fn bounded_inputs_and_predicate_values_reject_without_database_access() {
    let schema = definition().validate().unwrap();
    let c = schema
        .resolve_collection(&CollectionId::new("records").unwrap())
        .unwrap();
    let id = schema
        .resolve_field(&c, &FieldId::new("id").unwrap())
        .unwrap();
    let label = schema
        .resolve_field(&c, &FieldId::new("label").unwrap())
        .unwrap();
    let key = schema
        .runtime_key(&c, &[(id, RuntimeValue::Integer(1))])
        .unwrap();
    let limits = RuntimeMutationLimits {
        max_value_bytes: 64,
        ..Default::default()
    };
    let value = RuntimeValue::String("fixture-private-value".repeat(10));
    let error = schema
        .runtime_update_request(
            key.clone(),
            &[(label.clone(), value.clone())],
            None,
            None,
            limits,
        )
        .unwrap_err();
    assert_eq!(error.code(), RuntimeMutationErrorCode::LimitExceeded);
    assert!(!format!("{error:?}").contains("fixture-private"));
    let expected = schema
        .runtime_compare(
            &c,
            &label,
            RuntimeScalarOperator::Eq,
            value,
            Default::default(),
        )
        .unwrap();
    assert_eq!(
        schema
            .runtime_update_request(
                key,
                &[(label, RuntimeValue::String("x".into()))],
                Some(expected),
                None,
                limits
            )
            .unwrap_err()
            .code(),
        RuntimeMutationErrorCode::LimitExceeded
    );
}

#[tokio::test]
async fn combined_key_cas_and_authority_bind_budget_rejects_before_target_reads() {
    let f = Fixture::new().await;
    let expected = f
        .schema()
        .runtime_compare(
            &f.collection(),
            &f.field("score"),
            RuntimeScalarOperator::Eq,
            RuntimeValue::Integer(7),
            Default::default(),
        )
        .unwrap();
    let limits = RuntimeMutationLimits {
        query: RuntimeQueryLimits {
            max_bind_parameters: 3,
            ..Default::default()
        },
        ..Default::default()
    };
    let request = f
        .schema()
        .runtime_update_request(
            f.key(1),
            &[(f.field("label"), RuntimeValue::String("changed".into()))],
            Some(expected),
            None,
            limits,
        )
        .unwrap();
    <Backend as WriteBackend>::execute_with_binds(f.db.pool(), "DROP TABLE runtime_records", &[])
        .await
        .unwrap();
    assert_eq!(
        f.run(request, f.authority())
            .await
            .unwrap_err()
            .public_error()
            .runtime_mutation_code(),
        Some("limit_exceeded")
    );
    f.finish().await;
}

#[tokio::test]
async fn host_policy_only_revision_changes_reject_unchanged_schema_requests() {
    let f = Fixture::new().await;
    let fingerprint = f.schema().fingerprint();
    let request = f.create(1);
    let mut authority = f.authority();
    // Host obtains these values from its pinned policy generation, not the ORM schema.
    authority.pinned_policy_revision = 2;
    let error = f.run(request, authority).await.unwrap_err();
    assert_eq!(
        error.public_error().runtime_mutation_code(),
        Some("schema_mismatch")
    );
    assert_eq!(f.schema().fingerprint(), fingerprint);
    assert_eq!(f.count().await, 0);
    let mut fresh = f.authority();
    fresh.pinned_policy_revision = 2;
    fresh.expected_policy_revision = 2;
    f.run(f.create(1), fresh).await.unwrap();
    f.finish().await;
}

// PostgreSQL executes only rejection cases until the reviewed #112/A/B reconciliation.
// The renderer's native-slot accounting is independently covered by unit tests.
#[tokio::test]
async fn emitted_suffix_bind_budgets_precede_target_reads_for_cas_and_authority() {
    let f = Fixture::new().await;
    let suffix = f
        .schema()
        .runtime_compare(
            &f.collection(),
            &f.field("label"),
            RuntimeScalarOperator::EndsWith,
            RuntimeValue::String("tial".into()),
            Default::default(),
        )
        .unwrap();
    let score = f
        .schema()
        .runtime_compare(
            &f.collection(),
            &f.field("score"),
            RuntimeScalarOperator::Eq,
            RuntimeValue::Integer(7),
            Default::default(),
        )
        .unwrap();
    let blocked = f
        .schema()
        .runtime_compare(
            &f.collection(),
            &f.field("label"),
            RuntimeScalarOperator::Eq,
            RuntimeValue::String("blocked".into()),
            Default::default(),
        )
        .unwrap();
    let not_blocked = f
        .schema()
        .runtime_not(&f.collection(), blocked, Default::default())
        .unwrap();
    let either = f
        .schema()
        .runtime_or(
            &f.collection(),
            vec![suffix.clone(), not_blocked],
            Default::default(),
        )
        .unwrap();
    let nested_cas = f
        .schema()
        .runtime_and(
            &f.collection(),
            vec![
                suffix.clone(),
                f.schema()
                    .runtime_or(
                        &f.collection(),
                        vec![score.clone(), score.clone()],
                        Default::default(),
                    )
                    .unwrap(),
            ],
            Default::default(),
        )
        .unwrap();
    let tenant = f.authority().predicate.unwrap();
    let nested_authority = f
        .schema()
        .runtime_and(
            &f.collection(),
            vec![tenant.clone(), either],
            Default::default(),
        )
        .unwrap();
    for (case, (expected, predicate, sqlite_slots, postgres_slots)) in [
        (suffix, tenant.clone(), 5, 4),
        (nested_cas.clone(), tenant, 7, 6),
        (score, nested_authority.clone(), 7, 6),
        (nested_cas, nested_authority, 10, 8),
    ]
    .into_iter()
    .enumerate()
    {
        let required = if Backend::DIALECT == DatabaseBackend::Sqlite {
            sqlite_slots
        } else {
            postgres_slots
        };
        for budget in [required - 1, required, required + 1] {
            if Backend::DIALECT == DatabaseBackend::Postgres && budget >= required {
                continue;
            }
            let id = (case * 3 + budget + 2 - required) as i64;
            f.run(f.create(id), f.authority()).await.unwrap();
            let request = f
                .schema()
                .runtime_update_request(
                    f.key(id),
                    &[(f.field("score"), RuntimeValue::Integer(8))],
                    Some(expected.clone()),
                    None,
                    RuntimeMutationLimits {
                        query: RuntimeQueryLimits {
                            max_bind_parameters: budget,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
                .unwrap();
            let mut authority = f.authority();
            authority.predicate = Some(predicate.clone());
            let calls = authority.preimage_calls.clone();
            let result = f.run(request, authority).await;
            if budget < required {
                assert_eq!(
                    result.unwrap_err().public_error().runtime_mutation_code(),
                    Some("limit_exceeded")
                );
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    0,
                    "case {case}: preimage authorization ran before rejecting the bind budget"
                );
            } else {
                result.unwrap();
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            }
            let sql = format!(
                "SELECT score FROM runtime_records WHERE id = {}",
                Backend::DIALECT.placeholder(1)
            );
            let rows = Backend::fetch_rows(f.db.pool(), &sql, &[SqlValue::Int(id)])
                .await
                .unwrap();
            assert_eq!(
                Backend::try_get_i64(&rows[0], "score").unwrap(),
                if budget < required { 7 } else { 8 }
            );
        }
    }
    f.finish().await;
}
