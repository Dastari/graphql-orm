//! Executes against memory SQLite or an independently owned PostgreSQL container.
#![cfg(any(feature = "sqlite", feature = "postgres"))]
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "private_issuers",
        plural = "PrivateIssuers",
        schema_policy = "external_read_only",
        read_policy = "issuers.read"
    )
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "private_issuers",
        plural = "PrivateIssuers",
        schema_policy = "external_read_only",
        read_policy = "issuers.read"
    )
)]
#[graphql_orm(projection(name = "PublicIssuer", fields = [id, tenant, certificate_pem], private = true))]
#[graphql_orm(projection(name = "CustodyIssuer", fields = [id, encrypted_private_key], private = true))]
struct PrivateIssuer {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    tenant: String,
    certificate_pem: String,
    #[graphql_orm(private, sensitive, read_policy = "custody.read")]
    encrypted_private_key: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(backend = "sqlite", table = "other_source", plural = "OtherSources")
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(backend = "postgres", table = "other_source", plural = "OtherSources")
)]
struct OtherSource {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    id: String,
}

#[derive(Clone, Copy)]
enum Mode {
    Unrestricted,
    Complete,
    CallbackOnly,
    Prefilter,
    WrongEntity,
}
#[derive(Clone)]
struct Visibility {
    mode: Arc<Mutex<Mode>>,
    tenant: Arc<Mutex<String>>,
}
impl RowPolicy<Backend> for Visibility {
    fn read_visibility<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<Backend>,
        entity: &'static str,
        policy: Option<&'static str>,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        assert_eq!(entity, "PrivateIssuer");
        assert_eq!(policy, Some("issuers.read"));
        assert_eq!(surface, EntityAccessSurface::Repository);
        let mode = *self.mode.lock().unwrap();
        let tenant = self.tenant.lock().unwrap().clone();
        Box::pin(async move {
            Ok(match mode {
                Mode::Unrestricted => ReadVisibility::Unrestricted,
                Mode::CallbackOnly => ReadVisibility::CallbackOnly,
                Mode::WrongEntity => {
                    ReadVisibility::Complete(ReadPredicate::from_filter::<Backend, _>(
                        &OtherSourceWhereInput {
                            id: Some(StringFilter {
                                eq: Some(tenant),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )?)
                }
                Mode::Complete | Mode::Prefilter => {
                    let predicate =
                        ReadPredicate::from_filter::<Backend, _>(&PrivateIssuerWhereInput {
                            tenant: Some(StringFilter {
                                eq: Some(tenant),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })?;
                    if matches!(mode, Mode::Complete) {
                        ReadVisibility::Complete(predicate)
                    } else {
                        ReadVisibility::Prefilter(predicate)
                    }
                }
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _policy: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async {
            panic!("projection or group must never load a full entity for a residual callback")
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _policy: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}
struct EntityAccess(bool);
impl EntityPolicy<Backend> for EntityAccess {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _policy: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(self.0) })
    }
}
#[derive(Clone, Default)]
struct Reads(Arc<Mutex<Vec<String>>>);
impl ReadQueryObserver for Reads {
    fn on_read(&self, sql: &str, _rows: usize) {
        self.0.lock().unwrap().push(sql.into());
    }
}
struct Fixture {
    db: Database<Backend>,
    visibility: Visibility,
    reads: Reads,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
impl Fixture {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(feature = "sqlite")]
        let mut db = Database::<Backend>::connect_sqlite("sqlite::memory:").await?;
        #[cfg(feature = "postgres")]
        let mut owned = owned_postgres::OwnedPostgres::start("projection-visibility")?;
        #[cfg(feature = "postgres")]
        let mut db = match Database::<Backend>::connect_postgres(&owned.url).await {
            Ok(db) => db,
            Err(error) => {
                owned.cleanup()?;
                return Err(error.into());
            }
        };
        sqlx::query("CREATE TABLE issuer_base(id TEXT PRIMARY KEY, tenant TEXT NOT NULL, certificate_pem TEXT NOT NULL)").execute(db.pool()).await?;
        sqlx::query("INSERT INTO issuer_base VALUES ('a','beta','PUBLIC-B'), ('b','alpha','PUBLIC-A'), ('c','alpha','PUBLIC-C')").execute(db.pool()).await?;
        #[cfg(feature = "sqlite")]
        sqlx::query("CREATE VIEW private_issuers AS SELECT id,tenant,certificate_pem,json_extract('invalid','$') AS encrypted_private_key FROM issuer_base").execute(db.pool()).await?;
        #[cfg(feature = "postgres")]
        {
            sqlx::query("CREATE FUNCTION forbidden_secret() RETURNS TEXT LANGUAGE plpgsql STABLE AS $$ BEGIN RAISE EXCEPTION 'private material fetched'; END; $$").execute(db.pool()).await?;
            sqlx::query("CREATE VIEW private_issuers AS SELECT id,tenant,certificate_pem,forbidden_secret() AS encrypted_private_key FROM issuer_base").execute(db.pool()).await?;
        }
        // An oracle that proves fetching the excluded expression really fails.
        assert!(
            sqlx::query("SELECT encrypted_private_key FROM private_issuers")
                .fetch_all(db.pool())
                .await
                .is_err()
        );
        let visibility = Visibility {
            mode: Arc::new(Mutex::new(Mode::Unrestricted)),
            tenant: Arc::new(Mutex::new("alpha".into())),
        };
        db.set_entity_policy(EntityAccess(true));
        db.set_row_policy(visibility.clone());
        db.set_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
        let reads = Reads::default();
        let db = db.with_read_query_observer(reads.clone());
        Ok(Self {
            db,
            visibility,
            reads,
            #[cfg(feature = "postgres")]
            owned,
        })
    }
    async fn close(self) -> TestResult {
        self.db.pool().close().await;
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup()?;
        }
        Ok(())
    }
}

#[tokio::test]
async fn global_unrestricted_and_complete_visibility_project_without_private_material() -> TestResult
{
    let fixture = Fixture::start().await?;
    let db = &fixture.db;
    let rows = PublicIssuer::query(db).limit(2).fetch_all().await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "a");
    assert_eq!(
        PublicIssuer::query(db).fetch_first().await?.unwrap().id,
        "a"
    );
    assert!(PublicIssuer::query(db).fetch_optional_one().await.is_err());
    *fixture.visibility.mode.lock().unwrap() = Mode::Complete;
    let rows = PublicIssuer::query(db).limit(1).fetch_all().await?;
    assert_eq!(
        rows[0].id, "b",
        "authorization precedes LIMIT, even when an unauthorized row sorts first"
    );
    assert_eq!(
        PublicIssuer::query(db).fetch_first().await?.unwrap().id,
        "b"
    );
    assert!(PublicIssuer::find_by_id(db, &"a".into()).await?.is_none());
    let row = PublicIssuer::find_by_id(db, &"b".into()).await?.unwrap();
    assert_eq!(row.certificate_pem, "PUBLIC-A");
    let tx_rows = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                assert!(
                    PublicIssuer::find_by_id_in(tx, &"a".into())
                        .await
                        .map_err(OrmPublicError::from)?
                        .is_none()
                );
                assert_eq!(
                    tx.project::<PublicIssuer>()
                        .fetch_first()
                        .await
                        .map_err(OrmPublicError::from)?
                        .unwrap()
                        .id,
                    "b"
                );
                assert!(
                    tx.project::<PublicIssuer>()
                        .fetch_optional_one()
                        .await
                        .is_err()
                );
                tx.project::<PublicIssuer>()
                    .limit(1)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?;
    assert_eq!(tx_rows[0].id, "b");
    *fixture.visibility.tenant.lock().unwrap() = "beta".into();
    assert!(
        PublicIssuer::find_by_id(db, &"b".into()).await?.is_none(),
        "previously returned values confer no authority"
    );
    assert_eq!(
        PublicIssuer::query(db)
            .fetch_optional_one()
            .await?
            .unwrap()
            .id,
        "a"
    );
    let reads = fixture.reads.0.lock().unwrap().clone();
    assert_eq!(
        reads.len(),
        13,
        "one bounded SELECT per projection operation including transaction reads"
    );
    assert!(
        reads
            .iter()
            .all(|sql| !sql.contains("encrypted_private_key") && !sql.contains("SELECT *"))
    );
    fixture.close().await
}

#[tokio::test]
async fn residual_wrong_entity_entity_denial_and_selected_field_denial_fail_before_io() -> TestResult
{
    let mut fixture = Fixture::start().await?;
    for mode in [Mode::CallbackOnly, Mode::Prefilter, Mode::WrongEntity] {
        *fixture.visibility.mode.lock().unwrap() = mode;
        assert!(PublicIssuer::query(&fixture.db).fetch_all().await.is_err());
        assert!(
            PublicIssuer::find_by_id(&fixture.db, &"b".into())
                .await
                .is_err()
        );
        fixture
            .db
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    assert!(tx.project::<PublicIssuer>().fetch_first().await.is_err());
                    Ok(())
                })
            })
            .await?;
    }
    *fixture.visibility.mode.lock().unwrap() = Mode::Unrestricted;
    assert!(
        CustodyIssuer::query(&fixture.db).fetch_all().await.is_err(),
        "declared selected field policy has no provider"
    );
    fixture.db.set_entity_policy(EntityAccess(false));
    assert!(
        PublicIssuer::query(&fixture.db)
            .fetch_first()
            .await
            .is_err()
    );
    assert!(fixture.reads.0.lock().unwrap().is_empty());
    fixture.close().await
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn group_pages_under_global_provider_use_only_authorized_source_rows() -> TestResult {
    let fixture = Fixture::start().await?;
    let options = AggregateGroupPageOptions {
        order: AggregateGroupOrder::Binary,
        exclude_blank: true,
        context: "trusted-public-revision".into(),
    };
    let page = PrivateIssuer::aggregate(&fixture.db)
        .group_by(PrivateIssuerAggregateField::Tenant)?
        .count_rows()?
        .fetch_group_page(options.clone(), None)
        .await?;
    assert_eq!(page.rows.len(), 2);
    *fixture.visibility.mode.lock().unwrap() = Mode::Complete;
    let page = PrivateIssuer::aggregate(&fixture.db)
        .group_by(PrivateIssuerAggregateField::Tenant)?
        .count_rows()?
        .fetch_group_page(options.clone(), None)
        .await?;
    assert_eq!(page.rows.len(), 1);
    assert_eq!(
        page.rows[0].groups[0].value,
        AggregateValue::Text("alpha".into())
    );
    assert_eq!(page.rows[0].metrics[0].value, AggregateValue::Count(2));
    for mode in [Mode::CallbackOnly, Mode::Prefilter, Mode::WrongEntity] {
        *fixture.visibility.mode.lock().unwrap() = mode;
        assert!(
            PrivateIssuer::aggregate(&fixture.db)
                .group_by(PrivateIssuerAggregateField::Tenant)?
                .count_rows()?
                .fetch_group_page(options.clone(), None)
                .await
                .is_err()
        );
    }
    assert_eq!(fixture.reads.0.lock().unwrap().len(), 2);
    fixture.close().await
}

// Existing external implementations need no new required method or 'static Entity bound.
#[derive(Clone)]
struct LegacyPublicIssuer(PublicIssuer);
impl FromSqlRow<Backend> for LegacyPublicIssuer {
    fn from_row(row: &<Backend as OrmBackend>::Row) -> graphql_orm::Result<Self> {
        PublicIssuer::from_row(row).map(Self)
    }
}
impl ReadProjection<Backend> for LegacyPublicIssuer {
    type Entity = PrivateIssuer;
    type Filter = PrivateIssuerWhereInput;
    type Order = PrivateIssuerOrderByInput;
    const COLUMNS: &'static [&'static str] = <PublicIssuer as ReadProjection<Backend>>::COLUMNS;
    const FIELD_NAMES: &'static [&'static str] =
        <PublicIssuer as ReadProjection<Backend>>::FIELD_NAMES;
    const UNIQUE_COLUMNS: &'static [&'static str] =
        <PublicIssuer as ReadProjection<Backend>>::UNIQUE_COLUMNS;
}

