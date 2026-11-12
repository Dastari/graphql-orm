// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! Every field that a read argument references (a `where` predicate, an
//! `orderBy` term, or a full-text search source) passes the same field read
//! policy as a selected field, on every generated surface. Relation traversal
//! enforces the target entity's auth mode and operation scopes.

use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use std::any::Any;
use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

const PRIVATE_DETAIL: &str = "synthetic-private-detail";

/// A host callback failure propagated with `?`, rather than an authored
/// refusal: it carries a source and no public code.
fn host_failure() -> async_graphql::Error {
    std::io::Error::other(PRIVATE_DETAIL).into()
}

/// Field authority that denies a configurable set of policy keys, or fails
/// with a private host error for them.
#[derive(Clone, Default)]
struct FieldAuthority {
    denied: Arc<Mutex<HashSet<&'static str>>>,
    failing: Arc<Mutex<HashSet<&'static str>>>,
}
impl FieldAuthority {
    fn deny(&self, key: &'static str) {
        self.denied.lock().unwrap().insert(key);
    }
    fn fail(&self, key: &'static str) {
        self.failing.lock().unwrap().insert(key);
    }
    fn decide(&self, key: Option<&'static str>) -> async_graphql::Result<bool> {
        let Some(key) = key else { return Ok(true) };
        if self.failing.lock().unwrap().contains(key) {
            return Err(host_failure());
        }
        Ok(!self.denied.lock().unwrap().contains(key))
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
}

struct CountReads(Arc<AtomicUsize>);
impl ReadQueryObserver for CountReads {
    fn on_read(&self, _: &str, _: usize) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn subject() -> AuthSubject {
    AuthSubject::new("fixture-principal")
}
fn scoped(scopes: &[&str]) -> AuthSubject {
    AuthSubject::from_parts(
        "fixture-principal",
        Vec::new(),
        scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        None,
    )
}
fn refused(response: &Response) -> bool {
    !response.errors.is_empty()
}
fn assert_no_private_detail(response: &Response) {
    let rendered = serde_json::to_string(response).unwrap();
    assert!(
        !rendered.contains(PRIVATE_DETAIL),
        "host callback detail reached the client: {rendered}"
    );
}
async fn execute_sql(db: &Database, statements: &[&str]) {
    for statement in statements {
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await
            .unwrap();
    }
}

mod staff {
    use graphql_orm::async_graphql::SimpleObject;
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_members",
        plural = "RefMembers",
        auth = "none",
        default_sort = "id ASC"
    )]
    #[graphql_orm(search(index = true, tokenizer = "unicode61", min_token_len = 2))]
    pub struct RefMember {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        pub group_id: String,
        #[graphql_orm(searchable(weight = "A"))]
        #[filterable(type = "string")]
        #[sortable]
        pub name: String,
        #[graphql_orm(read_policy = "score.read")]
        #[filterable(type = "number")]
        #[sortable]
        pub score: i32,
        #[graphql_orm(read_policy = "notes.read")]
        #[graphql_orm(searchable(weight = "B", policy = "notes.search"))]
        pub notes: Option<String>,
    }
    impl BatchLoadEntity for RefMember {
        fn batch_column() -> &'static str {
            "group_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "group_id")
        }
    }

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
        table = "ref_groups",
        plural = "RefGroups",
        auth = "none"
    )]
    pub struct RefGroup {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        #[graphql(skip)]
        #[relation(
            target = "RefMember",
            from = "id",
            to = "group_id",
            multiple,
            emit_fk = false
        )]
        pub members: Vec<RefMember>,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [RefGroup, RefMember],
    }
}

async fn staff_db() -> (Database, FieldAuthority, Arc<AtomicUsize>) {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let target =
        SchemaModel::from_entities(&[staff::RefGroup::metadata(), staff::RefMember::metadata()]);
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target,
    );
    for statement in &plan.statements {
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await
            .unwrap();
    }
    execute_sql(
        &db,
        &[
            "INSERT INTO ref_groups (id) VALUES ('d')",
            "INSERT INTO ref_members (id, group_id, name, score, notes) VALUES ('e1','d','alpha',40,'plain'),('e2','d','beta',90,'needleword')",
        ],
    )
    .await;
    staff::RefMember::rebuild_search_index(&db).await.unwrap();
    let authority = FieldAuthority::default();
    db.set_field_policy(authority.clone());
    let reads = Arc::new(AtomicUsize::new(0));
    let db = db
        .with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 7, 64))
        .with_read_query_observer(CountReads(reads.clone()));
    (db, authority, reads)
}

