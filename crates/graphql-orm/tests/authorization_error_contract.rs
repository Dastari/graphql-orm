// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! Authorization refusals keep their public code on every path, host callback
//! text never reaches a caller, and the strict mode's key-less field default
//! is the documented one (#450).

use graphql_orm::async_graphql::{ErrorExtensions, Request, Response};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::any::Any;
use std::sync::{Arc, Mutex};

const PRIVATE_DETAIL: &str = "synthetic-private-detail";

/// What the installed field policy answers for the policy key `notes.read`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Answer {
    Deny,
    Forbidden,
    Unauthenticated,
    Misconfigured,
    TypedForbidden,
    HostFailure,
    UncodedText,
    HostCode,
    DeclaredInternal,
    ServiceUnavailable,
}

fn answer(answer: Answer) -> async_graphql::Result<bool> {
    match answer {
        Answer::Deny => Ok(false),
        Answer::Forbidden => Err(OrmPublicError::forbidden().into_graphql_error()),
        Answer::Unauthenticated => Err(OrmPublicError::unauthenticated().into_graphql_error()),
        Answer::Misconfigured => {
            Err(OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured).into_graphql_error())
        }
        // A typed public error propagated with `?`.
        Answer::TypedForbidden => Err(OrmPublicError::forbidden())?,
        // A host failure propagated with `?`: a source and no public code.
        Answer::HostFailure => Err(std::io::Error::other(PRIVATE_DETAIL))?,
        // A host failure written as a plain error: no source and no code.
        Answer::UncodedText => Err(async_graphql::Error::new(format!(
            "policy store down: {PRIVATE_DETAIL}"
        ))),
        Answer::ServiceUnavailable => {
            Err(OrmPublicError::new(OrmErrorCode::ServiceUnavailable).into_graphql_error())
        }
        // An authored refusal under a code only the host knows.
        Answer::HostCode => Err(async_graphql::Error::new(PRIVATE_DETAIL)
            .extend_with(|_, extensions| extensions.set("code", "HOST_POLICY_REFUSAL"))),
        // An authored error that declares itself internal.
        Answer::DeclaredInternal => Err(async_graphql::Error::new(PRIVATE_DETAIL)
            .extend_with(|_, extensions| extensions.set("code", "INTERNAL_ERROR"))),
    }
}

#[derive(Clone)]
struct FieldAuthority(Arc<Mutex<Answer>>);
impl FieldAuthority {
    fn decide(&self, key: Option<&'static str>) -> async_graphql::Result<bool> {
        match key {
            Some("notes.read") => answer(*self.0.lock().unwrap()),
            _ => Ok(true),
        }
    }
}
impl FieldPolicy for FieldAuthority {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        policy: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { self.decide(policy) })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        policy: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { self.decide(policy) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        policy: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            match policy {
                Some("notes.write") => answer(*self.0.lock().unwrap()),
                _ => Ok(true),
            }
        })
    }
}

mod schema {
    use graphql_orm::async_graphql::SimpleObject;
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        GraphQLRelations,
        SimpleObject,
        Clone,
        Debug,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql(complex)]
    #[graphql_entity(
        backend = "sqlite",
        table = "contract_owners",
        plural = "ContractOwners",
        auth = "none",
        default_sort = "id ASC"
    )]
    pub struct ContractOwner {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql(skip)]
        #[graphql_orm(read_policy = "notes.read")]
        #[relation(
            target = "ContractNote",
            from = "id",
            to = "owner_id",
            multiple,
            emit_fk = false
        )]
        pub notes: Vec<ContractNote>,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "contract_notes",
        plural = "ContractNotes",
        auth = "none",
        default_sort = "id ASC"
    )]
    pub struct ContractNote {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        pub owner_id: String,
        #[graphql_orm(read_policy = "notes.read", write_policy = "notes.write")]
        pub body: String,
    }
    impl BatchLoadEntity for ContractNote {
        fn batch_column() -> &'static str {
            "owner_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "owner_id")
        }
    }

    /// An entity with its own read policy key, so row policy applies to it.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "contract_vaults",
        plural = "ContractVaults",
        auth = "none",
        default_sort = "id ASC",
        read_policy = "vaults.read"
    )]
    pub struct ContractVault {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[graphql_orm(read_policy = "vaults.secret.read")]
        pub secret: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [ContractOwner, ContractNote, ContractVault],
    }
}
use schema::{ContractNote, ContractOwner};

async fn database(mode: AuthorizationMode) -> Database {
    let db = Database::connect_sqlite("sqlite::memory:")
        .await
        .unwrap()
        .with_authorization_mode(mode);
    let target = SchemaModel::from_entities(&[
        ContractOwner::metadata(),
        ContractNote::metadata(),
        schema::ContractVault::metadata(),
    ]);
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target,
    );
    for statement in plan.statements.iter().map(String::as_str).chain([
        "INSERT INTO contract_owners (id) VALUES ('o1')",
        "INSERT INTO contract_notes (id, owner_id, body) VALUES ('n1', 'o1', 'first')",
    ]) {
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await
            .unwrap();
    }
    db
}

