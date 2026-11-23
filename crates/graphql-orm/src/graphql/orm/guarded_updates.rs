//! Tables whose generated update carries per-row checks (#851).
//!
//! An entity declaring `update_expected_version = "required"` or
//! `update_assurance(...)` records its table whenever metadata is built for
//! it through a typed constructor ([`super::EntityMetadata::from_schema`] or
//! `from_schema_with_retention`), including the generated `metadata()`.
//! Conversions that would serve the table without those checks, such as the
//! runtime schema IR, refuse it. The set only grows and is process-wide, so a
//! conversion fails closed for any metadata naming a recorded table,
//! including a copy, whichever spelling of the table it uses.
//!
//! Metadata or runtime IR that a host writes by hand, deserializes or
//! reconstructs without a typed constructor is host-authored and outside
//! this registry.
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

fn guarded_tables() -> &'static Mutex<BTreeSet<String>> {
    static TABLES: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    TABLES.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// Normalizes a table path to its bare lowercase name: identifier delimiters
/// are removed, ASCII letters are lowercased, and a default schema
/// (`public`, `dbo` or `main`) is stripped.
fn normalized(table_name: &str) -> String {
    let parts = table_name
        .split('.')
        .map(|part| {
            part.trim()
                .trim_matches(|c| matches!(c, '"' | '`' | '[' | ']'))
                .to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [schema, table] if matches!(schema.as_str(), "public" | "dbo" | "main") => table.clone(),
        _ => parts.join("."),
    }
}

/// Records a table whose generated update requires `ExpectedVersion` or
/// selects assurance per request. Public only for derive output.
#[doc(hidden)]
pub fn __register_guarded_update_table(table_name: &str) {
    guarded_tables()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(normalized(table_name));
}

/// Whether a typed constructor recorded `table_name`, in any equivalent
/// spelling, as guarded.
pub(crate) fn is_guarded_update_table(table_name: &str) -> bool {
    guarded_tables()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&normalized(table_name))
}

#[cfg(test)]
mod tests {
    use super::normalized;

    #[test]
    fn equivalent_spellings_normalize_to_one_bare_name() {
        for spelling in [
            "review_records",
            "Review_Records",
            "\"review_records\"",
            "public.review_records",
            "\"public\".\"Review_Records\"",
            "dbo.review_records",
            "[dbo].[REVIEW_RECORDS]",
            "main.review_records",
        ] {
            assert_eq!(normalized(spelling), "review_records", "{spelling}");
        }
        assert_eq!(normalized("audit.review_records"), "audit.review_records");
    }
}