#[tokio::test]
async fn every_filter_and_order_entry_point_authorizes_referenced_fields_before_sql() {
    let probes = [
        // Offset list ordering.
        "{ refMembers(orderBy: [{score: DESC}], page: {limit: 1}) { edges { node { id } } } }",
        // To-many relation filter and ordering.
        "{ refGroup(id: \"d\") { members(where: {score: {gt: 50}}) { pageInfo { totalCount } } } }",
        "{ refGroup(id: \"d\") { members(orderBy: {score: DESC}, page: {limit: 1}) { edges { node { id } } } } }",
        // Full-text search filter.
        "{ refMembersSearch(search: {query: \"alpha\"}, where: {score: {gt: 50}}) { edges { node { id } } } }",
    ];
    for probe in probes {
        let (db, authority, _) = staff_db().await;
        let schema = staff::schema_builder(db).finish();
        let allowed = schema.execute(Request::new(probe).data(subject())).await;
        assert!(allowed.errors.is_empty(), "{probe}: {:?}", allowed.errors);

        authority.deny("score.read");
        let denied = schema.execute(Request::new(probe).data(subject())).await;
        assert!(refused(&denied), "{probe} must refuse a denied field");
        let rendered = serde_json::to_string(&denied.data).unwrap();
        assert!(!rendered.contains("totalCount\":1"), "{probe}: {rendered}");
    }
}