async fn with_authority(answer: Answer) -> Database {
    let mut db = database(AuthorizationMode::DeclaredPoliciesRequired).await;
    db.set_field_policy(FieldAuthority(Arc::new(Mutex::new(answer))));
    db
}

fn code(response: &Response) -> String {
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    let rendered = serde_json::to_string(response).unwrap();
    assert!(
        !rendered.contains(PRIVATE_DETAIL),
        "host callback detail reached the client: {rendered}"
    );
    response.errors[0]
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        .map(|code| code.to_string().trim_matches('"').to_owned())
        .unwrap_or_default()
}

/// The public code each field policy answer must keep.
const REFUSALS: [(Answer, OrmErrorCode); 8] = [
    (Answer::Deny, OrmErrorCode::Forbidden),
    (Answer::Forbidden, OrmErrorCode::Forbidden),
    (Answer::TypedForbidden, OrmErrorCode::Forbidden),
    (Answer::Unauthenticated, OrmErrorCode::Unauthenticated),
    (
        Answer::Misconfigured,
        OrmErrorCode::AuthorizationMisconfigured,
    ),
    (Answer::HostFailure, OrmErrorCode::InternalError),
    // Only a known public code marks a callback's text as safe.
    (Answer::UncodedText, OrmErrorCode::InternalError),
    (Answer::ServiceUnavailable, OrmErrorCode::ServiceUnavailable),
];

#[tokio::test]
async fn repository_field_read_refusals_keep_their_public_code() {
    for (decision, expected) in REFUSALS {
        let db = with_authority(decision).await;
        let error = ContractNote::find_by_id(&db, &"n1".to_owned())
            .await
            .expect_err("a refused field refuses the repository read");
        let public = OrmPublicError::from_sqlx(&error);
        assert_eq!(public.code, expected, "{decision:?}");
        assert!(!error.to_string().contains(PRIVATE_DETAIL), "{decision:?}");
        assert!(
            !format!("{error:?}").contains(PRIVATE_DETAIL),
            "{decision:?}"
        );
        let graphql = graphql_orm::graphql::errors::graphql_error_from_sqlx(error);
        assert!(!graphql.message.contains(PRIVATE_DETAIL), "{decision:?}");
        assert_eq!(
            graphql.extensions.unwrap().get("code"),
            Some(&async_graphql::Value::from(expected.as_str())),
            "{decision:?}"
        );
    }
}

#[tokio::test]
async fn repository_field_write_refusals_keep_their_public_code() {
    for (decision, expected) in REFUSALS {
        let db = with_authority(decision).await;
        let error = db
            .ensure_repository_writable_field(
                None,
                "ContractNote",
                "body",
                Some("notes.write"),
                None,
                None,
            )
            .await
            .expect_err("a refused field refuses the repository write");
        assert_eq!(error.code, expected, "{decision:?}");
        assert!(!error.to_string().contains(PRIVATE_DETAIL), "{decision:?}");
        assert!(
            !format!("{error:?}").contains(PRIVATE_DETAIL),
            "{decision:?}"
        );
        let graphql = error.into_graphql_error();
        assert!(!graphql.message.contains(PRIVATE_DETAIL), "{decision:?}");
    }
}

#[tokio::test]
async fn generated_field_reads_keep_their_public_code() {
    // A GraphQL field getter renders an uncoded authored error as written
    // (the 0.127.0 contract), so that case belongs to the repository tests.
    for (decision, expected) in REFUSALS
        .into_iter()
        .filter(|(decision, _)| *decision != Answer::UncodedText)
    {
        let db = with_authority(decision).await;
        let schema = schema::schema_builder(db).finish();
        let response = schema
            .execute(Request::new("{ contractNote(id: \"n1\") { id body } }"))
            .await;
        assert_eq!(code(&response), expected.as_str(), "{decision:?}");
    }
}

#[tokio::test]
async fn a_custom_resolver_never_relays_repository_policy_text() {
    // A host resolver that maps a repository error with
    // `graphql_error_from_sqlx` must not hand the policy's text to a client.
    for decision in [Answer::UncodedText, Answer::HostFailure] {
        let db = with_authority(decision).await;
        let error = ContractNote::find_by_id(&db, &"n1".to_owned())
            .await
            .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)
            .expect_err("the repository read is refused");
        assert_eq!(error.message, "internal error", "{decision:?}");
        assert_eq!(
            error.extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from("INTERNAL_ERROR")),
            "{decision:?}"
        );
        assert!(
            !format!("{error:?}").contains(PRIVATE_DETAIL),
            "{decision:?}"
        );
    }
}

