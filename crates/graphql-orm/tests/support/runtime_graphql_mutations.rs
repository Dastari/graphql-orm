// Reuse B's owned database, journal, policy and cancellation fixtures.
use super::*;
use graphql_orm::{
    async_graphql::{Request, Variables, dynamic::Schema},
    graphql::runtime::*,
};
struct Read;
impl RuntimeReadAuthority for Read {
    fn authorize<'a>(
        &'a self,
        check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Box::pin(async move {
            if check
                .filter_operations
                .iter()
                .any(|(field, _)| field.id().as_str() == "tenant")
            {
                return Err(RuntimeGraphqlError::denied());
            }
            Ok(RuntimeReadGrant::new(None))
        })
    }
}
fn api(f: &Fixture) -> Schema {
    let module = RuntimeGraphqlModule::compile(
        Arc::new(f.schema().clone()),
        RuntimeGraphqlOptions::default()
            .with_cursor_profile(RuntimeCursorProfile::Unprotected)
            .with_mutations(Default::default()),
    )
    .unwrap();
    RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
        .unwrap()
        .mutation_root("Mutation")
        .unwrap()
        .install(module)
        .unwrap()
        .finish()
        .unwrap()
}
fn request(f: &Fixture, query: &str) -> Request {
    Request::new(query)
        .variables(Variables::from_json(serde_json::json!({
            "schema": f.schema().fingerprint().to_string()
        })))
        .data(context(f))
}
fn context(f: &Fixture) -> RuntimeGraphqlRequest<Backend> {
    RuntimeGraphqlRequest::new(
        f.schema().fingerprint(),
        Arc::new(Read),
        None,
        Default::default(),
    )
    .with_mutation_environment(f.environment.clone())
    .with_write_authority(Arc::new(f.authority()))
}
#[tokio::test]
async fn typed_roots_delegate_create_cas_delete_and_require_keys() {
    let f = Fixture::new().await;
    let api = api(&f);
    let sdl = api.sdl();
    assert!(sdl.contains("id: RuntimeInt64!"));
    let invalid = api
        .execute(request(
            &f,
            r#"mutation($schema:String!){
        createRecord(input:{tenant:"north",label:"initial"},expectedSchema:$schema){__typename}
    }"#,
        ))
        .await;
    assert!(!invalid.errors.is_empty());
    assert_eq!(f.count().await, 0);
    let created = api.execute(request(&f, r#"mutation($schema:String!){
        createRecord(input:{id:"1",tenant:"north",label:"initial"},expectedSchema:$schema){__typename}
    }"#)).await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    assert_eq!(f.count().await, 1);
    let update = r#"mutation($schema:String!){
        updateRecord(key:{id:"1"},input:{label:"updated"},expected:{label:{eq:"initial"}},
            expectedSchema:$schema){__typename}
    }"#;
    let updated = api.execute(request(&f, update)).await;
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);
    let conflict = api.execute(request(&f, update)).await;
    assert!(!conflict.errors.is_empty());
    let denied = api
        .execute(request(
            &f,
            r#"mutation($schema:String!){
        updateRecord(key:{id:"1"},input:{score:"8"},expected:{tenant:{notIn:[]}},
            expectedSchema:$schema){__typename}
    }"#,
        ))
        .await;
    assert!(!denied.errors.is_empty());
    let deleted = api
        .execute(request(
            &f,
            r#"mutation($schema:String!){
        deleteRecord(key:{id:"1"},expectedSchema:$schema){__typename}
    }"#,
        ))
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    assert_eq!(f.count().await, 0);
    f.finish().await;
}

