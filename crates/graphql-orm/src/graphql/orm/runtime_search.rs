//! Search documents of runtime collections.
//!
//! A runtime collection with [`RuntimeSearch`] keeps one document per row in
//! the same storage as a static search entity (`__graphql_orm_search_<table>` or
//! the SQLite FTS5 table), keyed by the text of its single primary-key value.

use super::SearchWeight;
use super::backend::OrmBackend;
use super::core::SqlValue;
use super::dialect::{DatabaseBackend, SqlDialect};
use super::runtime_record::RuntimeValue;
use super::runtime_schema::{
    CollectionId, RuntimeCollection, RuntimeSearch, RuntimeSearchWeight, RuntimeValueKind,
    ValidatedRuntimeSchema,
};
use super::search::{
    SearchDocument, SearchDocumentChunk, SearchDocumentSource, search_json_path_text,
};

/// The document key text of a primary-key value: the string itself, the
/// hyphenated lowercase UUID, or the decimal integer, matching what
/// `CAST(<key> AS TEXT)` yields on SQLite and PostgreSQL.
pub(crate) fn search_key_text(value: &RuntimeValue) -> Option<String> {
    match value {
        RuntimeValue::String(value) => Some(value.clone()),
        RuntimeValue::Uuid(value) => Some(value.hyphenated().to_string()),
        RuntimeValue::Integer(value) => Some(value.to_string()),
        _ => None,
    }
}

/// The JSON form of a primary-key value stored beside its document.
pub(crate) fn search_key_json(value: &RuntimeValue) -> serde_json::Value {
    match value {
        RuntimeValue::String(value) => serde_json::Value::String(value.clone()),
        RuntimeValue::Uuid(value) => serde_json::Value::String(value.hyphenated().to_string()),
        RuntimeValue::Integer(value) => serde_json::Value::from(*value),
        _ => serde_json::Value::Null,
    }
}

fn source_alias(index: usize) -> String {
    format!("__gom_search_source_{index}")
}

/// One text column per search entry, in declaration order.
pub(crate) fn source_columns_sql(
    backend: DatabaseBackend,
    collection: &RuntimeCollection,
    search: &RuntimeSearch,
) -> String {
    search
        .fields
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let column = collection
                .fields
                .iter()
                .find(|field| field.id == entry.field)
                .map(|field| backend.quote_identifier(&field.physical_column))
                .unwrap_or_else(|| "NULL".to_string());
            format!("CAST({column} AS TEXT) AS {}", source_alias(index))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn weight(weight: RuntimeSearchWeight) -> SearchWeight {
    match weight {
        RuntimeSearchWeight::A => SearchWeight::A,
        RuntimeSearchWeight::B => SearchWeight::B,
        RuntimeSearchWeight::C => SearchWeight::C,
        RuntimeSearchWeight::D => SearchWeight::D,
    }
}

/// Build a row's search document from the columns selected by
/// [`source_columns_sql`]. String entries contribute their text; JSON entries
/// contribute the string leaves at their path.
pub(crate) fn document_from_row<B: OrmBackend>(
    collection: &RuntimeCollection,
    search: &RuntimeSearch,
    row: &B::Row,
    entity_pk: String,
    entity_pk_json: serde_json::Value,
) -> crate::Result<SearchDocument> {
    let _ = collection;
    let mut chunks = Vec::with_capacity(search.fields.len());
    for (index, entry) in search.fields.iter().enumerate() {
        let Some(raw) = B::try_get_optional_string(row, &source_alias(index))? else {
            continue;
        };
        let text = match &entry.json_path {
            None => raw,
            Some(path) => serde_json::from_str::<serde_json::Value>(&raw)
                .map(|value| search_json_path_text(&value, path))
                .unwrap_or_default(),
        };
        if text.trim().is_empty() {
            continue;
        }
        // Fold accents on every backend, so PostgreSQL documents match folded
        // queries; SQLite's tokenizer folds them again natively.
        let text = if search.remove_diacritics {
            super::runtime_query::fold_diacritics(&text)
        } else {
            text
        };
        chunks.push(SearchDocumentChunk {
            source: match entry.json_path {
                None => SearchDocumentSource::Field {
                    field_name: "runtime",
                },
                Some(_) => SearchDocumentSource::JsonPath {
                    field_name: "runtime",
                    path: "runtime",
                },
            },
            weight: weight(entry.weight),
            text,
        });
    }
    Ok(SearchDocument {
        entity_pk,
        entity_pk_json,
        chunks,
    })
}

/// Options for [`SchemaManager::backfill_runtime_search`](super::SchemaManager::backfill_runtime_search).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuntimeSearchBackfillOptions {
    /// Rows indexed per batch; each batch is its own transaction. At least 1;
    /// 500 by default.
    pub batch_size: usize,
    /// Stop after this many batches and report progress, so a host can run a
    /// large backfill as its own bounded, fenced job. Unbounded by default.
    pub max_batches: Option<usize>,
}

