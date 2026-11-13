// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! `#[graphql_orm_custom_operations]` enforces the authorization it declares
//! before the resolver body runs (#449). A described root is either explicitly
//! public or carries an enforced requirement; the catalogue never reports a
//! policy that nothing enforces.

use graphql_orm::async_graphql::{Context, Guard, Request, Response, SimpleObject};
use graphql_orm::futures::StreamExt;
use graphql_orm::graphql::errors::OrmPublicError;
use graphql_orm::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts resolver bodies and host guards that ran.
#[derive(Clone, Default)]
struct Calls {
    resolvers: Arc<AtomicUsize>,
    host_guards: Arc<AtomicUsize>,
}
fn ran(ctx: &Context<'_>) {
    ctx.data_unchecked::<Calls>()
        .resolvers
        .fetch_add(1, Ordering::SeqCst);
}

/// Authority a host can revoke while a request or stream is in flight.
#[derive(Clone, Default)]
struct LiveAuthority(Arc<std::sync::atomic::AtomicBool>);

/// A host guard that asks the live authority on every evaluation.
struct LiveAuthorityGuard;
impl Guard for LiveAuthorityGuard {
    async fn check(&self, ctx: &Context<'_>) -> async_graphql::Result<()> {
        if ctx.data::<LiveAuthority>()?.0.load(Ordering::SeqCst) {
            Err(OrmPublicError::forbidden().into_graphql_error())
        } else {
            Ok(())
        }
    }
}

struct HostGuard;
impl Guard for HostGuard {
    async fn check(&self, ctx: &Context<'_>) -> async_graphql::Result<()> {
        ctx.data_unchecked::<Calls>()
            .host_guards
            .fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "declared_things",
    plural = "DeclaredThings",
    auth = "none"
)]
struct DeclaredThing {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

/// A handwritten entity served below `_entities`.
#[derive(SimpleObject, GraphQLSemanticObject)]
#[graphql_orm(classification = "internal")]
struct DeclaredWidget {
    /// Public widget identity.
    id: String,
}

#[derive(Default)]
struct ProtectedQueries;

// No `authorization` argument: a described root is protected by default.
#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl ProtectedQueries {
    /// Returns a protected status.
    async fn protected_status(&self, ctx: &Context<'_>) -> String {
        ran(ctx);
        "protected".to_owned()
    }

    /// Returns a status behind the host guard as well.
    #[graphql(guard = "HostGuard")]
    async fn guarded_status(&self, ctx: &Context<'_>) -> String {
        ran(ctx);
        "guarded".to_owned()
    }

    /// Returns a status this resolver declares public.
    #[graphql_orm(authorization = false)]
    async fn open_status(&self, ctx: &Context<'_>) -> String {
        ran(ctx);
        "open".to_owned()
    }

    /// Returns a report that needs both scopes.
    #[graphql_orm(all_scopes = ["reports.read", "reports.export"])]
    async fn exported_report(&self, ctx: &Context<'_>) -> String {
        ran(ctx);
        "exported".to_owned()
    }

    /// Returns a report that needs either scope alternative.
    #[graphql_orm(any_scopes = [["reports.admin"], ["reports.read"]])]
    async fn any_report(&self, ctx: &Context<'_>) -> String {
        ran(ctx);
        "any".to_owned()
    }

    /// Resolves a handwritten Federation entity.
    #[graphql(entity)]
    async fn find_declared_widget_by_id(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<DeclaredWidget> {
        ran(ctx);
        Ok(DeclaredWidget { id })
    }
}

#[derive(Default)]
struct PublicQueries;

#[graphql_orm_custom_operations(kind = "query", authorization = false)]
#[graphql_orm::async_graphql::Object]
impl PublicQueries {
    /// Returns a public status.
    async fn public_status(&self, ctx: &Context<'_>) -> String {
        ran(ctx);
        "public".to_owned()
    }
}

#[derive(Default)]
struct ScopedMutations;

#[graphql_orm_custom_operations(kind = "mutation", all_scopes = ["notes.write"])]
#[graphql_orm::async_graphql::Object]
impl ScopedMutations {
    /// Writes one note.
    async fn write_note(&self, ctx: &Context<'_>, body: String) -> bool {
        ran(ctx);
        !body.is_empty()
    }

    /// Restating the requirement keeps the root's scopes.
    #[graphql_orm(authorization = true)]
    async fn annotate_note(&self, ctx: &Context<'_>) -> bool {
        ran(ctx);
        true
    }

    /// Resolver scopes are added to the root's.
    #[graphql_orm(all_scopes = ["notes.audit"])]
    async fn audit_note(&self, ctx: &Context<'_>) -> bool {
        ran(ctx);
        true
    }

