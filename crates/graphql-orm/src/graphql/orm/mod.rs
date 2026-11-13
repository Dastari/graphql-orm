mod generated_filter;
#[doc(hidden)]
pub use generated_filter::*;
mod authorized_pagination;
pub use authorized_pagination::*;
mod backend;
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
mod backup;
mod core;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
mod default_equivalence;
mod dialect;
mod execution;
mod lease;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
mod migrations;
mod operation_metadata;
mod owned_foreign_keys;
mod owned_schema;
mod point_read;
mod query;
mod read_authorization;
mod rls;
mod root_fields;
mod runtime_backup;
mod runtime_decimal;
mod runtime_description;
mod runtime_format;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
mod runtime_key_read;
mod runtime_literal;
mod runtime_migration;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
mod runtime_mutation;
mod runtime_query;
mod runtime_record;
mod runtime_relation;
mod runtime_schema;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
mod runtime_search;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
#[doc(hidden)]
pub use runtime_search::runtime_search_storage_ready;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
pub use runtime_search::{
    RuntimeSearchApplyOptions, RuntimeSearchBackfillOptions, RuntimeSearchBackfillProgress,
};
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
mod runtime_transaction;
mod scalar_policy;
mod schema_manager;
mod schema_module;
mod search;
#[cfg(feature = "sqlite")]
pub mod spatial;

pub use backend::*;
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
pub(crate) use backup::begin_change_journal_transaction;
/// Multi-backend and MSSQL builds do not journal.
#[cfg(all(
    any(feature = "sqlite", feature = "postgres", feature = "mssql"),
    not(any(
        all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
        all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
    ))
))]
pub(crate) async fn begin_change_journal_transaction<B: WriteBackend>(
    _db: &crate::db::Database<B>,
    _tx: &mut B::Transaction<'_>,
) -> crate::Result<()> {
    Ok(())
}
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
pub use backup::*;
pub use core::*;
pub use dialect::*;
pub use execution::*;
pub use lease::*;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
pub use migrations::*;
pub use operation_metadata::*;
pub use owned_schema::{IndexModel, IndexPredicateModel, OwnedSchemaModel, OwnedTableModel};
pub use point_read::*;
pub use query::*;
pub use read_authorization::*;
pub use rls::*;
pub use root_fields::*;
pub use runtime_backup::*;
#[cfg(feature = "runtime-graphql")]
pub(crate) use runtime_decimal::runtime_decimal_syntax;
pub use runtime_decimal::{
    MAX_RUNTIME_DECIMAL_TEXT_BYTES, RuntimeDecimalError, RuntimeDecimalProblem,
    canonical_runtime_decimal, parse_runtime_decimal, runtime_decimal_from_integer,
};
pub use runtime_format::*;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
pub use runtime_key_read::*;
pub use runtime_literal::*;
pub use runtime_migration::*;
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
pub use runtime_mutation::*;
pub use runtime_query::*;
pub use runtime_record::*;
pub use runtime_relation::*;
pub use runtime_schema::*;
pub use scalar_policy::*;
pub use schema_manager::*;
pub use schema_module::*;
pub use search::*;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod static_deletes;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use static_deletes::*;