impl Default for RuntimeSearchBackfillOptions {
    fn default() -> Self {
        Self {
            batch_size: 500,
            max_batches: None,
        }
    }
}

impl RuntimeSearchBackfillOptions {
    /// Index `batch_size` rows per batch.
    #[must_use]
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    /// Stop after at most `max_batches` batches.
    #[must_use]
    pub fn with_max_batches(mut self, max_batches: usize) -> Self {
        self.max_batches = Some(max_batches);
        self
    }
}

/// Progress of a runtime search backfill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuntimeSearchBackfillProgress {
    /// Rows indexed by this call.
    pub processed: u64,
    /// Rows still to index after this call; zero once ready.
    pub remaining: u64,
    /// The index is `ready` and the collection's `search` argument is admitted.
    pub ready: bool,
}

/// Readiness recorded in a runtime search index's metadata row.
pub(crate) const READINESS_BUILDING: &str = "building";
pub(crate) const READINESS_READY: &str = "ready";

fn search_failure(message: &str) -> crate::graphql::errors::OrmPublicError {
    crate::graphql::errors::OrmPublicError::from(sqlx::Error::Protocol(message.to_string()))
}

fn transaction_failure(
    error: super::TransactionError,
) -> super::runtime_migration::RuntimeMigrationError {
    let message = match error {
        super::TransactionError::Rejected(error)
        | super::TransactionError::Retryable(error)
        | super::TransactionError::Failed(error) => error.message,
    };
    super::runtime_migration::RuntimeMigrationError::Database(sqlx::Error::Protocol(message))
}

fn not_declared(collection: &CollectionId) -> super::runtime_migration::RuntimeMigrationError {
    super::runtime_migration::RuntimeMigrationError::Diagnostics(
        super::runtime_migration::RuntimeMigrationDiagnostics(vec![
            super::runtime_migration::RuntimeMigrationDiagnostic {
                code: super::runtime_migration::RuntimeMigrationDiagnosticCode::SearchNotDeclared,
                collection: Some(collection.clone()),
                subject: None,
            },
        ]),
    )
}

/// The bind form of a key read back as text, typed like the key column so the
/// keyset comparison uses the primary-key index.
fn key_bind(kind: RuntimeValueKind, text: &str) -> Option<SqlValue> {
    match kind {
        RuntimeValueKind::Integer => text.parse().ok().map(SqlValue::Int),
        RuntimeValueKind::Uuid => uuid::Uuid::parse_str(text).ok().map(SqlValue::Uuid),
        RuntimeValueKind::String => Some(SqlValue::String(text.to_string())),
        _ => None,
    }
}

fn key_json(kind: RuntimeValueKind, text: &str) -> serde_json::Value {
    match kind {
        RuntimeValueKind::Integer => text
            .parse::<i64>()
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::String(text.to_string()),
    }
}

/// Read the readiness metadata of one runtime search index inside a transaction.
async fn read_metadata<B: super::WriteBackend>(
    context: &mut super::MutationContext<'_, B>,
    table: &str,
) -> crate::Result<Option<serde_json::Value>> {
    let column = match B::DIALECT {
        DatabaseBackend::Postgres => "config_json::text",
        _ => "config_json",
    };
    let sql = format!(
        "SELECT {column} AS config_json FROM {} WHERE table_name = ?",
        super::search::search_metadata_table_name()
    );
    let rows = context
        .fetch_rows(&sql, &[SqlValue::String(table.to_string())])
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let text = B::try_get_string(row, "config_json")?;
    Ok(serde_json::from_str(&text).ok())
}