const CREATE: &str = r#"mutation($schema:String!){
    createRecord(input:{id:"1",tenant:"north",label:"initial"},expectedSchema:$schema){__typename}
}"#;
#[tokio::test]
async fn guarded_journal_failure_and_whole_request_cancellation_roll_back() {
    let f = Fixture::new().await;
    let api = api(&f);
    let mut events = f.db.ensure_event_sender::<u32>().subscribe();
    for pending in [false, true] {
        let staged = Arc::new(AtomicBool::new(false));
        let hook = Arc::new(JournalHook {
            staged: staged.clone(),
            pending,
            fail: !pending,
        });
        let request = request(&f, CREATE).data(context(&f).with_mutation_hook(hook));
        if pending {
            assert!(
                tokio::time::timeout(Duration::from_millis(150), api.execute(request))
                    .await
                    .is_err()
            );
        } else {
            assert!(!api.execute(request).await.errors.is_empty());
        }
        assert!(staged.load(Ordering::SeqCst));
        assert_eq!(f.count().await, 0);
        assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
        assert!(events.try_recv().is_err());
    }
    let request = request(&f, CREATE).data(context(&f).with_mutation_hook(Arc::new(JournalHook {
        staged: Arc::new(AtomicBool::new(false)),
        pending: false,
        fail: false,
    })));
    let response = api.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(f.count().await, 1);
    assert_eq!(Journal::count_all(&f.db).await.unwrap(), 1);
    assert_eq!(events.try_recv().unwrap(), 20);
    f.finish().await;
}
#[tokio::test]
async fn competing_graphql_cas_roots_cannot_both_commit() {
    let f = Fixture::new().await;
    let api = api(&f);
    assert!(api.execute(request(&f, CREATE)).await.errors.is_empty());
    let query = r#"mutation($schema:String!){
        updateRecord(key:{id:"1"},input:{score:"8"},expected:{score:{eq:"7"}},
            expectedSchema:$schema){__typename}
    }"#;
    let (a, b) = tokio::join!(
        api.execute(request(&f, query)),
        api.execute(request(&f, query))
    );
    assert_eq!(
        usize::from(a.errors.is_empty()) + usize::from(b.errors.is_empty()),
        1
    );
    // No blind retry is performed by the adapter.
    f.finish().await;
}
#[tokio::test]
async fn composite_required_keys_nullability_and_policy_revision_fail_closed() {
    let mut definition = definition();
    definition.collections[0]
        .primary_key
        .push(FieldId::new("tenant").unwrap());
    let f = Fixture::with_definition(definition).await;
    let api = api(&f);
    let mut old = f.authority();
    old.expected_policy_revision = 0;
    let response = api
        .execute(request(&f, CREATE).data(context(&f).with_write_authority(Arc::new(old))))
        .await;
    assert!(!response.errors.is_empty());
    assert_eq!(f.count().await, 0);
    assert!(api.execute(request(&f, CREATE)).await.errors.is_empty());
    for input in ["{label:null}", "{id:\"2\"}"] {
        let response = api
            .execute(request(
                &f,
                &format!(
                    r#"mutation($schema:String!){{
            updateRecord(key:{{id:"1",tenant:"north"}},input:{input},
                expectedSchema:$schema){{__typename}}
        }}"#
                ),
            ))
            .await;
        assert!(!response.errors.is_empty());
    }
    let response = api
        .execute(request(
            &f,
            r#"mutation($schema:String!){
        updateRecord(key:{id:"1"},input:{label:"changed"},expectedSchema:$schema){__typename}
    }"#,
        ))
        .await;
    assert!(!response.errors.is_empty());
    for input in ["{optional:\"value\"}", "{score:\"9\"}", "{optional:null}"] {
        let response = api
            .execute(request(
                &f,
                &format!(
                    r#"mutation($schema:String!){{
            updateRecord(key:{{id:"1",tenant:"north"}},input:{input},
                expectedSchema:$schema){{__typename}}
        }}"#
                ),
            ))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
    }
    f.finish().await;
}

