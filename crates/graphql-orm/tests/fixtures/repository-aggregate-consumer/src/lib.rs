//! Repository aggregate consumer with no direct async-graphql dependency.

use graphql_orm::prelude::*;

#[cfg(feature = "sqlite")]
pub type Backend = SqliteBackend;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
pub type Backend = PostgresBackend;
#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
pub type Backend = MssqlBackend;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "repository_aggregate_scalar",
        plural = "RepositoryAggregateScalars"
    )
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(
        backend = "postgres",
        table = "repository_aggregate_scalar",
        plural = "RepositoryAggregateScalars"
    )
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "repository_aggregate_scalar",
        plural = "RepositoryAggregateScalars",
        schema_policy = "external_writable"
    )
)]
pub struct ScalarAggregate {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    #[filterable(type = "string")]
    pub team: Option<String>,
    #[graphql_orm(read_policy = "aggregate.units.read")]
    pub units: i64,
    #[graphql_orm(private, sensitive)]
    pub secret: Option<String>,
    #[graphql_orm(read = false)]
    pub unreadable: Option<String>,
}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "repository_aggregate_composite",
        plural = "RepositoryAggregateComposites",
        repository_mutations = true
    )
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(
        backend = "postgres",
        table = "repository_aggregate_composite",
        plural = "RepositoryAggregateComposites",
        repository_mutations = true
    )
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "repository_aggregate_composite",
        plural = "RepositoryAggregateComposites",
        repository_mutations = true,
        schema_policy = "external_writable"
    )
)]
pub struct CompositeAggregate {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub tenant: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub item: String,
    pub units: i64,
    #[graphql_orm(private, sensitive)]
    pub secret: Option<String>,
}

/// Type-checks the plain aggregate enums/builders on every supported backend.
pub fn build_aggregates(database: &Database<Backend>) -> graphql_orm::Result<()> {
    let scalar = ScalarAggregateAggregateField::Units;
    let same = scalar;
    assert_eq!(scalar, same);
    let mut set = std::collections::HashSet::new();
    set.insert(scalar);
    assert!(set.contains(&same));
    let _ = ScalarAggregate::aggregate(database)
        .group_by(ScalarAggregateAggregateField::Team)?
        .count_rows()?
        .sum(scalar)?;
    let _ = CompositeAggregate::aggregate(database)
        .group_by(CompositeAggregateAggregateField::Tenant)?
        .sum(CompositeAggregateAggregateField::Units)?;
    Ok(())
}

// Inference is ambiguous if either repository enum accidentally implements a
// GraphQL trait. These assertions type-check without a direct dependency.
const _: fn() = || {
    trait NotInput<A> {
        fn probe() {}
    }
    impl<T: ?Sized> NotInput<()> for T {}
    impl<T: graphql_orm::async_graphql::InputType> NotInput<u8> for T {}
    trait NotOutput<A> {
        fn probe() {}
    }
    impl<T: ?Sized> NotOutput<()> for T {}
    impl<T: graphql_orm::async_graphql::OutputType> NotOutput<u8> for T {}
    let _ = <ScalarAggregateAggregateField as NotInput<_>>::probe;
    let _ = <ScalarAggregateAggregateField as NotOutput<_>>::probe;
    let _ = <CompositeAggregateAggregateField as NotInput<_>>::probe;
    let _ = <CompositeAggregateAggregateField as NotOutput<_>>::probe;
};