async fn write_metadata<B: super::WriteBackend>(
    context: &mut super::MutationContext<'_, B>,
    table: &str,
    config: &serde_json::Value,
) -> crate::Result<()> {
    let value = match B::DIALECT {
        DatabaseBackend::Postgres => "?::jsonb",
        _ => "?",
    };
    let sql = format!(
        "UPDATE {} SET config_json = {value} WHERE table_name = ?",
        super::search::search_metadata_table_name()
    );
    context
        .execute(
            &sql,
            &[
                SqlValue::String(config.to_string()),
                SqlValue::String(table.to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Whether a readiness-tracked index is `ready`. A missing row or readiness is
/// not ready.
pub(crate) fn metadata_ready(config: Option<&serde_json::Value>) -> bool {
    config
        .and_then(|config| config.get("readiness"))
        .and_then(serde_json::Value::as_str)
        == Some(READINESS_READY)
}

impl<B> super::SchemaManager<'_, B>
where
    B: super::RuntimeMutationBackend + super::runtime_migration::RuntimeMigrationBackend,
{
    /// Whether a runtime collection's search index is `ready`, so its `search`
    /// argument is admitted.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeMigrationDiagnosticCode::SearchNotDeclared`](super::RuntimeMigrationDiagnosticCode::SearchNotDeclared)
    /// for a collection without search, or a database failure.
    pub async fn runtime_search_ready(
        &self,
        schema: &ValidatedRuntimeSchema,
        collection: &CollectionId,
    ) -> Result<bool, super::runtime_migration::RuntimeMigrationError> {
        let found = schema
            .schema()
            .collections
            .iter()
            .find(|candidate| &candidate.id == collection)
            .filter(|candidate| candidate.search.is_some())
            .ok_or_else(|| not_declared(collection))?;
        let table = found.physical_table.clone();
        self.database
            .transaction(super::TransactionMode::Default, move |tx| {
                Box::pin(
                    async move { Ok(metadata_ready(read_metadata(tx, &table).await?.as_ref())) },
                )
            })
            .await
            .map_err(transaction_failure)
    }

    /// Index existing rows of a runtime collection's search storage, resuming
    /// by primary key, and mark it `ready` once every row is indexed.
    ///
    /// Each batch of `options.batch_size` rows is one transaction. On
    /// PostgreSQL it locks the batch's rows (`FOR UPDATE`) while it writes their
    /// documents, so a concurrent writer either waits for the batch or wrote its
    /// own document first; SQLite serialises writers anyway. Progress is
    /// recorded in the index metadata after every batch, so a backfill stopped
    /// by a crash, a cancellation or `max_batches` resumes where it stopped.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeMigrationDiagnosticCode::SearchNotDeclared`](super::RuntimeMigrationDiagnosticCode::SearchNotDeclared)
    /// when the collection has no search or its search storage does not exist,
    /// or a database failure.
    pub async fn backfill_runtime_search(
        &self,
        schema: &ValidatedRuntimeSchema,
        collection: &CollectionId,
        options: RuntimeSearchBackfillOptions,
    ) -> Result<RuntimeSearchBackfillProgress, super::runtime_migration::RuntimeMigrationError>
    {
        let found = schema
            .schema()
            .collections
            .iter()
            .find(|candidate| &candidate.id == collection)
            .ok_or_else(|| not_declared(collection))?;
        let search = found
            .search
            .clone()
            .ok_or_else(|| not_declared(collection))?;
        let found = found.clone();
        let [key_id] = found.primary_key.as_slice() else {
            return Err(not_declared(collection));
        };
        let key_field = found
            .fields
            .iter()
            .find(|field| &field.id == key_id)
            .cloned()
            .ok_or_else(|| not_declared(collection))?;
        let batch_size = options.batch_size.max(1);
        let mut processed = 0_u64;
        let mut batches = 0_usize;
        loop {
            if options.max_batches.is_some_and(|max| batches >= max) {
                break;
            }
            batches += 1;
            let found = found.clone();
            let search = search.clone();
            let key_field = key_field.clone();
            let step = self
                .database
                .transaction(super::TransactionMode::Default, move |tx| {
                    Box::pin(async move {
                        let table = found.physical_table.clone();
                        let Some(mut config) = read_metadata(tx, &table).await? else {
                            return Ok(None);
                        };
                        if metadata_ready(Some(&config)) {
                            return Ok(Some((0_u64, true)));
                        }
                        let key_column = B::DIALECT.quote_identifier(&key_field.physical_column);
                        let mut values = Vec::new();
                        let after = config
                            .get("backfill_after")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|text| key_bind(key_field.value_kind, text));
                        let condition = match after {
                            Some(bind) => {
                                values.push(bind);
                                format!("WHERE {key_column} > ?")
                            }
                            None => String::new(),
                        };
                        let lock = match B::DIALECT {
                            DatabaseBackend::Postgres => " FOR UPDATE",
                            _ => "",
                        };
                        let sql = format!(
                            "SELECT CAST({key_column} AS TEXT) AS __gom_search_key, {} FROM {} {condition} ORDER BY {key_column} LIMIT {batch_size}{lock}",
                            source_columns_sql(B::DIALECT, &found, &search),
                            B::DIALECT.quote_identifier(&table),
                        );
                        let rows = tx.fetch_rows(&sql, &values).await?;
                        let mut last = None;
                        for row in &rows {
                            let key = B::try_get_string(row, "__gom_search_key")?;
                            let document = document_from_row::<B>(
                                &found,
                                &search,
                                row,
                                key.clone(),
                                key_json(key_field.value_kind, &key),
                            )?;
                            super::search::upsert_search_document_for_table_on(
                                tx,
                                &table,
                                search.effective_language(),
                                &document,
                            )
                            .await?;
                            last = Some(key);
                        }
                        let finished = rows.len() < batch_size;
                        let object = config
                            .as_object_mut()
                            .ok_or_else(|| search_failure("search metadata is not an object"))?;
                        if finished {
                            object.insert("readiness".into(), READINESS_READY.into());
                            object.remove("backfill_after");
                        } else if let Some(last) = last {
                            object.insert("readiness".into(), READINESS_BUILDING.into());
                            object.insert("backfill_after".into(), last.into());
                        }
                        write_metadata(tx, &table, &config).await?;
                        Ok(Some((rows.len() as u64, finished)))
                    })
                })
                .await
                .map_err(transaction_failure)?;
            let Some((count, ready)) = step else {
                return Err(not_declared(collection));
            };
            processed += count;
            if ready {
                return Ok(RuntimeSearchBackfillProgress {
                    processed,
                    remaining: 0,
                    ready: true,
                });
            }
        }
        let remaining = self.remaining_backfill_rows(&found, &key_field).await?;
        Ok(RuntimeSearchBackfillProgress {
            processed,
            remaining,
            ready: false,
        })
    }

    async fn remaining_backfill_rows(
        &self,
        collection: &RuntimeCollection,
        key_field: &super::runtime_schema::RuntimeField,
    ) -> Result<u64, super::runtime_migration::RuntimeMigrationError> {
        let table = collection.physical_table.clone();
        let key_field = key_field.clone();
        self.database
            .transaction(super::TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    let config = read_metadata(tx, &table).await?;
                    let key_column = B::DIALECT.quote_identifier(&key_field.physical_column);
                    let mut values = Vec::new();
                    let condition = match config
                        .as_ref()
                        .and_then(|config| config.get("backfill_after"))
                        .and_then(serde_json::Value::as_str)
                        .and_then(|text| key_bind(key_field.value_kind, text))
                    {
                        Some(bind) => {
                            values.push(bind);
                            format!("WHERE {key_column} > ?")
                        }
                        None => String::new(),
                    };
                    let sql = format!(
                        "SELECT COUNT(*) AS remaining FROM {} {condition}",
                        B::DIALECT.quote_identifier(&table)
                    );
                    let rows = tx.fetch_rows(&sql, &values).await?;
                    let remaining = rows
                        .first()
                        .map(|row| B::try_get_i64(row, "remaining"))
                        .transpose()?
                        .unwrap_or(0);
                    Ok(u64::try_from(remaining).unwrap_or(0))
                })
            })
            .await
            .map_err(transaction_failure)
    }
}