#[tokio::test]
async fn offset_ordering_on_a_denied_field_is_refused_before_any_select() {
    let (db, authority, reads) = staff_db().await;
    authority.deny("score.read");
    let schema = staff::schema_builder(db).finish();
    let response = schema
        .execute(
            Request::new(
                "{ refMembers(orderBy: [{score: DESC}], page: {limit: 1}) { edges { node { id } } } }",
            )
            .data(subject()),
        )
        .await;
    assert!(refused(&response));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bulk_write_filters_on_a_denied_field_are_refused_without_writing() {
    for (field, mutation) in [
        (
            "updateRefMembers",
            "mutation { updateRefMembers(where: {score: {gt: 50}}, input: {name: \"renamed\"}) { success error affectedCount } }",
        ),
        (
            "deleteRefMembers",
            "mutation { deleteRefMembers(where: {score: {gt: 50}}) { success error deletedCount } }",
        ),
    ] {
        for deny in [true, false] {
            let (db, authority, _) = staff_db().await;
            if deny {
                authority.deny("score.read");
            }
            let schema = staff::schema_builder(db.clone()).finish();
            let response = schema.execute(Request::new(mutation).data(subject())).await;
            assert!(response.errors.is_empty(), "{field}: {:?}", response.errors);
            let data = response.data.into_json().unwrap();
            assert_eq!(data[field]["success"], !deny, "{field}: {data}");
            let unchanged: (i64,) = graphql_orm::sqlx::query_as(
                "SELECT COUNT(*) FROM ref_members WHERE name IN ('alpha','beta')",
            )
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(unchanged.0, if deny { 2 } else { 1 }, "{field}: {data}");
        }
    }
}

#[tokio::test]
async fn search_refuses_when_a_search_source_policy_is_denied() {
    let query =
        "{ refMembersSearch(search: {query: \"needleword\"}) { edges { score node { id } } } }";
    let (db, _, _) = staff_db().await;
    let schema = staff::schema_builder(db).finish();
    let allowed = schema.execute(Request::new(query).data(subject())).await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
    assert_eq!(
        allowed.data.into_json().unwrap()["refMembersSearch"]["edges"][0]["node"]["id"],
        "e2"
    );

    for key in ["notes.search", "notes.read"] {
        let (db, authority, reads) = staff_db().await;
        authority.deny(key);
        let schema = staff::schema_builder(db).finish();
        let response = schema.execute(Request::new(query).data(subject())).await;
        assert!(refused(&response), "{key} denial must refuse the search");
        assert_eq!(reads.load(Ordering::SeqCst), 0, "{key}");
    }
}

#[tokio::test]
async fn host_callback_errors_from_filter_and_field_reads_are_sanitized() {
    for query in [
        "{ refMembers(where: {score: {gt: 1}}) { edges { node { id } } } }",
        "{ refMembers { edges { node { id score } } } }",
        "{ refMembersSearch(search: {query: \"needleword\"}) { edges { node { id } } } }",
    ] {
        let (db, authority, _) = staff_db().await;
        authority.fail("score.read");
        authority.fail("notes.search");
        let schema = staff::schema_builder(db).finish();
        let response = schema.execute(Request::new(query).data(subject())).await;
        assert!(refused(&response), "{query}");
        assert_no_private_detail(&response);
    }
}

mod ledgers {
    use graphql_orm::async_graphql::SimpleObject;
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_ledgers",
        plural = "RefLedgers",
        auth = "none"
    )]
    #[graphql_orm(
        operation_authorization(categories = ["list"], all_scopes = ["ledgers.list"]),
        operation_authorization(categories = ["single_read"], all_scopes = ["ledgers.read"])
    )]
    pub struct RefLedger {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        pub owner_id: String,
        #[filterable(type = "number")]
        pub amount: i32,
    }
    impl BatchLoadEntity for RefLedger {
        fn batch_column() -> &'static str {
            "owner_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "owner_id")
        }
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_vaults",
        plural = "RefVaults",
        auth = "required"
    )]
    pub struct RefVault {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub owner_id: String,
    }
    impl BatchLoadEntity for RefVault {
        fn batch_column() -> &'static str {
            "owner_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "owner_id")
        }
    }

    /// Declares read scopes only for shapes a traversal does not map to.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_archives",
        plural = "RefArchives",
        auth = "none",
        keyset = "id asc",
        query_shapes = "single,keyset"
    )]
    #[graphql_orm(
        operation_authorization(categories = ["single_read"], all_scopes = ["archives.read"]),
        operation_authorization(categories = ["keyset_list"], all_scopes = ["archives.page"])
    )]
    pub struct RefArchive {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub owner_id: String,
    }
    impl BatchLoadEntity for RefArchive {
        fn batch_column() -> &'static str {
            "owner_id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "owner_id")
        }
    }

    /// Declares scopes only for the list shape.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_badges",
        plural = "RefBadges",
        auth = "none",
        query_shapes = "list"
    )]
    #[graphql_orm(operation_authorization(categories = ["list"], all_scopes = ["badges.list"]))]
    pub struct RefBadge {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
    }
    impl BatchLoadEntity for RefBadge {
        fn batch_column() -> &'static str {
            "id"
        }
        fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
            DefaultBackend::try_get_string(row, "id")
        }
    }

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
        table = "ref_owners",
        plural = "RefOwners",
        auth = "none"
    )]
    pub struct RefOwner {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub ledger_id: String,
        pub badge_id: String,
        #[graphql(skip)]
        #[relation(
            target = "RefArchive",
            from = "id",
            to = "owner_id",
            multiple,
            emit_fk = false
        )]
        pub archives: Vec<RefArchive>,
        #[graphql(skip)]
        #[relation(target = "RefBadge", from = "badge_id", to = "id", emit_fk = false)]
        pub badge: Option<RefBadge>,
        #[graphql(skip)]
        #[relation(
            target = "RefLedger",
            from = "id",
            to = "owner_id",
            multiple,
            emit_fk = false
        )]
        pub ledgers: Vec<RefLedger>,
        #[graphql(skip)]
        #[relation(target = "RefLedger", from = "ledger_id", to = "id", emit_fk = false)]
        pub ledger: Option<RefLedger>,
        #[graphql(skip)]
        #[relation(
            target = "RefVault",
            from = "id",
            to = "owner_id",
            multiple,
            emit_fk = false
        )]
        pub vaults: Vec<RefVault>,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_holders",
        plural = "RefHolders",
        auth = "none"
    )]
    pub struct RefHolder {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        #[relation(
            target = "RefLedger",
            from = "id",
            to = "owner_id",
            multiple,
            filter,
            emit_fk = false,
            order_aggregate(name = "LedgerCount", aggregate = "count")
        )]
        pub ledgers: Vec<RefLedger>,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [RefOwner, RefLedger, RefVault, RefHolder, RefArchive, RefBadge],
    }
}