    /// An explicit replacement states the resolver's whole requirement.
    #[graphql_orm(authorization(replace, any_scopes = [["notes.read"], ["notes.admin"]]))]
    async fn peek_note(&self, ctx: &Context<'_>) -> bool {
        ran(ctx);
        true
    }

    /// An explicit replacement with authentication alone.
    #[graphql_orm(authorization(replace))]
    async fn ping_notes(&self, ctx: &Context<'_>) -> bool {
        ran(ctx);
        true
    }
}

#[derive(Default)]
struct ProtectedEvents;

/// The source of `liveTicks`, handed to the first subscriber.
type TickSource =
    std::sync::Mutex<Option<graphql_orm::futures::channel::mpsc::UnboundedReceiver<i32>>>;

#[graphql_orm_custom_operations(kind = "subscription", authorization = true)]
#[graphql_orm::async_graphql::Subscription]
impl ProtectedEvents {
    /// Observes protected ticks.
    async fn protected_ticks(
        &self,
        ctx: &Context<'_>,
    ) -> impl graphql_orm::futures::Stream<Item = i32> {
        ran(ctx);
        graphql_orm::futures::stream::iter([1])
    }

    /// Observes ticks behind a host guard that consults live authority.
    #[graphql(guard = "LiveAuthorityGuard")]
    async fn guarded_ticks(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<impl graphql_orm::futures::Stream<Item = i32>> {
        ctx.data::<TickSource>()?
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| async_graphql::Error::new("tick source already taken"))
    }

    /// Observes ticks without a context argument of its own.
    async fn plain_ticks(&self, first: i32) -> impl graphql_orm::futures::Stream<Item = i32> {
        graphql_orm::futures::stream::iter([first, first + 1])
    }