struct OutputRead {
    db: Database<Backend>,
    mode: u8,
    delete: bool,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl RuntimeReadAuthority for OutputRead {
    fn authorize<'a>(
        &'a self,
        check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Read.authorize(check)
    }
    fn authorize_mutation_result<'a>(
        &'a self,
        check: RuntimeMutationOutputCheck<'a>,
    ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                Journal::count_all(&self.db).await.unwrap(),
                1,
                "callback is after journal commit"
            );
            assert_eq!(check.collection.id().as_str(), "records");
            assert_eq!(
                check.kind == RuntimeMutationRecordKind::DeletedPreimage,
                self.delete
            );
            assert_eq!(check.action == RuntimeMutationAction::Delete, self.delete);
            assert_eq!(check.projection.fields().len(), 1);
            let field = check
                .schema
                .resolve_field(check.collection, &FieldId::new("label").unwrap())
                .unwrap();
            assert_eq!(
                check.record.value(&field).unwrap(),
                &RuntimeValue::String("initial".into())
            );
            for hidden in ["id", "tenant", "score"] {
                let field = check
                    .schema
                    .resolve_field(check.collection, &FieldId::new(hidden).unwrap())
                    .unwrap();
                assert!(matches!(
                    check.record.state(&field).unwrap(),
                    RuntimeFieldState::Unloaded
                ));
            }
            match self.mode {
                0 => Ok(()),
                1 => Err(RuntimeGraphqlError::denied()),
                2 => Err(RuntimeGraphqlError::unavailable()),
                3 => std::future::pending().await,
                _ => unreachable!(),
            }
        })
    }
}
#[tokio::test]
async fn post_commit_allow_denial_error_timeout_and_deleted_preimages() {
    for delete in [false, true] {
        // 4 selects the unmodified, default-denying C-only authority.
        for mode in 0..6 {
            let f = Fixture::new().await;
            let mut options = RuntimeGraphqlMutationOptions::default();
            options.output_authorization_timeout = if mode == 5 {
                Duration::from_secs(10)
            } else {
                Duration::from_millis(30)
            };
            let module = RuntimeGraphqlModule::compile(
                Arc::new(f.schema().clone()),
                RuntimeGraphqlOptions::default()
                    .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                    .with_mutations(options),
            )
            .unwrap();
            let api = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
                .unwrap()
                .mutation_root("Mutation")
                .unwrap()
                .install(module)
                .unwrap()
                .finish()
                .unwrap();
            if delete {
                f.run(f.create(1), f.authority()).await.unwrap();
            }
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let read: Arc<dyn RuntimeReadAuthority> = if mode == 4 {
                Arc::new(Read)
            } else {
                Arc::new(OutputRead {
                    db: f.db.clone(),
                    mode: if mode == 5 { 3 } else { mode },
                    delete,
                    calls: calls.clone(),
                })
            };
            let context = RuntimeGraphqlRequest::new(
                f.schema().fingerprint(),
                read,
                None,
                Default::default(),
            )
            .with_mutation_environment(f.environment.clone())
            .with_write_authority(Arc::new(f.authority()))
            .with_mutation_hook(Arc::new(JournalHook {
                staged: Arc::new(AtomicBool::new(false)),
                pending: false,
                fail: false,
            }));
            let query = if delete {
                r#"mutation($schema:String!){deleteRecord(key:{id:"1"},expectedSchema:$schema){copy:record{caption:label}}}"#
            } else {
                r#"mutation($schema:String!){createRecord(input:{id:"1",tenant:"north",label:"initial"},expectedSchema:$schema){copy:record{caption:label}}}"#
            };
            if mode == 5 {
                assert!(
                    tokio::time::timeout(
                        Duration::from_millis(150),
                        api.execute(request(&f, query).data(context))
                    )
                    .await
                    .is_err()
                );
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                assert_eq!(f.count().await, if delete { 0 } else { 1 });
                assert_eq!(Journal::count_all(&f.db).await.unwrap(), 1);
                f.finish().await;
                continue;
            }
            let response = api.execute(request(&f, query).data(context)).await;
            assert_eq!(f.count().await, if delete { 0 } else { 1 });
            assert_eq!(Journal::count_all(&f.db).await.unwrap(), 1);
            if mode == 0 {
                assert!(response.errors.is_empty(), "{:?}", response.errors);
                assert_eq!(
                    response.data.into_json().unwrap()[if delete {
                        "deleteRecord"
                    } else {
                        "createRecord"
                    }]["copy"]["caption"],
                    "initial"
                );
            } else {
                assert_eq!(response.errors.len(), 1);
                let error = &response.errors[0];
                let extensions = error.extensions.as_ref().unwrap();
                assert_eq!(
                    extensions.get("outcome"),
                    Some(&graphql_orm::async_graphql::Value::String(
                        "committed".into()
                    ))
                );
                assert_eq!(
                    extensions.get("retryable"),
                    Some(&graphql_orm::async_graphql::Value::Boolean(false))
                );
                assert_eq!(
                    error.message,
                    match mode {
                        1 | 4 => "mutation_payload_denied",
                        2 => "mutation_payload_failed",
                        3 => "mutation_payload_timeout",
                        _ => unreachable!(),
                    }
                );
                assert!(!format!("{response:?}").contains("initial"));
            }
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(mode != 4));
            f.finish().await;
        }
    }
}