async fn ledger_schema()
-> async_graphql::Schema<ledgers::QueryRoot, ledgers::MutationRoot, ledgers::SubscriptionRoot> {
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    execute_sql(
        &db,
        &[
            "CREATE TABLE ref_owners (id TEXT PRIMARY KEY, ledger_id TEXT NOT NULL, badge_id TEXT NOT NULL)",
            "CREATE TABLE ref_archives (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL)",
            "CREATE TABLE ref_badges (id TEXT PRIMARY KEY)",
            "INSERT INTO ref_archives VALUES ('a1','o')",
            "INSERT INTO ref_badges VALUES ('b1')",
            "CREATE TABLE ref_ledgers (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, amount INTEGER NOT NULL)",
            "CREATE TABLE ref_vaults (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL)",
            "CREATE TABLE ref_holders (id TEXT PRIMARY KEY)",
            "INSERT INTO ref_owners VALUES ('o','l1','b1')",
            "INSERT INTO ref_ledgers VALUES ('l1','o',10),('l2','o',20)",
            "INSERT INTO ref_vaults VALUES ('v1','o')",
            "INSERT INTO ref_holders VALUES ('o'),('p')",
        ],
    )
    .await;
    ledgers::schema_builder(db).finish()
}

#[tokio::test]
async fn relation_traversal_enforces_the_target_operation_scopes() {
    let schema = ledger_schema().await;
    for (query, scope) in [
        (
            "{ refOwner(id: \"o\") { ledgers { edges { node { id } } } } }",
            "ledgers.list",
        ),
        ("{ refOwner(id: \"o\") { ledger { id } } }", "ledgers.read"),
        (
            "{ refHolders(where: {ledgers: [{amount: {gt: 15}}]}) { edges { node { id } } } }",
            "ledgers.list",
        ),
        (
            "{ refHolders(orderBy: [{LedgerCount: DESC}]) { edges { node { id } } } }",
            "ledgers.list",
        ),
    ] {
        let denied = schema.execute(Request::new(query).data(subject())).await;
        assert!(refused(&denied), "{query} must require {scope}");
        let rendered = serde_json::to_string(&denied.data).unwrap();
        assert!(!rendered.contains("\"l1\""), "{query}: {rendered}");

        let granted = schema
            .execute(Request::new(query).data(scoped(&[scope])))
            .await;
        assert!(granted.errors.is_empty(), "{query}: {:?}", granted.errors);
    }
}

#[tokio::test]
async fn traversal_into_an_undeclared_category_requires_every_declared_read_scope() {
    let schema = ledger_schema().await;
    for (query, required, leaked) in [
        // To-many maps to `list`, which the target does not declare: both of
        // its declared read categories (`single_read`, `keyset_list`) apply.
        (
            "{ refOwner(id: \"o\") { archives { edges { node { id } } } } }",
            &["archives.read", "archives.page"][..],
            "\"a1\"",
        ),
        // To-one maps to `single_read`, which the target does not declare.
        (
            "{ refOwner(id: \"o\") { badge { id } } }",
            &["badges.list"][..],
            "\"b1\"",
        ),
    ] {
        let denied = schema.execute(Request::new(query).data(subject())).await;
        assert!(refused(&denied), "{query} must require {required:?}");
        let rendered = serde_json::to_string(&denied.data).unwrap();
        assert!(!rendered.contains(leaked), "{query}: {rendered}");

        // Holding only some of the declared read scopes is not enough.
        for partial in required {
            if required.len() == 1 {
                break;
            }
            let response = schema
                .execute(Request::new(query).data(scoped(&[partial])))
                .await;
            assert!(refused(&response), "{query} with only {partial}");
            let rendered = serde_json::to_string(&response.data).unwrap();
            assert!(!rendered.contains(leaked), "{query}: {rendered}");
        }

        let granted = schema
            .execute(Request::new(query).data(scoped(required)))
            .await;
        assert!(granted.errors.is_empty(), "{query}: {:?}", granted.errors);
        let rendered = serde_json::to_string(&granted.data).unwrap();
        assert!(rendered.contains(leaked), "{query}: {rendered}");
    }
}

#[tokio::test]
async fn relation_traversal_enforces_the_target_auth_mode() {
    let schema = ledger_schema().await;
    let query = "{ refOwner(id: \"o\") { vaults { edges { node { id } } } } }";
    let anonymous = schema.execute(Request::new(query)).await;
    assert!(
        refused(&anonymous),
        "a required-auth target needs a subject"
    );
    let signed_in = schema.execute(Request::new(query).data(subject())).await;
    assert!(signed_in.errors.is_empty(), "{:?}", signed_in.errors);
}

mod repository_notes {
    use graphql_orm::prelude::*;

    #[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
    #[repository_entity(backend = "sqlite", table = "ref_repo_notes", plural = "RefRepoNotes")]
    #[graphql_orm(search(index = true, tokenizer = "unicode61"))]
    pub struct RefRepoNote {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        pub id: String,
        #[graphql_orm(searchable(weight = "A"))]
        pub title: String,
        #[graphql_orm(read_policy = "note.body.read")]
        #[graphql_orm(searchable(weight = "B", policy = "note.body.search"))]
        pub body: Option<String>,
    }
}

