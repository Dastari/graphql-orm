//! Connection-pinned, policy-aware snapshot reads.

use super::{
    DefaultWriteBackend, Entity, FromSqlRow, MutationContext, MutationQuery, ReadProjection,
    ReadSnapshotError, SqlValue, TransactionProjectionQuery, WriteBackend,
};
use crate::db::Database;
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};

tokio::task_local! {
    // The scope marks snapshot reads. The flag remembers a refusal even if
    // the callback catches the individual error.
    pub(crate) static SNAPSHOT_WRITE_REFUSED: std::cell::Cell<bool>;
}

pub(crate) fn ensure_write_allowed() -> Result<(), OrmPublicError> {
    if SNAPSHOT_WRITE_REFUSED
        .try_with(|refused| refused.set(true))
        .is_ok()
    {
        Err(OrmPublicError::with_message(
            OrmErrorCode::Forbidden,
            ReadSnapshotError::WriteRefused.to_string(),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn ensure_sql_write_allowed() -> crate::Result<()> {
    ensure_write_allowed().map_err(crate::graphql::errors::sqlx_error_from_public)
}

/// Read capability on a single snapshot created by
/// [`Database::read_snapshot_transaction`](Database::read_snapshot_transaction).
///
/// Reads use the same entity, row, and field policies as ordinary ORM
/// transaction reads. The context exposes typed reads and keeps the pinned
/// connection, raw SQL, mutations, savepoints, and post-commit hooks private.
/// Handwritten filters, ordering expressions, and other SQL-producing trait
/// implementations must supply read expressions, as for ordinary ORM reads.
pub struct ReadSnapshotContext<'tx, B: WriteBackend = DefaultWriteBackend> {
    reads: MutationContext<'tx, B>,
}

impl<'tx, B: WriteBackend> ReadSnapshotContext<'tx, B> {
    pub(crate) fn new(db: &'tx Database<B>, tx: B::Transaction<'tx>) -> Self {
        Self {
            reads: MutationContext::new(db, tx),
        }
    }

    pub(crate) async fn rollback(self) -> crate::Result<()> {
        self.reads.rollback().await
    }

    /// Start a bounded, policy-aware entity query on the snapshot.
    pub fn query<T>(&mut self) -> MutationQuery<'_, 'tx, T, B>
    where
        T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        self.reads.query()
    }

    /// Read a typed projection on the snapshot.
    pub fn project<P: ReadProjection<B>>(&mut self) -> TransactionProjectionQuery<'_, 'tx, P, B> {
        self.reads.project()
    }

    /// Look up a visible entity by its primary-key values, in declared key order.
    ///
    /// Each key component is bound as a parameter. A single-column key takes
    /// one value. An incomplete key fails with `INVALID_INPUT`.
    pub async fn find_by_key<T>(&mut self, values: &[SqlValue]) -> crate::Result<Option<T>>
    where
        T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        self.query::<T>()
            .matching_primary_key(values)?
            .fetch_one()
            .await
    }
}