struct DenyProjection;
impl RuntimeReadAuthority for DenyProjection {
    fn authorize<'a>(
        &'a self,
        _: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Box::pin(async { Err(RuntimeGraphqlError::denied()) })
    }
}
#[tokio::test]
async fn precommit_projection_and_result_checks_remain_required() {
    let f = Fixture::new().await;
    let api = api(&f);
    let query = r#"mutation($schema:String!){createRecord(input:{id:"1",tenant:"north",label:"initial"},expectedSchema:$schema){record{label}}}"#;
    let context = RuntimeGraphqlRequest::new(
        f.schema().fingerprint(),
        Arc::new(DenyProjection),
        None,
        Default::default(),
    )
    .with_mutation_environment(f.environment.clone())
    .with_write_authority(Arc::new(f.authority()));
    assert!(
        !api.execute(request(&f, query).data(context))
            .await
            .errors
            .is_empty()
    );
    assert_eq!(f.count().await, 0);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let read = Arc::new(OutputRead {
        db: f.db.clone(),
        mode: 0,
        delete: false,
        calls: calls.clone(),
    });
    let mut write = f.authority();
    write.deny_result = true;
    let context =
        RuntimeGraphqlRequest::new(f.schema().fingerprint(), read, None, Default::default())
            .with_mutation_environment(f.environment.clone())
            .with_write_authority(Arc::new(write));
    assert!(
        !api.execute(request(&f, query).data(context))
            .await
            .errors
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.count().await, 0);
    assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
    f.finish().await;
}

#[derive(Default)]
struct GroupProbe {
    intents: std::sync::atomic::AtomicUsize,
    projections: std::sync::Mutex<Vec<Vec<String>>>,
    released: std::sync::Mutex<Vec<(RuntimeMutationAction, String)>>,
    hooks: std::sync::Mutex<Vec<RuntimeMutationAction>>,
}
struct GroupRead {
    probe: Arc<GroupProbe>,
    // 0 allow, 1 deny, 2 error, 3 await cancellation/timeout, 4 deny score selection.
    mode: u8,
}
impl RuntimeReadAuthority for GroupRead {
    fn authorize<'a>(
        &'a self,
        check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Box::pin(async move {
            self.probe.projections.lock().unwrap().push(
                check
                    .projection
                    .iter()
                    .map(|f| f.id().as_str().to_owned())
                    .collect(),
            );
            if self.mode == 4 && check.projection.iter().any(|f| f.id().as_str() == "score") {
                return Err(RuntimeGraphqlError::denied());
            }
            Ok(RuntimeReadGrant::new(None))
        })
    }
    fn authorize_mutation_result<'a>(
        &'a self,
        check: RuntimeMutationOutputCheck<'a>,
    ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
        Box::pin(async move {
            let label = check
                .schema
                .resolve_field(check.collection, &FieldId::new("label").unwrap())
                .unwrap();
            let RuntimeFieldState::Value(RuntimeValue::String(label)) =
                check.record.state(&label).unwrap()
            else {
                panic!("merged label projection must be present");
            };
            assert_eq!(
                check.kind,
                if check.action == RuntimeMutationAction::Delete {
                    RuntimeMutationRecordKind::DeletedPreimage
                } else {
                    RuntimeMutationRecordKind::CommittedResult
                }
            );
            self.probe
                .released
                .lock()
                .unwrap()
                .push((check.action, label.clone()));
            match self.mode {
                1 => Err(RuntimeGraphqlError::denied()),
                2 => Err(RuntimeGraphqlError::unavailable()),
                3 => std::future::pending().await,
                _ => Ok(()),
            }
        })
    }
}
struct GroupWrite {
    probe: Arc<GroupProbe>,
    inner: Authority,
}
impl RuntimeWriteAuthority<Backend> for GroupWrite {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
            self.probe.intents.fetch_add(1, Ordering::SeqCst);
            self.inner.authorize_intent(check, tx).await
        })
    }
    fn authorize_preimage<'a>(
        &'a self,
        check: RuntimePreimageCheck<'a>,
        tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        self.inner.authorize_preimage(check, tx)
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        self.inner.authorize_result(check, tx)
    }
}
struct GroupJournal {
    probe: Arc<GroupProbe>,
    // 0 commit, 1 reject, 2 await cancellation.
    mode: u8,
}
impl RuntimeMutationHook<Backend> for GroupJournal {
    fn before_commit<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, Backend>,
        pending: &'a RuntimeMutationEffect,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async move {
            let id = {
                let mut hooks = self.probe.hooks.lock().unwrap();
                hooks.push(pending.action());
                hooks.len().to_string()
            };
            tx.insert::<Journal>(CreateJournalInput {
                id,
                message: "group".into(),
            })
            .await
            .map_err(|e| {
                RuntimeMutationError::new(RuntimeMutationErrorCode::HookFailed).with_source(e)
            })?;
            match self.mode {
                1 => Err(RuntimeMutationError::new(
                    RuntimeMutationErrorCode::HookFailed,
                )),
                2 => std::future::pending().await,
                _ => Ok(()),
            }
        })
    }
}
fn group_context(
    f: &Fixture,
    probe: &Arc<GroupProbe>,
    output: u8,
    hook: u8,
    budget: RuntimeGraphqlLimits,
) -> RuntimeGraphqlRequest<Backend> {
    RuntimeGraphqlRequest::new(
        f.schema().fingerprint(),
        Arc::new(GroupRead {
            probe: probe.clone(),
            mode: output,
        }),
        None,
        budget,
    )
    .with_mutation_environment(f.environment.clone())
    .with_write_authority(Arc::new(GroupWrite {
        probe: probe.clone(),
        inner: f.authority(),
    }))
    .with_mutation_hook(Arc::new(GroupJournal {
        probe: probe.clone(),
        mode: hook,
    }))
}
fn repeated(field: &str) -> String {
    format!(
        r#"mutation($schema:String!){{
        same:{field}{{record{{label}}}}
        same:{field}{{record{{score}}}}
        ...Again
    }} fragment Again on Mutation{{same:{field}{{record{{renamed:label score}}}}}}"#
    )
}
const GROUP_CREATE: &str =
    r#"createRecord(input:{id:"1",tenant:"north",label:"initial"},expectedSchema:$schema)"#;
