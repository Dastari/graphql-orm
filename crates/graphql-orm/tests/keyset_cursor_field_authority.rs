#![cfg(feature = "sqlite")]
use graphql_orm::async_graphql::Request;
use graphql_orm::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "cursor_policy_records",
    plural = "CursorPolicyRecords",
    keyset = "protected_rank asc, id asc"
)]
struct CursorPolicyRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[graphql_orm(read_policy = "rank.read")]
    #[sortable]
    protected_rank: i64,
}
schema_roots! { query_custom_ops: [], generated_mutations: "none", entities: [CursorPolicyRecord], }

#[derive(Clone)]
struct RankAuthority {
    allowed: Arc<AtomicBool>,
    deny_rows: Arc<AtomicBool>,
    deny_keys: Arc<AtomicBool>,
}
impl RankAuthority {
    fn new() -> Self {
        Self {
            allowed: Arc::new(AtomicBool::new(true)),
            deny_rows: Arc::new(AtomicBool::new(false)),
            deny_keys: Arc::new(AtomicBool::new(false)),
        }
    }
}
impl FieldPolicy for RankAuthority {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(match field {
                "protectedRank" => {
                    self.allowed.load(Ordering::SeqCst)
                        && (record.is_none() || !self.deny_rows.load(Ordering::SeqCst))
                }
                "id" => !self.deny_keys.load(Ordering::SeqCst),
                _ => true,
            })
        })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(match field {
                "protectedRank" => {
                    self.allowed.load(Ordering::SeqCst)
                        && (record.is_none() || !self.deny_rows.load(Ordering::SeqCst))
                }
                "id" => !self.deny_keys.load(Ordering::SeqCst),
                _ => true,
            })
        })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[tokio::test]
async fn configured_keyset_cursor_only_selection_requires_unselected_order_field_authority() {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE cursor_policy_records (id TEXT PRIMARY KEY, protected_rank INTEGER NOT NULL)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO cursor_policy_records VALUES ('one',17),('two',29)")
        .execute(db.pool())
        .await
        .unwrap();
    let authority = RankAuthority::new();
    db.set_field_policy(authority.clone());
    let schema = schema_builder(db).finish();
    let request = || {
        Request::new("{ cursorPolicyRecordsKeyset(page: {limit: 1}) { edges { cursor } } }")
            .data(AuthSubject::new("fixture-principal"))
    };
    let before = schema.execute(request()).await;
    assert!(before.errors.is_empty(), "{:?}", before.errors);
    let value = before.data.into_json().unwrap();
    let cursor = value["cursorPolicyRecordsKeyset"]["edges"][0]["cursor"]
        .as_str()
        .unwrap();
    assert!(cursor.starts_with("gomk1."));
    authority.allowed.store(false, Ordering::SeqCst);
    let denied = schema.execute(request()).await;
    assert!(
        !denied.errors.is_empty(),
        "a cursor-only selection must not bypass the configured ordering field's current read policy"
    );
    assert!(denied.data.into_json().unwrap()["cursorPolicyRecordsKeyset"].is_null());
}