#[tokio::test]
async fn relation_boundaries_keep_the_class_of_a_refusal_or_failure() {
    // An authored refusal is FORBIDDEN or UNAUTHENTICATED; a propagated or
    // declared failure keeps its failure class. No text crosses the boundary.
    for (decision, expected) in [
        (Answer::Deny, OrmErrorCode::Forbidden),
        (Answer::Forbidden, OrmErrorCode::Forbidden),
        (Answer::TypedForbidden, OrmErrorCode::Forbidden),
        (Answer::Unauthenticated, OrmErrorCode::Unauthenticated),
        (
            Answer::Misconfigured,
            OrmErrorCode::AuthorizationMisconfigured,
        ),
        (Answer::HostFailure, OrmErrorCode::InternalError),
        // An error the policy authored itself is its refusal, coded or not.
        (Answer::UncodedText, OrmErrorCode::Forbidden),
        (Answer::HostCode, OrmErrorCode::Forbidden),
        (Answer::DeclaredInternal, OrmErrorCode::InternalError),
        (Answer::ServiceUnavailable, OrmErrorCode::ServiceUnavailable),
    ] {
        let db = with_authority(decision).await;
        let schema = schema::schema_builder(db).finish();
        let response = schema
            .execute(Request::new(
                "{ contractOwner(id: \"o1\") { id notes { edges { node { id } } } } }",
            ))
            .await;
        assert_eq!(code(&response), expected.as_str(), "{decision:?}");
    }
}

#[tokio::test]
async fn a_declared_key_without_a_field_provider_is_a_misconfiguration_in_both_strict_modes() {
    for mode in [
        AuthorizationMode::DeclaredPoliciesRequired,
        AuthorizationMode::ExplicitPolicyForAllExposedOperations,
    ] {
        let db = database(mode).await;
        // Key-less fields stay readable and writable without a provider.
        db.ensure_repository_readable_field(None, "ContractNote", "id", None, None)
            .await
            .expect("a key-less field is readable without a provider");
        db.ensure_repository_writable_field(None, "ContractNote", "id", None, None, None)
            .await
            .expect("a key-less field is writable without a provider");
        // A declared key without a provider is a configuration error.
        let read = db
            .ensure_repository_readable_field(
                None,
                "ContractNote",
                "body",
                Some("notes.read"),
                None,
            )
            .await
            .expect_err("a declared read key needs a provider");
        assert_eq!(
            read.code,
            OrmErrorCode::AuthorizationMisconfigured,
            "{mode:?}"
        );
        let write = db
            .ensure_repository_writable_field(
                None,
                "ContractNote",
                "body",
                Some("notes.write"),
                None,
                None,
            )
            .await
            .expect_err("a declared write key needs a provider");
        assert_eq!(
            write.code,
            OrmErrorCode::AuthorizationMisconfigured,
            "{mode:?}"
        );
    }
}

/// Entity and row authority that admits everything except the row `hidden`.
struct AllButHidden;
impl EntityPolicy for AllButHidden {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: graphql_orm::graphql::orm::EntityAccessKind,
        _: graphql_orm::graphql::orm::EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl RowPolicy for AllButHidden {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: graphql_orm::graphql::orm::EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        let visible = row
            .downcast_ref::<schema::ContractVault>()
            .is_none_or(|vault| vault.id != "hidden");
        Box::pin(async move { Ok(visible) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: graphql_orm::graphql::orm::EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[tokio::test]
async fn a_missing_field_provider_does_not_tell_a_hidden_row_from_a_missing_one() {
    // Under the strictest mode the keyed field of a readable row reports the
    // misconfiguration. A row the caller cannot read answers exactly as a
    // missing one, so the refusal reveals nothing about hidden rows.
    let mut db = database(AuthorizationMode::ExplicitPolicyForAllExposedOperations).await;
    for statement in [
        "INSERT INTO contract_vaults (id, secret) VALUES ('open', 's1')",
        "INSERT INTO contract_vaults (id, secret) VALUES ('hidden', 's2')",
    ] {
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await
            .unwrap();
    }
    db.set_entity_policy(AllButHidden);
    db.set_row_policy(AllButHidden);
    let schema = schema::schema_builder(db).finish();
    let vault = |id: &'static str| {
        let schema = schema.clone();
        async move {
            schema
                .execute(Request::new(format!(
                    "{{ contractVault(id: \"{id}\") {{ id secret }} }}"
                )))
                .await
        }
    };
    let readable = vault("open").await;
    assert_eq!(code(&readable), "AUTHORIZATION_MISCONFIGURED");
    let hidden = serde_json::to_string(&vault("hidden").await).unwrap();
    let missing = serde_json::to_string(&vault("absent").await).unwrap();
    assert_eq!(hidden, missing);
    assert!(hidden.contains("\"contractVault\":null"), "{hidden}");
    assert!(!hidden.contains("errors"), "{hidden}");
}