const GROUP_UPDATE: &str = r#"updateRecord(key:{id:"1"},input:{label:"updated"},expected:{label:{eq:"initial"}},expectedSchema:$schema)"#;
const GROUP_DELETE: &str =
    r#"deleteRecord(key:{id:"1"},expected:{label:{eq:"initial"}},expectedSchema:$schema)"#;

#[tokio::test]
async fn repeated_mutation_groups_merge_projections_and_run_once_with_tight_budgets() {
    for field in [GROUP_CREATE, GROUP_UPDATE, GROUP_DELETE] {
        let f = Fixture::new().await;
        if field != GROUP_CREATE {
            f.run(f.create(1), f.authority()).await.unwrap();
        }
        for budget in [
            RuntimeGraphqlLimits {
                max_statements: 3,
                ..Default::default()
            },
            RuntimeGraphqlLimits {
                max_materialized_nodes: 0,
                ..Default::default()
            },
        ] {
            let probe = Arc::new(GroupProbe::default());
            let denied = api(&f)
                .execute(
                    request(&f, &repeated(field)).data(group_context(&f, &probe, 0, 0, budget)),
                )
                .await;
            assert_eq!(denied.errors[0].message, "cost_exceeded");
            assert_eq!(probe.intents.load(Ordering::SeqCst), 0);
            assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
        }
        let probe = Arc::new(GroupProbe::default());
        // One merged payload fits; separately charging its repeated occurrences does not.
        let budget = RuntimeGraphqlLimits {
            max_statements: 4,
            max_materialized_nodes: 1,
            max_response_bytes: 400,
            ..Default::default()
        };
        let response = api(&f)
            .execute(request(&f, &repeated(field)).data(group_context(&f, &probe, 0, 0, budget)))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        let label = if field == GROUP_UPDATE {
            "updated"
        } else {
            "initial"
        };
        assert_eq!(
            data["same"]["record"],
            serde_json::json!({"label":label,"score":"7","renamed":label})
        );
        assert_eq!(probe.intents.load(Ordering::SeqCst), 1);
        assert_eq!(probe.hooks.lock().unwrap().len(), 1);
        assert_eq!(probe.released.lock().unwrap().len(), 1);
        assert_eq!(
            *probe.projections.lock().unwrap(),
            vec![vec!["label".to_owned(), "score".to_owned()]]
        );
        assert_eq!(Journal::count_all(&f.db).await.unwrap(), 1);
        assert_eq!(f.count().await, if field == GROUP_DELETE { 0 } else { 1 });
        f.finish().await;
    }
}

