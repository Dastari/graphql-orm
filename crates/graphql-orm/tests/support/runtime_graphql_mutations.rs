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
