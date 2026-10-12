//! Neutral repository-only keys, shared with the dependency-minimal consumer.
use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(backend = "sqlite", table = "host_i16", plural = "HostI16s")
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(backend = "postgres", table = "host_i16", plural = "HostI16s")
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "dbo.HostI16",
        plural = "HostI16s",
        schema_policy = "external_writable"
    )
)]
pub struct HostI16 {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: i16,
    pub label: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(backend = "sqlite", table = "host_i32", plural = "HostI32s")
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(backend = "postgres", table = "host_i32", plural = "HostI32s")
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "dbo.HostI32",
        plural = "HostI32s",
        schema_policy = "external_writable"
    )
)]
pub struct HostI32 {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: i32,
    pub label: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(backend = "sqlite", table = "host_i64", plural = "HostI64s")
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(backend = "postgres", table = "host_i64", plural = "HostI64s")
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "dbo.HostI64",
        plural = "HostI64s",
        schema_policy = "external_writable"
    )
)]
pub struct HostI64 {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: i64,
    pub label: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(backend = "sqlite", table = "host_string", plural = "HostStrings")
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(backend = "postgres", table = "host_string", plural = "HostStrings")
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "dbo.HostString",
        plural = "HostStrings",
        schema_policy = "external_writable"
    )
)]
pub struct HostString {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub label: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(backend = "sqlite", table = "host_uuid", plural = "HostUuids")
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(backend = "postgres", table = "host_uuid", plural = "HostUuids")
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "dbo.HostUuid",
        plural = "HostUuids",
        schema_policy = "external_writable"
    )
)]
pub struct HostUuid {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: graphql_orm::uuid::Uuid,
    pub label: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(backend = "sqlite", table = "host_bytes", plural = "HostBytess")
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(backend = "postgres", table = "host_bytes", plural = "HostBytess")
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "dbo.HostBytes",
        plural = "HostBytess",
        schema_policy = "external_writable"
    )
)]
pub struct HostBytes {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: Vec<u8>,
    pub label: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "host_composite",
        plural = "HostComposites",
        repository_mutations = true
    )
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(
        backend = "postgres",
        table = "host_composite",
        plural = "HostComposites",
        repository_mutations = true
    )
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "dbo.HostComposite",
        plural = "HostComposites",
        schema_policy = "external_writable",
        repository_mutations = true
    )
)]
pub struct HostComposite {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub namespace: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: i32,
    pub label: String,
}

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.HostI8",
    plural = "HostI8s",
    schema_policy = "external_writable"
)]
pub struct HostI8 {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: i8,
    pub label: String,
}

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.HostIsize",
    plural = "HostIsizes",
    schema_policy = "external_writable"
)]
pub struct HostIsize {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: isize,
    pub label: String,
}

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.HostU8",
    plural = "HostU8s",
    schema_policy = "external_writable"
)]
pub struct HostU8 {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: u8,
    pub label: String,
}

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.HostU16",
    plural = "HostU16s",
    schema_policy = "external_writable"
)]
pub struct HostU16 {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: u16,
    pub label: String,
}

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.HostU32",
    plural = "HostU32s",
    schema_policy = "external_writable"
)]
pub struct HostU32 {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: u32,
    pub label: String,
}

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.HostU64",
    plural = "HostU64s",
    schema_policy = "external_writable"
)]
pub struct HostU64 {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: u64,
    pub label: String,
}

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.HostUsize",
    plural = "HostUsizes",
    schema_policy = "external_writable"
)]
pub struct HostUsize {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: usize,
    pub label: String,
}

/// Explicit admission only for these isolated test entities, never a production policy.
pub struct HostKeyFixturePolicy;
impl<B: OrmBackend> EntityPolicy<B> for HostKeyFixturePolicy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a graphql_orm::async_graphql::Context<'_>>,
        _: &'a Database<B>,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        let allowed = matches!(
            entity,
            "HostI8"
                | "HostI16"
                | "HostI32"
                | "HostI64"
                | "HostIsize"
                | "HostU8"
                | "HostU16"
                | "HostU32"
                | "HostU64"
                | "HostUsize"
                | "HostString"
                | "HostUuid"
                | "HostBytes"
                | "HostComposite"
        );
        Box::pin(async move { Ok(allowed) })
    }
}
