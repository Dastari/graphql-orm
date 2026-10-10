#![cfg(all(feature = "runtime-graphql", feature = "mssql"))]
use graphql_orm::{
    db::{Database, mssql::MssqlPool},
    graphql::{orm::MssqlBackend, runtime::RuntimeGraphqlComposer},
};
#[test]
fn mssql_runtime_graphql_rejects_before_pool_acquisition() {
    let database =
        Database::<MssqlBackend>::new(MssqlPool::new(graphql_orm::tiberius::Config::new()));
    let result = RuntimeGraphqlComposer::new(database, "Query", Default::default());
    assert!(matches!(result, Err(error) if error.code() == "unsupported_backend"));
}