#[tokio::test]
async fn distinct_aliases_keep_document_order_and_reserve_separate_transactions() {
    let f = Fixture::new().await;
    f.run(f.create(1), f.authority()).await.unwrap();
    let query = r#"mutation($schema:String!){
        z:updateRecord(key:{id:"1"},input:{label:"first"},expected:{label:{eq:"initial"}},expectedSchema:$schema){record{label}}
        a:updateRecord(key:{id:"1"},input:{label:"second"},expected:{label:{eq:"first"}},expectedSchema:$schema){record{label}}
        ...Again
    } fragment Again on Mutation {
        z:updateRecord(key:{id:"1"},input:{label:"first"},expected:{label:{eq:"initial"}},expectedSchema:$schema){record{score}}
    }"#;
    for statements in [4, 7] {
        let probe = Arc::new(GroupProbe::default());
        let budget = RuntimeGraphqlLimits {
            max_statements: statements,
            ..Default::default()
        };
        let denied = api(&f)
            .execute(request(&f, query).data(group_context(&f, &probe, 0, 0, budget)))
            .await;
        assert_eq!(denied.errors[0].message, "cost_exceeded");
        assert_eq!(probe.intents.load(Ordering::SeqCst), 0);
        assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
    }
    let probe = Arc::new(GroupProbe::default());
    let budget = RuntimeGraphqlLimits {
        max_statements: 8,
        max_materialized_nodes: 2,
        ..Default::default()
    };
    let response = api(&f)
        .execute(request(&f, query).data(group_context(&f, &probe, 0, 0, budget)))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(
        data["z"]["record"],
        serde_json::json!({"label":"first","score":"7"})
    );
    assert_eq!(data["a"]["record"]["label"], "second");
    assert_eq!(
        *probe.released.lock().unwrap(),
        vec![
            (RuntimeMutationAction::Update, "first".into()),
            (RuntimeMutationAction::Update, "second".into())
        ]
    );
    assert_eq!(probe.intents.load(Ordering::SeqCst), 2);
    assert_eq!(probe.hooks.lock().unwrap().len(), 2);
    assert_eq!(Journal::count_all(&f.db).await.unwrap(), 2);
    f.finish().await;
}