#[tokio::test]
async fn repository_search_refuses_when_a_search_source_policy_is_denied() {
    use repository_notes::RefRepoNote;
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    execute_sql(
        &db,
        &[
            "CREATE TABLE ref_repo_notes (id TEXT PRIMARY KEY, title TEXT NOT NULL, body TEXT)",
            "INSERT INTO ref_repo_notes VALUES ('n1','public title','needleword secret')",
        ],
    )
    .await;
    let authority = FieldAuthority::default();
    db.set_field_policy(authority.clone());
    let search = || SearchInput {
        query: "needleword".to_owned(),
        mode: Some(SearchMode::Plain),
        min_score: None,
    };
    let hits = RefRepoNote::search_db(&db, search())
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);

    authority.deny("note.body.search");
    let denied = RefRepoNote::search_db(&db, search()).fetch_all().await;
    assert!(
        denied.is_err(),
        "a denied search source must refuse the repository search"
    );
}

mod tagged_docs {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_docs",
        plural = "RefDocs",
        auth = "none"
    )]
    #[graphql_orm(search(index = true, tokenizer = "unicode61", min_token_len = 2))]
    pub struct RefDoc {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        #[graphql_orm(searchable(weight = "A"))]
        pub title: String,
        #[relation(
            target = "RefTag",
            from = "id",
            to = "doc_id",
            multiple,
            filter,
            emit_fk = false
        )]
        pub tags: Vec<RefTag>,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_tags",
        plural = "RefTags",
        auth = "none"
    )]
    pub struct RefTag {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub doc_id: String,
        #[filterable(type = "string")]
        pub label: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [RefDoc, RefTag],
    }
}