    /// Observes live ticks from a fallible resolver.
    async fn live_ticks(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<impl graphql_orm::futures::Stream<Item = i32>> {
        ran(ctx);
        ctx.data::<TickSource>()?
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| async_graphql::Error::new("tick source already taken"))
    }
}

// The schema default is `none`: a declared requirement does not follow it.
schema_roots! {
    backend: "sqlite",
    auth: "none",
    entities: [DeclaredThing],
    described_query_types: [ProtectedQueries, PublicQueries],
    described_mutation_types: [ScopedMutations],
    described_subscription_types: [ProtectedEvents],
}

async fn schema() -> (AppSchema, Calls) {
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let calls = Calls::default();
    (schema_builder(db).data(calls.clone()).finish(), calls)
}

fn subject(scopes: &[&str]) -> AuthSubject {
    AuthSubject::from_parts(
        "fixture-principal",
        Vec::new(),
        scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        None,
    )
}

fn code(response: &Response) -> Option<String> {
    let error = response.errors.first()?;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    Some(
        error
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(|code| code.to_string().trim_matches('"').to_owned())
            .unwrap_or_default(),
    )
}

/// Run `query` with the given subject and report its error code and whether
/// the resolver body ran.
async fn outcome(query: &str, scopes: Option<&[&str]>) -> (Option<String>, usize, usize) {
    let (schema, calls) = schema().await;
    let mut request = Request::new(query);
    if let Some(scopes) = scopes {
        request = request.data(subject(scopes));
    }
    let response = schema.execute(request).await;
    (
        code(&response),
        calls.resolvers.load(Ordering::SeqCst),
        calls.host_guards.load(Ordering::SeqCst),
    )
}

fn refused(code: &str) -> (Option<String>, usize, usize) {
    (Some(code.to_owned()), 0, 0)
}

#[tokio::test]
async fn a_described_root_requires_an_authenticated_subject_before_its_body_runs() {
    assert_eq!(
        outcome("{ protectedStatus }", None).await,
        refused("UNAUTHENTICATED")
    );
    assert_eq!(
        outcome("{ protectedStatus }", Some(&[])).await,
        (None, 1, 0)
    );
}

#[tokio::test]
async fn declared_authorization_runs_before_the_host_guard() {
    assert_eq!(
        outcome("{ guardedStatus }", None).await,
        refused("UNAUTHENTICATED")
    );
    assert_eq!(outcome("{ guardedStatus }", Some(&[])).await, (None, 1, 1));
}

#[tokio::test]
async fn explicitly_public_roots_and_resolvers_run_without_a_subject() {
    assert_eq!(outcome("{ openStatus }", None).await, (None, 1, 0));
    assert_eq!(outcome("{ publicStatus }", None).await, (None, 1, 0));
}

#[tokio::test]
async fn declared_scopes_are_enforced_with_the_standard_codes() {
    let all = "{ exportedReport }";
    assert_eq!(outcome(all, None).await, refused("UNAUTHENTICATED"));
    assert_eq!(outcome(all, Some(&[])).await, refused("FORBIDDEN"));
    assert_eq!(
        outcome(all, Some(&["reports.read"])).await,
        refused("FORBIDDEN")
    );
    assert_eq!(
        outcome(all, Some(&["reports.read", "reports.export"])).await,
        (None, 1, 0)
    );

    let any = "{ anyReport }";
    assert_eq!(outcome(any, None).await, refused("UNAUTHENTICATED"));
    assert_eq!(
        outcome(any, Some(&["reports.export"])).await,
        refused("FORBIDDEN")
    );
    assert_eq!(outcome(any, Some(&["reports.read"])).await, (None, 1, 0));
    assert_eq!(outcome(any, Some(&["reports.admin"])).await, (None, 1, 0));
}

#[tokio::test]
async fn root_level_scopes_guard_mutations() {
    let mutation = "mutation { writeNote(body: \"text\") }";
    assert_eq!(outcome(mutation, None).await, refused("UNAUTHENTICATED"));
    assert_eq!(
        outcome(mutation, Some(&["notes.read"])).await,
        refused("FORBIDDEN")
    );
    assert_eq!(
        outcome(mutation, Some(&["notes.write"])).await,
        (None, 1, 0)
    );
}

#[tokio::test]
async fn a_resolver_declaration_narrows_its_root_unless_it_says_it_replaces_it() {
    // `authorization = true` on a resolver keeps the root's scopes.
    let annotate = "mutation { annotateNote }";
    assert_eq!(outcome(annotate, None).await, refused("UNAUTHENTICATED"));
    assert_eq!(outcome(annotate, Some(&[])).await, refused("FORBIDDEN"));
    assert_eq!(
        outcome(annotate, Some(&["notes.write"])).await,
        (None, 1, 0)
    );

    // Resolver scopes are required in addition to the root's.
    let audit = "mutation { auditNote }";
    assert_eq!(
        outcome(audit, Some(&["notes.audit"])).await,
        refused("FORBIDDEN")
    );
    assert_eq!(
        outcome(audit, Some(&["notes.write"])).await,
        refused("FORBIDDEN")
    );
    assert_eq!(
        outcome(audit, Some(&["notes.write", "notes.audit"])).await,
        (None, 1, 0)
    );

    // Only `authorization(replace, ...)` carries a different requirement.
    let peek = "mutation { peekNote }";
    assert_eq!(outcome(peek, None).await, refused("UNAUTHENTICATED"));
    assert_eq!(
        outcome(peek, Some(&["notes.write"])).await,
        refused("FORBIDDEN")
    );
    assert_eq!(outcome(peek, Some(&["notes.read"])).await, (None, 1, 0));
    let ping = "mutation { pingNotes }";
    assert_eq!(outcome(ping, None).await, refused("UNAUTHENTICATED"));
    assert_eq!(outcome(ping, Some(&[])).await, (None, 1, 0));
}

/// Assurance evaluator a test can turn from admitting to refusing.
#[derive(Debug, Default)]
struct SwitchableAssurance(std::sync::atomic::AtomicBool);
impl graphql_orm::graphql::assurance::AssuranceRequirementEvaluator for SwitchableAssurance {
    fn enforce(
        &self,
        _: &graphql_orm::graphql::assurance::AssuranceCheck<'_>,
        _: graphql_orm::graphql::assurance::AssuranceActorClass,
        _: &str,
    ) -> async_graphql::Result<()> {
        if self.0.load(Ordering::SeqCst) {
            Err(OrmPublicError::forbidden().into_graphql_error())
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn a_described_subscription_stops_when_its_authority_is_lost() {
    use graphql_orm::graphql::assurance::{
        AssuranceActorClass, AssuranceEnforcement, AssuranceSchemaConfig,
        OperationAssuranceRegistry,
    };
    let mut registry = OperationAssuranceRegistry::builder(
        graphql_orm_operation_catalog(),
        AssuranceSchemaConfig::legacy(),
    );
    registry
        .register_custom(
            "custom:live-ticks:v1",
            GraphqlOperationKind::Subscription,
            "liveTicks",
            AssuranceActorClass::Interactive,
        )
        .unwrap()
        .require(
            GraphqlOperationKind::Subscription,
            "liveTicks",
            "interactive.recent-auth",
        )
        .unwrap();
    let assurance = Arc::new(SwitchableAssurance::default());
    let (ticks, source) = graphql_orm::futures::channel::mpsc::unbounded::<i32>();
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let schema = schema_builder(db)
        .data(Calls::default())
        .data(TickSource::new(Some(source)))
        .data(AssuranceEnforcement::new(
            Arc::new(registry.build().unwrap()),
            assurance.clone(),
        ))
        .finish();
    let mut stream = Box::pin(
        schema.execute_stream(Request::new("subscription { liveTicks }").data(subject(&[]))),
    );

    ticks.unbounded_send(1).unwrap();
    let first = stream.next().await.expect("an authorized item");
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(first.data.into_json().unwrap()["liveTicks"], 1);

    // Authority is lost while the stream is open: the next item is never
    // released and the stream ends.
    assurance.0.store(true, Ordering::SeqCst);
    ticks.unbounded_send(2).unwrap();
    let next = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .expect("the stream ends instead of waiting");
    assert!(
        next.is_none(),
        "an item was released after revocation: {next:?}"
    );
}

#[tokio::test]
async fn a_host_guard_is_re_evaluated_while_a_described_subscription_is_open() {
    let revoked = LiveAuthority::default();
    let (ticks, source) = graphql_orm::futures::channel::mpsc::unbounded::<i32>();
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let schema = schema_builder(db)
        .data(Calls::default())
        .data(TickSource::new(Some(source)))
        .data(revoked.clone())
        .finish();
    let mut stream = Box::pin(
        schema.execute_stream(Request::new("subscription { guardedTicks }").data(subject(&[]))),
    );
    ticks.unbounded_send(1).unwrap();
    let first = stream.next().await.expect("an authorized item");
    assert!(first.errors.is_empty(), "{:?}", first.errors);

    // The host revokes the subscriber: the next item is not released.
    revoked.0.store(true, Ordering::SeqCst);
    ticks.unbounded_send(2).unwrap();
    let next = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .expect("the stream ends instead of waiting");
    assert!(
        next.is_none(),
        "an item was released after revocation: {next:?}"
    );
}

#[tokio::test]
async fn described_subscriptions_without_a_context_argument_are_watched_too() {
    let (schema, _) = schema().await;
    let mut stream =
        Box::pin(schema.execute_stream(
            Request::new("subscription { plainTicks(first: 4) }").data(subject(&[])),
        ));
    for expected in [4, 5] {
        let item = stream.next().await.expect("an item");
        assert!(item.errors.is_empty(), "{:?}", item.errors);
        assert_eq!(item.data.into_json().unwrap()["plainTicks"], expected);
    }
    let mut refused =
        Box::pin(schema.execute_stream(Request::new("subscription { plainTicks(first: 4) }")));
    assert_eq!(
        code(&refused.next().await.unwrap()).as_deref(),
        Some("UNAUTHENTICATED")
    );
}

#[tokio::test]
async fn entity_resolvers_enforce_the_declaration_in_their_body() {
    let query = "{ _entities(representations: [{__typename: \"DeclaredWidget\", id: \"w1\"}]) { ... on DeclaredWidget { id } } }";
    assert_eq!(outcome(query, None).await, refused("UNAUTHENTICATED"));
    assert_eq!(outcome(query, Some(&[])).await, (None, 1, 0));
}

#[tokio::test]
async fn subscriptions_refuse_before_the_stream_is_created() {
    for (scopes, expected) in [(None, Some("UNAUTHENTICATED")), (Some(&[][..]), None)] {
        let (schema, calls) = schema().await;
        let mut request = Request::new("subscription { protectedTicks }");
        if let Some(scopes) = scopes {
            request = request.data(subject(scopes));
        }
        let mut stream = Box::pin(schema.execute_stream(request));
        let first = stream.next().await.expect("a refusal or an event");
        assert_eq!(code(&first).as_deref(), expected);
        assert_eq!(
            calls.resolvers.load(Ordering::SeqCst),
            usize::from(expected.is_none())
        );
    }
}

#[test]
fn the_catalogue_reports_a_policy_only_where_one_is_enforced() {
    let catalog = graphql_orm_semantic_catalog();
    let has_policy = |field: &str| {
        catalog
            .operations
            .iter()
            .find(|operation| operation.field_name == field)
            .unwrap_or_else(|| panic!("{field} is catalogued"))
            .has_authorization_policy
    };
    for protected in [
        "protectedStatus",
        "guardedStatus",
        "exportedReport",
        "anyReport",
        "writeNote",
        "annotateNote",
        "auditNote",
        "peekNote",
        "pingNotes",
        "protectedTicks",
        "plainTicks",
        "liveTicks",
        "guardedTicks",
    ] {
        assert!(has_policy(protected), "{protected}");
    }
    for public in ["openStatus", "publicStatus"] {
        assert!(!has_policy(public), "{public}");
    }
}