#[tokio::test]
async fn legacy_projection_implementation_stays_compatible_and_missing_identity_is_fail_closed()
-> TestResult {
    let fixture = Fixture::start().await?;
    let rows = ProjectionQuery::<LegacyPublicIssuer, Backend>::new(&fixture.db)
        .limit(1)
        .fetch_all()
        .await?;
    assert_eq!(rows[0].0.id, "a");
    *fixture.visibility.mode.lock().unwrap() = Mode::Complete;
    assert!(
        ProjectionQuery::<LegacyPublicIssuer, Backend>::new(&fixture.db)
            .fetch_first()
            .await
            .is_err()
    );
    assert_eq!(fixture.reads.0.lock().unwrap().len(), 1);
    fixture.close().await
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn current_sql_visibility_intersects_rls_and_preserves_pool_and_pinned_auth() -> TestResult {
    use std::str::FromStr;
    let fixture = Fixture::start().await?;
    for ddl in [
        "ALTER TABLE issuer_base ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE issuer_base FORCE ROW LEVEL SECURITY",
        "CREATE POLICY issuer_partition ON issuer_base FOR SELECT USING (tenant = current_setting('app.tenant_id', true))",
        "ALTER VIEW private_issuers SET (security_invoker = true)",
        "CREATE ROLE projection_reader LOGIN PASSWORD 'owned-fixture-only'",
        "GRANT USAGE ON SCHEMA public TO projection_reader",
        "GRANT SELECT ON issuer_base, private_issuers TO projection_reader",
    ] {
        sqlx::query(ddl).execute(fixture.db.pool()).await?;
    }
    let options = sqlx::postgres::PgConnectOptions::from_str(&fixture.owned.url)?
        .username("projection_reader")
        .password("owned-fixture-only");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await?;
    let mut reader = Database::<Backend>::new(pool.clone());
    reader.set_entity_policy(EntityAccess(true));
    reader.set_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
    reader.set_row_policy(fixture.visibility.clone());
    assert!(PublicIssuer::query(&reader).fetch_all().await?.is_empty());
    let mut auth = DbAuthContext {
        tenant_id: Some("beta".into()),
        ..Default::default()
    };
    assert_eq!(
        PublicIssuer::query_with_auth(&reader, Some(&auth))
            .fetch_first()
            .await?
            .unwrap()
            .id,
        "a"
    );
    *fixture.visibility.mode.lock().unwrap() = Mode::Complete;
    assert!(
        PublicIssuer::query_with_auth(&reader, Some(&auth))
            .fetch_all()
            .await?
            .is_empty(),
        "SQL row policy does not override RLS"
    );
    auth.tenant_id = Some("alpha".into());
    let public = reader
        .transaction_with_auth(TransactionMode::Default, Some(&auth), |tx| {
            Box::pin(async move {
                tx.project::<PublicIssuer>()
                    .limit(1)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?;
    assert_eq!(public.len(), 1);
    assert_eq!(public[0].id, "b");
    assert!(
        PublicIssuer::query(&reader).fetch_all().await?.is_empty(),
        "auth settings do not leak after commit"
    );
    pool.close().await;
    fixture.close().await
}
