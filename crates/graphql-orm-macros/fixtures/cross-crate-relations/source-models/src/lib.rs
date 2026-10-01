use async_graphql::SimpleObject;
use cross_crate_target_models::{
    Endpoint, EndpointConnection, EndpointEdge, EndpointOrderByInput, EndpointWhereInput,
};
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity,
    GraphQLOperations,
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
    graphql_entity(backend = "sqlite", table = "cross_groups", plural = "Groups")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "cross_groups", plural = "Groups")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(backend = "mssql", table = "cross_groups", plural = "Groups")
)]
pub struct Group {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[graphql(skip)]
    pub tenant_id: String,
    #[graphql(skip)]
    #[relation(target = "Endpoint", from = ["tenant_id"], to = ["tenant_id"], multiple, emit_fk = false)]
    pub endpoints: Vec<Endpoint>,
}

#[cfg(all(test, feature = "postgres"))]
#[path = "../../../../../graphql-orm/tests/support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
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

    #[cfg(feature = "sqlite")]
    static TARGET_SELECTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    #[cfg(feature = "sqlite")]
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
        groups: Vec<Group>,
    }
    #[Object]
    impl Query {
        async fn sessions(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Session>> {
            let db = ctx.data_unchecked::<Database<DefaultBackend>>();
            db.ensure_entity_access(
                Some(ctx),
                "Session",
                None,
                EntityAccessKind::Read,
                EntityAccessSurface::GraphqlQuery,
            )
            .await?;
            Ok(self.sessions.clone())
        }
        async fn activities(&self, _ctx: &Context<'_>) -> Vec<Activity> {
            self.activities.clone()
        }
        async fn groups(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Group>> {
            let db = ctx.data_unchecked::<Database<DefaultBackend>>();
            db.ensure_entity_access(
                Some(ctx),
                "Group",
                None,
                EntityAccessKind::Read,
                EntityAccessSurface::GraphqlQuery,
            )
            .await?;
            Ok(self.groups.clone())
        }
    }
    struct TargetPolicy(Arc<AtomicBool>);
    impl EntityPolicy<DefaultBackend> for TargetPolicy {
        fn can_access_entity<'a>(
            &'a self,
            ctx: Option<&'a Context<'_>>,
            _db: &'a Database<DefaultBackend>,
            entity: &'static str,
            _key: Option<&'static str>,
            kind: EntityAccessKind,
            _surface: EntityAccessSurface,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async move {
                let identity = ctx.and_then(|ctx| ctx.data_opt::<Identity>());
                if entity == "Endpoint"
                    && identity.is_some_and(|identity| identity.owner == "entity-error")
                {
                    return Err(async_graphql::Error::new("private entity policy details"));
                }
                if identity.is_some_and(|identity| !identity.current) {
                    return Ok(false);
                }
                if entity != "Endpoint" && identity.is_some_and(|identity| !identity.source_allowed)
                {
                    return Ok(false);
                }
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

    #[derive(Clone)]
    struct Identity {
        owner: String,
        current: bool,
        source_allowed: bool,
        name_allowed: bool,
    }
    fn identity(owner: &str) -> Identity {
        Identity {
            owner: owner.into(),
            current: true,
            source_allowed: true,
            name_allowed: true,
        }
    }
    struct CurrentRowPolicy {
        allowed: Arc<AtomicBool>,
        complete: bool,
    }
    impl RowPolicy<DefaultBackend> for CurrentRowPolicy {
        fn read_visibility<'a>(
            &'a self,
            ctx: Option<&'a Context<'_>>,
            _: &'a Database<DefaultBackend>,
            entity: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>>
        {
            Box::pin(async move {
                if entity != "Endpoint" {
                    return Ok(ReadVisibility::Unrestricted);
                }
                if !self.complete {
                    return Ok(ReadVisibility::CallbackOnly);
                }
                let owner = &ctx.unwrap().data_unchecked::<Identity>().owner;
                let filter = cross_crate_target_models::EndpointWhereInput {
                    name: Some(StringFilter {
                        starts_with: Some(format!("{owner}:")),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                Ok(ReadVisibility::Complete(ReadPredicate::from_filter::<
                    DefaultBackend,
                    _,
                >(&filter)?))
            })
        }
        fn can_read_row<'a>(
            &'a self,
            ctx: Option<&'a Context<'_>>,
            _: &'a Database<DefaultBackend>,
            entity: &'static str,
            _: Option<&'static str>,
            surface: EntityAccessSurface,
            row: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async move {
                assert_eq!(surface, EntityAccessSurface::GraphqlRelation);
                if entity != "Endpoint" {
                    return Ok(true);
                }
                let row = row.downcast_ref::<Endpoint>().unwrap();
                if ctx.unwrap().data_unchecked::<Identity>().owner == "row-error" {
                    return Err(async_graphql::Error::new("private row policy details"));
                }
                let owner = &ctx.unwrap().data_unchecked::<Identity>().owner;
                Ok(self.allowed.load(Ordering::SeqCst)
                    && row.name.starts_with(&format!("{owner}:")))
            })
        }
        fn can_write_row<'a>(
            &'a self,
            _: Option<&'a Context<'_>>,
            _: &'a Database<DefaultBackend>,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
            _: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
    }
    struct CurrentFieldPolicy;
    impl FieldPolicy<DefaultBackend> for CurrentFieldPolicy {
        fn can_read_field<'a>(
            &'a self,
            ctx: &'a Context<'_>,
            _: &'a Database<DefaultBackend>,
            _: &'static str,
            field: &'static str,
            _: Option<&'static str>,
            _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async move {
                assert_eq!(ctx.field().name(), field);
                Ok(ctx.data_unchecked::<Identity>().name_allowed)
            })
        }
        fn can_write_field<'a>(
            &'a self,
            _: &'a Context<'_>,
            _: &'a Database<DefaultBackend>,
            _: &'static str,
            _: &'static str,
            _: Option<&'static str>,
            _: Option<&'a (dyn std::any::Any + Send + Sync)>,
            _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
    }

    #[cfg(feature = "sqlite")]
    type FixtureOwner = ();
    #[cfg(feature = "postgres")]
    type FixtureOwner = crate::owned_postgres::OwnedPostgres;
    async fn fixture_pool()
    -> Result<(<DefaultBackend as OrmBackend>::Pool, FixtureOwner), Box<dyn std::error::Error>>
    {
        #[cfg(feature = "sqlite")]
        {
            Ok((
                graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
                    .max_connections(1)
                    .connect("sqlite::memory:")
                    .await?,
                (),
            ))
        }
        #[cfg(feature = "postgres")]
        {
            let owner = FixtureOwner::start("relationship-authorization")?;
            let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&owner.url)
                .await?;
            Ok((pool, owner))
        }
    }
    fn cleanup_owner(owner: &mut FixtureOwner) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(feature = "postgres")]
        owner.cleanup()?;
        #[cfg(feature = "sqlite")]
        let _ = owner;
        Ok(())
    }
    async fn authorized_db(
        complete: bool,
        budget: u32,
    ) -> Result<
        (
            Database<DefaultBackend>,
            Arc<AtomicBool>,
            Arc<AtomicBool>,
            FixtureOwner,
        ),
        Box<dyn std::error::Error>,
    > {
        let (pool, owner) = fixture_pool().await?;
        let entity_allowed = Arc::new(AtomicBool::new(true));
        let row_allowed = Arc::new(AtomicBool::new(true));
        let mut db = Database::<DefaultBackend>::with_entity_policy(
            pool,
            TargetPolicy(entity_allowed.clone()),
        );
        let plan = db
            .schema()
            .plan_migration_to_entities("fixture", "owned target", &[Endpoint::metadata()])
            .await?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
        for (id, tenant, name) in [
            ("a", "one", "one:current A"),
            ("b", "one", "two:denied B"),
            ("c", "one", "one:current C"),
            ("d", "two", "two:current D"),
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
        db.set_row_policy(CurrentRowPolicy {
            allowed: row_allowed.clone(),
            complete,
        });
        db.set_field_policy(CurrentFieldPolicy);
        let db = db.with_authorized_scan_config(AuthorizedScanConfig::new([3; 32], 2, budget));
        Ok((db, entity_allowed, row_allowed, owner))
    }
    fn cached_schema(
        db: Database<DefaultBackend>,
        sessions: Vec<Session>,
        groups: Vec<Group>,
        loader: bool,
    ) -> Schema<Query, EmptyMutation, EmptySubscription> {
        let builder = Schema::build(
            Query {
                sessions,
                activities: vec![],
                groups,
            },
            EmptyMutation,
            EmptySubscription,
        )
        .data(db.clone())
        .data("fixture-user".to_owned());
        if loader {
            builder
                .data(DataLoader::new(
                    RelationLoader::<Endpoint, DefaultBackend>::new(db),
                    tokio::spawn,
                ))
                .finish()
        } else {
            builder.finish()
        }
    }
    fn preloaded(id: &str, tenant: &str, name: &str) -> Endpoint {
        Endpoint {
            id: id.into(),
            tenant_id: tenant.into(),
            name: name.into(),
        }
    }
    const CHILD: &str =
        "{ sessions { id link: endpoint { ...Target } } } fragment Target on Endpoint { id name }";

    #[tokio::test]
    async fn application_loader_cache_cannot_supply_target_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        use async_graphql::dataloader::HashMapCache;
        use graphql_orm::graphql::loaders::{
            CompositeRelationQueryKey, RelationKey, RelationKeyPartKind, RelationLoadResult,
        };
        let (db, allowed, _, mut owner) = authorized_db(false, 20).await?;
        let loader = DataLoader::with_cache(
            RelationLoader::<Endpoint, DefaultBackend>::new(db.clone()),
            tokio::spawn,
            HashMapCache::default(),
        );
        let values = vec![SqlValue::String("a".into()), SqlValue::String("one".into())];
        loader
            .feed_one(
                CompositeRelationQueryKey {
                    relation: "Session::endpoint",
                    parent_key: RelationKey::from_sql_values(&values),
                    parent_values: values,
                    fk_columns: vec!["\"id\"", "\"tenant_id\""],
                    key_part_kinds: vec![RelationKeyPartKind::String; 2],
                    where_signature: None,
                    order_signature: None,
                    page_signature: None,
                    filter: None,
                    sorts: vec![],
                    pagination: None,
                    auth_context: None,
                },
                RelationLoadResult {
                    entities: vec![preloaded("a", "two", "private cached target")],
                    total_count: 1,
                    has_next_page: false,
                    has_previous_page: false,
                    offset: 0,
                },
            )
            .await;
        let schema = Schema::build(
            Query {
                sessions: vec![session("parent", "a", "one")],
                activities: vec![],
                groups: vec![],
            },
            EmptyMutation,
            EmptySubscription,
        )
        .data(db.clone())
        .data("fixture-user".to_owned())
        .data(loader)
        .finish();
        let current = schema
            .execute(async_graphql::Request::new(CHILD).data(identity("one")))
            .await;
        assert!(current.errors.is_empty(), "{:?}", current.errors);
        assert_eq!(
            current.data.into_json()?["sessions"][0]["link"]["name"],
            "one:current A"
        );
        allowed.store(false, Ordering::SeqCst);
        let denied = schema
            .execute(async_graphql::Request::new(CHILD).data(identity("one")))
            .await;
        assert!(denied.data.into_json()?["sessions"][0]["link"].is_null());
        assert_eq!(denied.errors[0].message, "forbidden");
        db.pool().close().await;
        cleanup_owner(&mut owner)?;
        Ok(())
    }

    #[tokio::test]
    async fn generated_parent_does_not_eagerly_query_a_denied_nullable_target()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::atomic::AtomicUsize;
        struct Observe(Arc<AtomicUsize>);
        impl ReadQueryObserver for Observe {
            fn on_read(&self, sql: &str, _: usize) {
                if sql.contains("cross_endpoints") {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        let (pool, mut owner) = fixture_pool().await?;
        let reads = Arc::new(AtomicUsize::new(0));
        let db = Database::<DefaultBackend>::with_entity_policy(
            pool,
            TargetPolicy(Arc::new(AtomicBool::new(false))),
        )
        .with_read_query_observer(Observe(reads.clone()));
        // Intentionally create only the authorized source table. A pool-only
        // eager target preload used to query the missing table and fail parent.
        let plan = db
            .schema()
            .plan_migration_to_entities("source", "owned source", &[Session::metadata()])
            .await?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
        Session::insert(
            &db,
            CreateSessionInput {
                id: "parent".into(),
                endpoint_id: "a".into(),
                tenant_id: "one".into(),
            },
        )
        .await?;
        let schema = Schema::build(SessionQueries, EmptyMutation, EmptySubscription)
            .data(db.clone())
            .data("fixture-user".to_owned())
            .finish();
        let response = schema
            .execute("{ session(id: \"parent\") { id link: endpoint { id name } } }")
            .await;
        let data = response.data.into_json()?;
        assert_eq!(data["session"]["id"], "parent", "{:?}", response.errors);
        assert!(data["session"]["link"].is_null());
        assert_eq!(response.errors.len(), 1);
        assert_eq!(response.errors[0].message, "forbidden");
        assert_eq!(
            response.errors[0].path.last(),
            Some(&async_graphql::PathSegment::Field("link".into()))
        );
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        db.pool().close().await;
        cleanup_owner(&mut owner)?;
        Ok(())
    }

    #[tokio::test]
    async fn preloaded_targets_are_authoritative_owned_and_rechecked_on_every_request()
    -> Result<(), Box<dyn std::error::Error>> {
        for use_loader in [true, false] {
            let (db, entity_allowed, row_allowed, mut owner) = authorized_db(false, 20).await?;
            let mut parent = session("parent", "a", "one");
            parent.endpoint = Some(preloaded("forged", "two", "private forged name"));
            let mut mismatch = session("mismatch", "a", "two");
            mismatch.endpoint = Some(preloaded("a", "one", "cached cross-tenant value"));
            let mut missing = session("missing", "absent", "one");
            missing.endpoint = Some(preloaded("absent", "one", "fabricated missing value"));
            let schema = cached_schema(
                db.clone(),
                vec![parent, mismatch, missing],
                vec![],
                use_loader,
            );
            let response = schema
                .execute(async_graphql::Request::new(CHILD).data(identity("one")))
                .await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let data = response.data.into_json()?;
            assert_eq!(data["sessions"][0]["link"]["name"], "one:current A");
            assert!(data["sessions"][1]["link"].is_null());
            assert!(data["sessions"][2]["link"].is_null());

            for policy in [&entity_allowed, &row_allowed] {
                policy.store(false, Ordering::SeqCst);
                let denied = schema
                    .execute(async_graphql::Request::new(CHILD).data(identity("one")))
                    .await;
                assert_eq!(denied.data.into_json()?["sessions"][0]["id"], "parent");
                assert!(!denied.errors.is_empty());
                assert!(
                    denied
                        .errors
                        .iter()
                        .all(|error| error.message == "forbidden"
                            && error.path.last()
                                == Some(&async_graphql::PathSegment::Field("link".into())))
                );
                policy.store(true, Ordering::SeqCst);
            }
            let denied_identity = schema
                .execute(async_graphql::Request::new(CHILD).data(identity("two")))
                .await;
            assert!(denied_identity.data.into_json()?["sessions"][0]["link"].is_null());
            assert_eq!(denied_identity.errors.len(), 1);

            let mut field_denied = identity("one");
            field_denied.name_allowed = false;
            let denied = schema
                .execute(async_graphql::Request::new(CHILD).data(field_denied))
                .await;
            let data = denied.data.into_json()?;
            assert_eq!(data["sessions"][0]["id"], "parent");
            assert!(
                data["sessions"][0]["link"].is_null(),
                "field denial response: {data}; errors={:?}",
                denied.errors
            );
            assert_eq!(denied.errors[0].message, "forbidden");
            assert_eq!(
                denied.errors[0].path.last(),
                Some(&async_graphql::PathSegment::Field("link".into()))
            );

            for selection in [
                "{ sessions { link: endpoint { id hidden: name @skip(if: true) } } }",
                "query($skip: Boolean!) { sessions { link: endpoint { id ...Secret @skip(if: $skip) } } } fragment Secret on Endpoint { name }",
                "query($include: Boolean!) { sessions { link: endpoint { id ... @include(if: $include) { name } } } }",
            ] {
                let mut identity = identity("one");
                identity.name_allowed = false;
                let skipped = schema
                    .execute(
                        async_graphql::Request::new(selection)
                            .variables(async_graphql::Variables::from_json(
                                serde_json::json!({"skip": true, "include": false}),
                            ))
                            .data(identity),
                    )
                    .await;
                assert!(
                    skipped.errors.is_empty(),
                    "skipped field must not be denied: {selection}; {:?}",
                    skipped.errors
                );
                assert_eq!(skipped.data.into_json()?["sessions"][0]["link"]["id"], "a");
            }

            for owner in ["entity-error", "row-error"] {
                let denied = schema
                    .execute(async_graphql::Request::new(CHILD).data(identity(owner)))
                    .await;
                assert_eq!(denied.data.into_json()?["sessions"][0]["id"], "parent");
                assert!(
                    denied
                        .errors
                        .iter()
                        .all(|error| error.message == "forbidden"),
                    "policy provider diagnostics must stay private"
                );
            }

            // Reuse the schema/loader and all preloaded snapshots after ownership changes.
            Endpoint::update_by_id(
                &db,
                &"a".to_owned(),
                cross_crate_target_models::UpdateEndpointInput {
                    tenant_id: Some("two".into()),
                    name: Some("two:reassigned".into()),
                },
            )
            .await?;
            let moved = schema
                .execute(async_graphql::Request::new(CHILD).data(identity("one")))
                .await;
            assert!(moved.data.into_json()?["sessions"][0]["link"].is_null());
            let current = schema
                .execute(async_graphql::Request::new(CHILD).data(identity("two")))
                .await;
            assert_eq!(
                current.data.into_json()?["sessions"][1]["link"]["name"],
                "two:reassigned"
            );

            let mut expired = identity("one");
            expired.current = false;
            assert!(
                !schema
                    .execute(async_graphql::Request::new(CHILD).data(expired))
                    .await
                    .errors
                    .is_empty()
            );
            let mut denied_source = identity("one");
            denied_source.source_allowed = false;
            assert!(
                !schema
                    .execute(async_graphql::Request::new(CHILD).data(denied_source))
                    .await
                    .errors
                    .is_empty()
            );
            db.pool().close().await;
            cleanup_owner(&mut owner)?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn preloaded_many_relations_authorize_before_pages_and_counts()
    -> Result<(), Box<dyn std::error::Error>> {
        for complete in [false, true] {
            let (db, _, _, mut owner) = authorized_db(complete, 20).await?;
            let groups = vec![Group {
                id: "group".into(),
                tenant_id: "one".into(),
                endpoints: vec![preloaded("d", "two", "cached other tenant")],
            }];
            let schema = cached_schema(db.clone(), vec![], groups, true);
            let response = schema.execute(async_graphql::Request::new("{ groups { id links: endpoints(page: {limit: 1, offset: 1}) { edges { node { id name } } pageInfo { totalCount hasNextPage hasPreviousPage } } } }").data(identity("one"))).await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let data = response.data.into_json()?;
            assert_eq!(data["groups"][0]["links"]["edges"][0]["node"]["id"], "c");
            assert_eq!(data["groups"][0]["links"]["pageInfo"]["totalCount"], 2);
            assert_eq!(data["groups"][0]["links"]["pageInfo"]["hasNextPage"], false);
            let all = schema.execute(async_graphql::Request::new("{ groups { endpoints { edges { node { id } } pageInfo { totalCount } } } }").data(identity("one"))).await;
            assert!(all.errors.is_empty(), "{:?}", all.errors);
            assert_eq!(
                all.data.into_json()?["groups"][0]["endpoints"]["edges"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            db.pool().close().await;
            cleanup_owner(&mut owner)?;
        }
        let (db, _, _, mut owner) = authorized_db(false, 2).await?;
        let schema = cached_schema(
            db.clone(),
            vec![],
            vec![Group {
                id: "group".into(),
                tenant_id: "one".into(),
                endpoints: vec![],
            }],
            true,
        );
        let budget = schema
            .execute(
                async_graphql::Request::new("{ groups { endpoints { pageInfo { totalCount } } } }")
                    .data(identity("one")),
            )
            .await;
        assert!(
            !budget.errors.is_empty(),
            "a partial scan must not publish count/exhaustion"
        );
        assert_eq!(budget.errors[0].message, "authorization is misconfigured");
        db.pool().close().await;
        cleanup_owner(&mut owner)?;
        Ok(())
    }

    #[tokio::test]
    async fn cross_crate_execution_ownership_batching_and_nullable_entity_denial()
    -> Result<(), Box<dyn std::error::Error>> {
        let (pool, mut owner) = fixture_pool().await?;
        let allowed = Arc::new(AtomicBool::new(true));
        let db =
            Database::<DefaultBackend>::with_entity_policy(pool, TargetPolicy(allowed.clone()));
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
                groups: vec![],
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
            RelationLoader::<Endpoint, DefaultBackend>::new(db),
            tokio::spawn,
        ))
        .finish();
        #[cfg(feature = "sqlite")]
        {
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
        }
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
        #[cfg(feature = "sqlite")]
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
        cleanup_owner(&mut owner)?;
        Ok(())
    }
    #[tokio::test]
    async fn preloaded_nullable_target_cannot_bypass_current_entity_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let (pool, mut owner) = fixture_pool().await?;
        let db = Database::<DefaultBackend>::with_entity_policy(
            pool,
            TargetPolicy(Arc::new(AtomicBool::new(false))),
        );
        let mut parent = session("parent", "authorized-key", "one");
        parent.endpoint = Some(Endpoint {
            id: "forged-key".into(),
            tenant_id: "two".into(),
            name: "private preloaded target".into(),
        });
        let db_pool = db.pool().clone();
        let schema = Schema::build(
            Query {
                sessions: vec![parent],
                activities: vec![],
                groups: vec![],
            },
            EmptyMutation,
            EmptySubscription,
        )
        .data(db.clone())
        .data("fixture-user".to_owned())
        .data(DataLoader::new(
            RelationLoader::<Endpoint, DefaultBackend>::new(db),
            tokio::spawn,
        ))
        .finish();
        let response = schema.execute("{ sessions { id link: endpoint { ...Target } } } fragment Target on Endpoint { id name }").await;
        let data = response.data.into_json()?;
        assert_eq!(data["sessions"][0]["id"], "parent");
        assert!(
            data["sessions"][0]["link"].is_null(),
            "preloaded object bypassed current target policy: {data}"
        );
        assert_eq!(response.errors.len(), 1);
        assert_eq!(
            response.errors[0].path.last(),
            Some(&async_graphql::PathSegment::Field("link".into()))
        );
        db_pool.close().await;
        cleanup_owner(&mut owner)?;
        Ok(())
    }
}
