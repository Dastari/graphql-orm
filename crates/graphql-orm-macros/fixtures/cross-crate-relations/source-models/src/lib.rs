use async_graphql::SimpleObject;
use cross_crate_target_models::Endpoint;
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "cross_sessions",
        plural = "Sessions",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "cross_sessions",
        plural = "Sessions",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "cross_sessions",
        plural = "Sessions",
        default_sort = "id ASC"
    )
)]
pub struct Session {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    pub endpoint_id: String,
    #[graphql(skip)]
    pub tenant_id: String,
    #[graphql(skip)]
    #[relation(target = "Endpoint", from = ["endpoint_id", "tenant_id"], to = ["id", "tenant_id"], emit_fk = false)]
    pub endpoint: Option<Endpoint>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "cross_activity",
        plural = "Activities",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "cross_activity",
        plural = "Activities",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "cross_activity",
        plural = "Activities",
        default_sort = "id ASC"
    )
)]
pub struct Activity {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    pub endpoint_id: Option<String>,
    #[graphql(skip)]
    pub tenant_id: Option<String>,
    pub endpoint_name: Option<String>,
    #[graphql(skip)]
    #[relation(target = "Endpoint", from = ["endpoint_id", "tenant_id"], to = ["id", "tenant_id"], emit_fk = false)]
    pub endpoint: Option<Endpoint>,
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use async_graphql::dataloader::DataLoader;
    use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema};
    use graphql_orm::db::Database;
    use graphql_orm::graphql::loaders::RelationLoader;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    static TARGET_SELECTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    unsafe extern "C" fn trace(
        kind: u32,
        _context: *mut std::ffi::c_void,
        statement: *mut std::ffi::c_void,
        _sql: *mut std::ffi::c_void,
    ) -> i32 {
        if kind == libsqlite3_sys::SQLITE_TRACE_STMT as u32 {
            // SQLite owns the statement and guarantees its lifetime for this callback.
            let sql = unsafe { libsqlite3_sys::sqlite3_sql(statement.cast()) };
            if !sql.is_null() {
                let text = unsafe { std::ffi::CStr::from_ptr(sql) }.to_string_lossy();
                if text.starts_with("SELECT ") && text.contains("cross_endpoints") {
                    TARGET_SELECTS.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        0
    }

    struct Query {
        sessions: Vec<Session>,
        activities: Vec<Activity>,
    }
    #[Object]
    impl Query {
        async fn sessions(&self, _ctx: &Context<'_>) -> Vec<Session> {
            self.sessions.clone()
        }
        async fn activities(&self, _ctx: &Context<'_>) -> Vec<Activity> {
            self.activities.clone()
        }
    }
    struct TargetPolicy(Arc<AtomicBool>);
    impl EntityPolicy<SqliteBackend> for TargetPolicy {
        fn can_access_entity<'a>(
            &'a self,
            _ctx: Option<&'a Context<'_>>,
            _db: &'a Database<SqliteBackend>,
            entity: &'static str,
            _key: Option<&'static str>,
            kind: EntityAccessKind,
            _surface: EntityAccessSurface,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async move {
                Ok(entity != "Endpoint"
                    || kind != EntityAccessKind::Read
                    || self.0.load(Ordering::SeqCst))
            })
        }
    }
    fn session(id: &str, endpoint: &str, tenant: &str) -> Session {
        Session {
            id: id.into(),
            endpoint_id: endpoint.into(),
            tenant_id: tenant.into(),
            endpoint: None,
        }
    }
    fn activity(id: &str, endpoint: Option<&str>, tenant: Option<&str>) -> Activity {
        Activity {
            id: id.into(),
            endpoint_id: endpoint.map(str::to_owned),
            tenant_id: tenant.map(str::to_owned),
            endpoint_name: Some("historical name".into()),
            endpoint: None,
        }
    }

    #[tokio::test]
    async fn cross_crate_execution_ownership_batching_and_nullable_entity_denial()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        let allowed = Arc::new(AtomicBool::new(true));
        let db = Database::<SqliteBackend>::with_entity_policy(pool, TargetPolicy(allowed.clone()));
        let plan = db
            .schema()
            .plan_migration_to_entities("fixture", "synthetic target", &[Endpoint::metadata()])
            .await?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
        for (id, tenant, name) in [
            ("a", "one", "Current A"),
            ("b", "one", "Current B"),
            ("moved", "two", "Other tenant"),
        ] {
            Endpoint::insert(
                &db,
                cross_crate_target_models::CreateEndpointInput {
                    id: id.into(),
                    tenant_id: tenant.into(),
                    name: name.into(),
                },
            )
            .await?;
        }
        let db_pool = db.pool().clone();
        let schema = Schema::build(
            Query {
                sessions: vec![
                    session("s1", "a", "one"),
                    session("s2", "b", "one"),
                    session("s3", "moved", "one"),
                    session("s4", "missing", "one"),
                ],
                activities: vec![
                    activity("h1", Some("a"), Some("one")),
                    activity("h2", None, Some("one")),
                    activity("h3", Some("a"), None),
                ],
            },
            EmptyMutation,
            EmptySubscription,
        )
        .data(db.clone())
        .data("fixture-user".to_owned())
        .data(DataLoader::new(
            RelationLoader::<Endpoint, SqliteBackend>::new(db),
            tokio::spawn,
        ))
        .finish();
        let mut connection = db_pool.acquire().await?;
        {
            let mut handle = connection.lock_handle().await?;
            // The driver is paused while installing a native execution trace.
            // No application query path calls SQLite FFI or handwritten SQL.
            let result = unsafe {
                libsqlite3_sys::sqlite3_trace_v2(
                    handle.as_raw_handle().as_ptr(),
                    libsqlite3_sys::SQLITE_TRACE_STMT as u32,
                    Some(trace),
                    std::ptr::null_mut(),
                )
            };
            assert_eq!(result, libsqlite3_sys::SQLITE_OK);
        }
        drop(connection);
        TARGET_SELECTS.store(0, Ordering::SeqCst);
        let response = schema.execute("{ sessions { id endpoint { id name } } activities { id endpointName endpoint { id name } } }").await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json()?;
        assert_eq!(data["sessions"][0]["endpoint"]["name"], "Current A");
        assert_eq!(data["sessions"][1]["endpoint"]["name"], "Current B");
        assert!(
            data["sessions"][2]["endpoint"].is_null(),
            "tenant reassignment cannot grant access"
        );
        assert!(data["sessions"][3]["endpoint"].is_null());
        assert_eq!(data["activities"][0]["endpointName"], "historical name");
        assert!(data["activities"][1]["endpoint"].is_null());
        assert!(data["activities"][2]["endpoint"].is_null());
        assert_eq!(
            TARGET_SELECTS.load(Ordering::SeqCst),
            2,
            "one bounded target dispatch per source relation"
        );
        assert!(
            !schema.sdl().contains("tenantId:"),
            "redacted target exposes no ownership storage field"
        );

        allowed.store(false, Ordering::SeqCst);
        let denied = schema
            .execute("{ sessions { id endpoint { id name } } }")
            .await;
        let denied_data = denied.data.into_json()?;
        assert_eq!(denied_data["sessions"][0]["id"], "s1");
        assert!(denied_data["sessions"][0]["endpoint"].is_null());
        assert!(!denied.errors.is_empty());
        assert!(denied.errors.iter().all(|error| error.path.last()
            == Some(&async_graphql::PathSegment::Field("endpoint".into()))));
        assert!(
            denied
                .errors
                .iter()
                .all(|error| !error.message.contains("endpoint.read"))
        );
        db_pool.close().await;
        Ok(())
    }
}