struct CancelFirstRoot;
struct CancelRootExtension(AtomicBool);
impl graphql_orm::async_graphql::extensions::ExtensionFactory for CancelFirstRoot {
    fn create(&self) -> Arc<dyn graphql_orm::async_graphql::extensions::Extension> {
        Arc::new(CancelRootExtension(AtomicBool::new(false)))
    }
}
#[async_trait::async_trait]
impl graphql_orm::async_graphql::extensions::Extension for CancelRootExtension {
    async fn resolve(
        &self,
        ctx: &graphql_orm::async_graphql::extensions::ExtensionContext<'_>,
        info: graphql_orm::async_graphql::extensions::ResolveInfo<'_>,
        next: graphql_orm::async_graphql::extensions::NextResolve<'_>,
    ) -> graphql_orm::async_graphql::ServerResult<Option<graphql_orm::async_graphql::Value>> {
        if info.parent_type == "Mutation" && !self.0.swap(true, Ordering::SeqCst) {
            tokio::time::timeout(Duration::from_millis(500), next.run(ctx, info))
                .await
                .unwrap_or_else(|_| {
                    Err(graphql_orm::async_graphql::ServerError::new(
                        "host_timeout",
                        None,
                    ))
                })
        } else {
            next.run(ctx, info).await
        }
    }
}
fn group_api(f: &Fixture, cancel: bool, timeout: Duration) -> Schema {
    let mut options = RuntimeGraphqlMutationOptions::default();
    options.output_authorization_timeout = timeout;
    let module = RuntimeGraphqlModule::compile(
        Arc::new(f.schema().clone()),
        RuntimeGraphqlOptions::default()
            .with_cursor_profile(RuntimeCursorProfile::Unprotected)
            .with_mutations(options),
    )
    .unwrap();
    let composer = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
        .unwrap()
        .mutation_root("Mutation")
        .unwrap()
        .install(module)
        .unwrap();
    if cancel {
        composer.extension(CancelFirstRoot).finish().unwrap()
    } else {
        composer.finish().unwrap()
    }
}
#[tokio::test]
async fn repeated_groups_cache_rejections_and_post_commit_denial_error_timeout() {
    for field in [GROUP_CREATE, GROUP_UPDATE, GROUP_DELETE] {
        for (output, hook, code, committed) in [
            (0, 1, "hook_failed", false),
            (1, 0, "mutation_payload_denied", true),
            (2, 0, "mutation_payload_failed", true),
            (3, 0, "mutation_payload_timeout", true),
        ] {
            let f = Fixture::new().await;
            if field != GROUP_CREATE {
                f.run(f.create(1), f.authority()).await.unwrap();
            }
            let probe = Arc::new(GroupProbe::default());
            let response = group_api(&f, false, Duration::from_millis(25))
                .execute(request(&f, &repeated(field)).data(group_context(
                    &f,
                    &probe,
                    output,
                    hook,
                    Default::default(),
                )))
                .await;
            assert_eq!(response.errors.len(), 3, "{:?}", response.errors);
            for error in &response.errors {
                assert_eq!(error.message, code);
                if committed {
                    assert_eq!(
                        error.extensions.as_ref().unwrap().get("outcome").unwrap(),
                        &graphql_orm::async_graphql::Value::String("committed".into())
                    );
                    assert_eq!(
                        error.extensions.as_ref().unwrap().get("retryable").unwrap(),
                        &graphql_orm::async_graphql::Value::Boolean(false)
                    );
                }
            }
            let wire = serde_json::to_string(&response).unwrap();
            assert!(!wire.contains("updated") && !wire.contains("initial"));
            assert_eq!(probe.intents.load(Ordering::SeqCst), 1);
            assert_eq!(probe.hooks.lock().unwrap().len(), 1);
            assert_eq!(probe.released.lock().unwrap().len(), usize::from(committed));
            assert_eq!(
                Journal::count_all(&f.db).await.unwrap(),
                i64::from(committed)
            );
            // A fresh request is a fresh group, and sees the actual committed state.
            let next = api(&f)
                .execute(request(
                    &f,
                    &format!("mutation($schema:String!){{{field}{{__typename}}}}"),
                ))
                .await;
            assert_eq!(next.errors.is_empty(), !committed);
            f.finish().await;
        }
    }
    for denial in [false, true] {
        let f = Fixture::new().await;
        f.run(f.create(1), f.authority()).await.unwrap();
        let probe = Arc::new(GroupProbe::default());
        let mut authority = f.authority();
        authority.deny_preimage = denial;
        let preimages = authority.preimage_calls.clone();
        let field = if denial {
            GROUP_UPDATE.to_owned()
        } else {
            GROUP_UPDATE.replace("eq:\"initial\"", "eq:\"stale\"")
        };
        let context = group_context(&f, &probe, 0, 0, Default::default()).with_write_authority(
            Arc::new(GroupWrite {
                probe: probe.clone(),
                inner: authority,
            }),
        );
        let response = api(&f)
            .execute(request(&f, &repeated(&field)).data(context))
            .await;
        assert_eq!(response.errors.len(), 3);
        assert!(
            response
                .errors
                .iter()
                .all(|error| error.message == response.errors[0].message)
        );
        assert_eq!(probe.intents.load(Ordering::SeqCst), 1);
        assert_eq!(preimages.load(Ordering::SeqCst), 1);
        assert!(probe.hooks.lock().unwrap().is_empty());
        assert!(probe.released.lock().unwrap().is_empty());
        assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
        f.finish().await;
    }
}
#[tokio::test]
async fn canceled_occurrence_is_terminal_while_the_executor_continues_repeats() {
    for field in [GROUP_CREATE, GROUP_UPDATE, GROUP_DELETE] {
        for committed in [false, true] {
            let f = Fixture::new().await;
            if field != GROUP_CREATE {
                f.run(f.create(1), f.authority()).await.unwrap();
            }
            let probe = Arc::new(GroupProbe::default());
            let response = group_api(&f, true, Duration::from_secs(10))
                .execute(request(&f, &repeated(field)).data(group_context(
                    &f,
                    &probe,
                    if committed { 3 } else { 0 },
                    if committed { 0 } else { 2 },
                    Default::default(),
                )))
                .await;
            assert_eq!(response.errors.len(), 3, "{:?}", response.errors);
            assert_eq!(response.errors[0].message, "host_timeout");
            for error in &response.errors[1..] {
                assert_eq!(
                    error.message,
                    if committed {
                        "mutation_payload_canceled"
                    } else {
                        "mutation_outcome_unknown"
                    }
                );
                let extensions = error.extensions.as_ref().unwrap();
                assert_eq!(
                    extensions.get("outcome").unwrap(),
                    &graphql_orm::async_graphql::Value::String(
                        if committed { "committed" } else { "unknown" }.into()
                    )
                );
                assert_eq!(
                    extensions.get("retryable").unwrap(),
                    &graphql_orm::async_graphql::Value::Boolean(false)
                );
            }
            assert_eq!(probe.intents.load(Ordering::SeqCst), 1);
            assert_eq!(probe.hooks.lock().unwrap().len(), 1);
            assert_eq!(probe.released.lock().unwrap().len(), usize::from(committed));
            assert_eq!(
                Journal::count_all(&f.db).await.unwrap(),
                i64::from(committed)
            );
            let present = if field == GROUP_CREATE {
                committed
            } else if field == GROUP_DELETE {
                !committed
            } else {
                true
            };
            assert_eq!(f.count().await, i64::from(present));
            assert!(
                !serde_json::to_string(&response)
                    .unwrap()
                    .contains("initial")
            );
            f.finish().await;
        }
    }
}

