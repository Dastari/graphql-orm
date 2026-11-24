//! A consumer of a ready-made backup adapter in a multi-backend workspace.

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub fn adapter(
    database: std::sync::Arc<graphql_orm::db::Database<BackupBackend>>,
) -> graphql_orm_backup::OrmBackupAdapter {
    graphql_orm_backup::OrmBackupAdapter::new(database, Vec::new())
}

#[cfg(feature = "sqlite")]
type BackupBackend = graphql_orm::SqliteBackend;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
type BackupBackend = graphql_orm::PostgresBackend;