struct Observe {
    reads: Arc<AtomicUsize>,
    revoke: Option<Arc<AtomicBool>>,
}
impl ReadQueryObserver for Observe {
    fn on_read(&self, _: &str, _: usize) {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(allowed) = &self.revoke {
            allowed.store(false, Ordering::SeqCst);
        }
    }
}
async fn setup() -> (Database, RankAuthority, Arc<AtomicUsize>) {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE cursor_policy_records (id TEXT PRIMARY KEY, protected_rank INTEGER NOT NULL)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO cursor_policy_records VALUES ('one',17),('two',29)")
        .execute(db.pool())
        .await
        .unwrap();
    let authority = RankAuthority::new();
    db.set_field_policy(authority.clone());
    let reads = Arc::new(AtomicUsize::new(0));
    let db = db.with_read_query_observer(Observe {
        reads: reads.clone(),
        revoke: None,
    });
    (db, authority, reads)
}
#[tokio::test]
async fn configured_orders_deny_before_select_even_for_cursor_only_payload() {
    for denied in ["rank", "key"] {
        let (db, authority, reads) = setup().await;
        if denied == "rank" {
            authority.allowed.store(false, Ordering::SeqCst);
        } else {
            authority.deny_keys.store(true, Ordering::SeqCst);
        }
        let schema = schema_builder(db).finish();
        let response = schema
            .execute(
                Request::new(
                    "{ cursorPolicyRecordsKeyset(page: {limit: 1}) { edges { cursor } } }",
                )
                .data(AuthSubject::new("fixture-principal")),
            )
            .await;
        assert!(!response.errors.is_empty());
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn row_field_denial_and_permission_change_during_read_never_release_cursor() {
    for during_read in [false, true] {
        let (db, authority, reads) = setup().await;
        let db = if during_read {
            db.with_read_query_observer(Observe {
                reads: reads.clone(),
                revoke: Some(authority.allowed.clone()),
            })
        } else {
            authority.deny_rows.store(true, Ordering::SeqCst);
            db
        };
        let schema = schema_builder(db).finish();
        let response = schema
            .execute(
                Request::new(
                    "{ cursorPolicyRecordsKeyset(page: {limit: 1}) { edges { cursor } } }",
                )
                .data(AuthSubject::new("fixture-principal")),
            )
            .await;
        assert!(!response.errors.is_empty());
        assert!(reads.load(Ordering::SeqCst) > 0);
        assert!(response.data.into_json().unwrap()["cursorPolicyRecordsKeyset"].is_null());
    }
}
#[tokio::test]
async fn repository_backward_and_scan_paths_retain_current_field_checks() {
    let (db, authority, reads) = setup().await;
    let configured = CursorPolicyRecord::keyset_page(&db, Default::default(), Default::default())
        .await
        .unwrap();
    assert!(
        configured
            .page_info
            .start_cursor
            .unwrap()
            .starts_with("gomk1.")
    );
    authority.allowed.store(false, Ordering::SeqCst);
    reads.store(0, Ordering::SeqCst);
    assert!(
        CursorPolicyRecord::keyset_connection_page(
            &db,
            Default::default(),
            KeysetConnectionInput {
                last: Some(1),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    let db = db.with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 2, 4));
    assert!(
        CursorPolicyRecord::authorized_scan(&db, Default::default(), Default::default())
            .await
            .is_err()
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    authority.allowed.store(true, Ordering::SeqCst);
    let scan = CursorPolicyRecord::authorized_scan(
        &db,
        Default::default(),
        AuthorizedScanInput {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(scan.nodes.len(), 1);
    assert!(scan.page_info.continuation.unwrap().starts_with("goms1."));
}

mod skipped_order {
    use super::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        table = "skipped_cursor_records",
        plural = "SkippedCursorRecords",
        keyset = "hidden_rank asc, id asc"
    )]
    struct HiddenOrderRecord {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        id: String,
        // Keep the generated update input valid while testing hidden cursor fields.
        label: String,
        #[graphql(skip)]
        hidden_rank: i64,
    }
    schema_roots! { query_custom_ops: [], generated_mutations: "none", entities: [HiddenOrderRecord], }
    #[tokio::test]
    async fn configured_hidden_field_cannot_escape_via_cursor_or_scan() {
        let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE skipped_cursor_records (id TEXT PRIMARY KEY, label TEXT NOT NULL, hidden_rank INTEGER NOT NULL)").execute(db.pool()).await.unwrap();
        sqlx::query(
            "INSERT INTO skipped_cursor_records VALUES ('one','First',17),('two','Second',29)",
        )
            .execute(db.pool())
            .await
            .unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let db = db
            .with_read_query_observer(Observe {
                reads: reads.clone(),
                revoke: None,
            })
            .with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 2, 4));
        let schema = schema_builder(db.clone()).finish();
        for operation in [
            "{ skippedCursorRecordsKeyset(page: {limit: 1}) { edges { cursor } } }",
            "{ skippedCursorRecordsScan(page: {limit: 1}) { pageInfo { continuation status } } }",
        ] {
            let response = schema
                .execute(Request::new(operation).data(AuthSubject::new("fixture-principal")))
                .await;
            assert!(
                !response.errors.is_empty(),
                "a hidden raw ordering value must be denied independently of selection"
            );
        }
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        // Trusted repository reads retain their ordinary independent field policy.
        let page = HiddenOrderRecord::keyset_page(&db, Default::default(), Default::default())
            .await
            .unwrap();
        assert_eq!(page.edges.len(), 2);
    }
}

mod private_order {
    use super::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        table = "private_cursor_records",
        plural = "PrivateCursorRecords",
        keyset = "hidden_rank asc, id asc"
    )]
    struct HiddenOrderRecord {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        id: String,
        // Keep the generated update input valid while testing hidden cursor fields.
        label: String,
        // Explicit repository surface overrides cannot publish a private cursor value.
        #[graphql_orm(private, read = true, order = true)]
        #[sortable]
        hidden_rank: i64,
    }
    schema_roots! { query_custom_ops: [], generated_mutations: "none", entities: [HiddenOrderRecord], }
    #[tokio::test]
    async fn configured_hidden_field_cannot_escape_via_cursor_or_scan() {
        let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE private_cursor_records (id TEXT PRIMARY KEY, label TEXT NOT NULL, hidden_rank INTEGER NOT NULL)").execute(db.pool()).await.unwrap();
        sqlx::query(
            "INSERT INTO private_cursor_records VALUES ('one','First',17),('two','Second',29)",
        )
            .execute(db.pool())
            .await
            .unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let db = db
            .with_read_query_observer(Observe {
                reads: reads.clone(),
                revoke: None,
            })
            .with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 2, 4));
        let schema = schema_builder(db.clone()).finish();
        for operation in [
            "{ privateCursorRecordsKeyset(page: {limit: 1}) { edges { cursor } } }",
            "{ privateCursorRecordsScan(page: {limit: 1}) { pageInfo { continuation status } } }",
        ] {
            let response = schema
                .execute(Request::new(operation).data(AuthSubject::new("fixture-principal")))
                .await;
            assert!(
                !response.errors.is_empty(),
                "a hidden raw ordering value must be denied independently of selection"
            );
        }
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        // Trusted repository reads retain their ordinary independent field policy.
        let page = HiddenOrderRecord::keyset_page(&db, Default::default(), Default::default())
            .await
            .unwrap();
        assert_eq!(page.edges.len(), 2);
    }
}