#[tokio::test]
async fn native_and_fallback_search_refuse_an_unprepared_relation_filter() {
    use tagged_docs::{RefDoc, RefDocWhereInput, RefTag, RefTagWhereInput};
    for native in [true, false] {
        let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap();
        if native {
            let target = SchemaModel::from_entities(&[RefDoc::metadata(), RefTag::metadata()]);
            let plan = build_migration_plan(
                DatabaseBackend::Sqlite,
                &SchemaModel {
                    extensions: Vec::new(),
                    tables: Vec::new(),
                },
                &target,
            );
            for statement in &plan.statements {
                graphql_orm::sqlx::query(statement)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        } else {
            for statement in [
                "CREATE TABLE ref_docs (id TEXT PRIMARY KEY, title TEXT NOT NULL)",
                "CREATE TABLE ref_tags (id TEXT PRIMARY KEY, doc_id TEXT NOT NULL, label TEXT NOT NULL)",
            ] {
                graphql_orm::sqlx::query(statement)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
        for statement in [
            "INSERT INTO ref_docs VALUES ('a','orbit notes'),('b','orbit plans')",
            "INSERT INTO ref_tags VALUES ('t','a','kept')",
        ] {
            graphql_orm::sqlx::query(statement)
                .execute(&pool)
                .await
                .unwrap();
        }
        let db = Database::<SqliteBackend>::new(pool.clone());
        if native {
            RefDoc::rebuild_search_index(&db).await.unwrap();
        }
        let unprepared = RefDocWhereInput {
            tags: Some(vec![RefTagWhereInput {
                label: Some(StringFilter {
                    eq: Some("kept".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let search = || SearchInput {
            query: "orbit".to_owned(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        };
        let all = RefDoc::search(&pool, search())
            .filter(unprepared.clone())
            .fetch_all()
            .await;
        assert!(all.is_err(), "native={native}: {:?}", all.map(|h| h.len()));
        let connection = RefDoc::search(&pool, search())
            .filter(unprepared)
            .fetch_connection()
            .await;
        assert!(connection.is_err(), "native={native}");
    }
}

mod keyed {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_keyed",
        plural = "RefKeyeds",
        auth = "none",
        federation_key
    )]
    pub struct RefKeyed {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [RefKeyed],
    }
}

struct FailingEntityAndRowPolicy {
    fail_entity: bool,
}
impl EntityPolicy for FailingEntityAndRowPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        let fail = self.fail_entity;
        Box::pin(async move { if fail { Err(host_failure()) } else { Ok(true) } })
    }
}
impl RowPolicy for FailingEntityAndRowPolicy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async { Err(host_failure()) })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Err(host_failure()) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[tokio::test]
async fn host_callback_errors_from_federation_and_list_visibility_are_sanitized() {
    for (fail_entity, query) in [
        (
            true,
            "{ _entities(representations: [{__typename: \"RefKeyed\", id: \"k\"}]) { ... on RefKeyed { id } } }",
        ),
        (
            false,
            "{ _entities(representations: [{__typename: \"RefKeyed\", id: \"k\"}]) { ... on RefKeyed { id } } }",
        ),
        (false, "{ refKeyeds { edges { node { id } } } }"),
    ] {
        let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        execute_sql(
            &db,
            &[
                "CREATE TABLE ref_keyed (id TEXT PRIMARY KEY)",
                "INSERT INTO ref_keyed VALUES ('k')",
            ],
        )
        .await;
        db.set_entity_policy(FailingEntityAndRowPolicy { fail_entity });
        db.set_row_policy(FailingEntityAndRowPolicy { fail_entity });
        let schema = keyed::schema_builder(db).finish();
        let response = schema.execute(Request::new(query).data(subject())).await;
        assert!(refused(&response), "{query}");
        assert_no_private_detail(&response);
    }
}

mod catalog {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_catalogs",
        plural = "RefCatalogs",
        auth = "none"
    )]
    #[graphql_orm(search(index = true, tokenizer = "unicode61", min_token_len = 2))]
    pub struct RefCatalog {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        #[graphql_orm(searchable(weight = "A"))]
        pub title: String,
        #[graphql_orm(search_relation(fields = ["label"], policy = "labels.search"))]
        #[relation(
            target = "RefLabel",
            from = "id",
            to = "catalog_id",
            multiple,
            emit_fk = false
        )]
        pub labels: Vec<RefLabel>,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "ref_labels",
        plural = "RefLabels",
        auth = "none"
    )]
    #[graphql_orm(operation_authorization(categories = ["list"], all_scopes = ["labels.list"]))]
    pub struct RefLabel {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
        pub catalog_id: String,
        #[graphql_orm(read_policy = "label.read")]
        pub label: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [RefCatalog, RefLabel],
    }
}

#[tokio::test]
async fn relation_search_sources_require_the_relation_and_target_field_policies() {
    use catalog::{RefCatalog, RefLabel};
    let setup = || async {
        let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        let target = SchemaModel::from_entities(&[RefCatalog::metadata(), RefLabel::metadata()]);
        let plan = build_migration_plan(
            DatabaseBackend::Sqlite,
            &SchemaModel {
                extensions: Vec::new(),
                tables: Vec::new(),
            },
            &target,
        );
        for statement in &plan.statements {
            graphql_orm::sqlx::query(statement)
                .execute(db.pool())
                .await
                .unwrap();
        }
        execute_sql(
            &db,
            &[
                "INSERT INTO ref_catalogs (id, title) VALUES ('c1','plain title')",
                "INSERT INTO ref_labels (id, catalog_id, label) VALUES ('l1','c1','needleword')",
            ],
        )
        .await;
        RefCatalog::rebuild_search_index(&db).await.unwrap();
        let authority = FieldAuthority::default();
        db.set_field_policy(authority.clone());
        let db = db.with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 7, 64));
        (db, authority)
    };
    let search = || SearchInput {
        query: "plain".to_owned(),
        mode: Some(SearchMode::Plain),
        min_score: None,
    };
    let query = "{ refCatalogsSearch(search: {query: \"plain\"}) { edges { node { id } } } }";

    let (db, _) = setup().await;
    let hits = RefCatalog::search_db(&db, search())
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    let schema = catalog::schema_builder(db).finish();
    // The relation source traverses into the target: its `list` scope applies.
    let unscoped = schema.execute(Request::new(query).data(subject())).await;
    assert!(
        refused(&unscoped),
        "relation search text must require the target's traversal scope"
    );
    let allowed = schema
        .execute(Request::new(query).data(scoped(&["labels.list"])))
        .await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);

    for key in ["labels.search", "label.read"] {
        let (db, authority) = setup().await;
        authority.deny(key);
        let repository = RefCatalog::search_db(&db, search()).fetch_all().await;
        assert!(repository.is_err(), "{key} denial must refuse search_db");
        let schema = catalog::schema_builder(db).finish();
        let response = schema
            .execute(Request::new(query).data(scoped(&["labels.list"])))
            .await;
        assert!(
            refused(&response),
            "{key} denial must refuse the search root"
        );
    }
}