#[tokio::test]
async fn repeated_groups_keep_validation_directives_and_projection_denial_before_writes() {
    let f = Fixture::new().await;
    let schema = api(&f);
    let probe = Arc::new(GroupProbe::default());
    let invalid = repeated(GROUP_CREATE).replacen("id:\"1\"", "id:\"2\"", 1);
    let response = schema
        .execute(request(&f, &invalid).data(group_context(&f, &probe, 0, 0, Default::default())))
        .await;
    assert!(!response.errors.is_empty());
    assert_eq!(probe.intents.load(Ordering::SeqCst), 0);
    // A later fragment's selected field is authorized before any occurrence writes.
    let denied = schema
        .execute(request(&f, &repeated(GROUP_CREATE)).data(group_context(
            &f,
            &probe,
            4,
            0,
            Default::default(),
        )))
        .await;
    assert_eq!(denied.errors[0].message, "authorization_denied");
    assert_eq!(probe.intents.load(Ordering::SeqCst), 0);
    assert_eq!(f.count().await, 0);
    assert_eq!(Journal::count_all(&f.db).await.unwrap(), 0);
    let query = format!(
        r#"mutation($schema:String!, $skip:Boolean = true){{
        same:{GROUP_CREATE} @skip(if:$skip){{record{{label}}}}
        ... on Mutation {{same:{GROUP_CREATE}{{__typename}}}}
        ...Again @include(if:true)
    }} fragment Again on Mutation {{same:{GROUP_CREATE}{{__typename}}}}"#
    );
    let probe = Arc::new(GroupProbe::default());
    let budget = RuntimeGraphqlLimits {
        max_statements: 4,
        max_materialized_nodes: 1,
        ..Default::default()
    };
    let response = schema
        .execute(request(&f, &query).data(group_context(&f, &probe, 1, 0, budget)))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["same"]["__typename"],
        "RecordMutationPayload"
    );
    assert_eq!(probe.intents.load(Ordering::SeqCst), 1);
    assert_eq!(probe.hooks.lock().unwrap().len(), 1);
    assert!(probe.projections.lock().unwrap().is_empty());
    assert!(probe.released.lock().unwrap().is_empty());
    assert_eq!(Journal::count_all(&f.db).await.unwrap(), 1);
    assert_eq!(f.count().await, 1);
    f.finish().await;
}
