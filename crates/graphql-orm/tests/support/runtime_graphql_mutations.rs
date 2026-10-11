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