/// Search options of [`SchemaManager::apply_runtime_migration`](super::SchemaManager::apply_runtime_migration).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuntimeSearchApplyOptions {
    /// Leave new or changed search storage `building` for a host-run
    /// [`backfill_runtime_search`](super::SchemaManager::backfill_runtime_search)
    /// job instead of indexing existing rows during apply.
    pub defer_backfill: bool,
    /// Batching of the apply-time backfill.
    pub backfill: RuntimeSearchBackfillOptions,
}

impl RuntimeSearchApplyOptions {
    /// Defer the backfill to a host-run job.
    #[must_use]
    pub fn deferred() -> Self {
        Self {
            defer_backfill: true,
            ..Self::default()
        }
    }

    /// Run the apply-time backfill with these options.
    #[must_use]
    pub fn with_backfill(mut self, backfill: RuntimeSearchBackfillOptions) -> Self {
        self.backfill = backfill;
        self
    }
}

/// Whether a runtime collection's search storage is `ready`, read without a
/// transaction. A missing metadata row or table is not ready.
pub async fn runtime_search_storage_ready<B: OrmBackend>(pool: &B::Pool, table: &str) -> bool {
    let column = match B::DIALECT {
        DatabaseBackend::Postgres => "config_json::text",
        _ => "config_json",
    };
    let sql = format!(
        "SELECT {column} AS config_json FROM {} WHERE table_name = {}",
        super::search::search_metadata_table_name(),
        B::DIALECT.placeholder(1),
    );
    let Ok(rows) = B::fetch_rows(pool, &sql, &[SqlValue::String(table.to_string())]).await else {
        return false;
    };
    let config = rows
        .first()
        .and_then(|row| B::try_get_string(row, "config_json").ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    metadata_ready(config.as_ref())
}
