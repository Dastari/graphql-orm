//! A second consumer that enables the SQL Server backend.

pub type Database = graphql_orm::db::Database<graphql_orm::MssqlBackend>;
