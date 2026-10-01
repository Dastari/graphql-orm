//! Safe public projections and group pages with a globally installed row policy.
//! Run with --no-default-features --features sqlite --example policy_projections.
#[cfg(feature = "sqlite")]
pub mod example {
    use graphql_orm::{async_graphql, prelude::*};
    #[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
    #[repository_entity(
        backend = "sqlite",
        table = "private_issuers",
        plural = "PrivateIssuers"
    )]
    #[graphql_orm(projection(name = "PublicIssuer", fields = [id, tenant, certificate_pem], private = true))]
    pub struct PrivateIssuer {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        id: String,
        #[filterable(type = "string")]
        tenant: String,
        certificate_pem: String,
        #[graphql_orm(private, sensitive)]
        encrypted_private_key: String,
    }
    enum Visibility {
        Complete { verified_tenant: String },
        Unrestricted,
        Residual,
    }
    impl RowPolicy<SqliteBackend> for Visibility {
        fn read_visibility<'a>(
            &'a self,
            _ctx: Option<&'a async_graphql::Context<'_>>,
            _db: &'a Database<SqliteBackend>,
            entity: &'static str,
            _policy: Option<&'static str>,
            surface: EntityAccessSurface,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>>
        {
            assert_eq!(surface, EntityAccessSurface::Repository);
            Box::pin(async move {
                // A global provider must explicitly decide for each model; unknown models fail closed.
                if entity != "PrivateIssuer" {
                    return Ok(ReadVisibility::CallbackOnly);
                }
                Ok(match self {
                    Self::Unrestricted => ReadVisibility::Unrestricted,
                    Self::Residual => ReadVisibility::CallbackOnly,
                    Self::Complete { verified_tenant } => ReadVisibility::Complete(
                        ReadPredicate::from_filter::<SqliteBackend, _>(&PrivateIssuerWhereInput {
                            tenant: Some(StringFilter {
                                eq: Some(verified_tenant.clone()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })?,
                    ),
                })
            })
        }
        fn can_read_row<'a>(
            &'a self,
            _ctx: Option<&'a async_graphql::Context<'_>>,
            _db: &'a Database<SqliteBackend>,
            _entity: &'static str,
            _policy: Option<&'static str>,
            _surface: EntityAccessSurface,
            _row: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(false) })
        }
        fn can_write_row<'a>(
            &'a self,
            _ctx: Option<&'a async_graphql::Context<'_>>,
            _db: &'a Database<SqliteBackend>,
            _entity: &'static str,
            _policy: Option<&'static str>,
            _surface: EntityAccessSurface,
            _row: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(false) })
        }
    }
    pub async fn run() -> graphql_orm::Result<()> {
        let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
        // The example owns its synthetic database. External-read-only models do not use this setup.
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "example",
                "owned issuer fixture",
                &[PrivateIssuer::metadata()],
            )
            .await?;
        db.schema()
            .apply_migration(&plan, Default::default())
            .await?;
        for (id, tenant) in [("a", "beta"), ("b", "alpha"), ("c", "alpha")] {
            PrivateIssuer::insert(
                &db,
                CreatePrivateIssuerInput {
                    id: id.into(),
                    tenant: tenant.into(),
                    certificate_pem: format!("PUBLIC-{id}"),
                    encrypted_private_key: "synthetic-custody".into(),
                },
            )
            .await?;
        }
        db.set_row_policy(Visibility::Complete {
            verified_tenant: "alpha".into(),
        });
        // No full entity, excluded private-key column, application query SQL, or GraphQL roots.
        let public = PublicIssuer::query(&db).limit(1).fetch_all().await?;
        assert_eq!(public[0].id, "b");
        assert!(PublicIssuer::find_by_id(&db, &"a".into()).await?.is_none());
        let options = AggregateGroupPageOptions {
            order: AggregateGroupOrder::Binary,
            exclude_blank: true,
            context: "verified-tenant-alpha:public-revision-1".into(),
        };
        let page = PrivateIssuer::aggregate(&db)
            .group_by(PrivateIssuerAggregateField::Tenant)?
            .count_rows()?
            .fetch_group_page(options.clone(), None)
            .await?;
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].metrics[0].value, AggregateValue::Count(2));
        // An intentionally unrestricted model remains usable under an installed global provider.
        db.set_row_policy(Visibility::Unrestricted);
        assert_eq!(
            PublicIssuer::query(&db).limit(10).fetch_all().await?.len(),
            3
        );
        assert_eq!(
            PrivateIssuer::aggregate(&db)
                .group_by(PrivateIssuerAggregateField::Tenant)?
                .fetch_group_page(options.clone(), None)
                .await?
                .rows
                .len(),
            2
        );
        db.set_row_policy(Visibility::Residual);
        assert!(PublicIssuer::query(&db).fetch_first().await.is_err());
        assert!(
            PrivateIssuer::aggregate(&db)
                .group_by(PrivateIssuerAggregateField::Tenant)?
                .fetch_group_page(options, None)
                .await
                .is_err()
        );
        println!(
            "Public projections and complete groups retain global row authorization without loading custody fields"
        );
        Ok(())
    }
}
#[cfg(feature = "sqlite")]
#[tokio::main]
async fn main() -> graphql_orm::Result<()> {
    example::run().await
}
#[cfg(not(feature = "sqlite"))]
fn main() {
    println!("This runnable private repository example uses SQLite");
}
