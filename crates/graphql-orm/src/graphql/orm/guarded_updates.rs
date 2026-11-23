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
//! Tables are matched on their bare table component in any schema. An
//! unqualified name can resolve to a table in another schema (PostgreSQL's
//! `search_path`, for example), so a guarded `other.t` refuses a runtime `t`
//! and a guarded `t` refuses a runtime `other.t`. An unrelated table with the
//! same bare name in another schema is refused too; that false refusal is
//! deliberate.
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

/// Normalizes a table path to its bare table component: the last
/// dot-separated part, with identifier delimiters removed and ASCII letters
/// lowercased. Any schema, default or not, is dropped.
fn normalized(table_name: &str) -> String {
    table_name
        .rsplit('.')
        .next()
        .unwrap_or(table_name)
        .trim()
        .trim_matches(|c| matches!(c, '"' | '`' | '[' | ']'))
        .to_ascii_lowercase()
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

/// Whether a typed constructor recorded a table with the same bare table
/// component as `table_name`, in any schema, as guarded.
pub(crate) fn is_guarded_update_table(table_name: &str) -> bool {
    guarded_tables()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&normalized(table_name))
}

#[cfg(test)]
mod tests {
    use super::{__register_guarded_update_table, is_guarded_update_table, normalized};

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
            "audit.review_records",
        ] {
            assert_eq!(normalized(spelling), "review_records", "{spelling}");
        }
    }

    #[test]
    fn a_guarded_table_matches_its_bare_name_in_any_schema() {
        // A guarded table in a non-default schema refuses the bare name,
        // which search_path can resolve to it.
        __register_guarded_update_table("other.guarded_in_other_schema");
        assert!(is_guarded_update_table("guarded_in_other_schema"));
        assert!(is_guarded_update_table("public.guarded_in_other_schema"));
        // A guarded bare table refuses the same name in another schema.
        __register_guarded_update_table("guarded_bare_table");
        assert!(is_guarded_update_table("other.guarded_bare_table"));
        assert!(is_guarded_update_table("\"Other\".\"Guarded_Bare_Table\""));
        // Unrelated names pass.
        assert!(!is_guarded_update_table("unrelated_table"));
        assert!(!is_guarded_update_table("other.unrelated_table"));
    }
}
