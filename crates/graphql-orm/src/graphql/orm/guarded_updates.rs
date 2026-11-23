//! Tables whose generated update carries per-row checks (#851).
//!
//! An entity declaring `update_expected_version = "required"` or
//! `update_assurance(...)` records its table when its metadata is first
//! built. Conversions that would serve the table without those checks, such
//! as the runtime schema IR, refuse it. The set only grows and is
//! process-wide, so a conversion fails closed for any metadata naming a
//! recorded table, including a copy of the entity's metadata.
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

fn guarded_tables() -> &'static Mutex<BTreeSet<&'static str>> {
    static TABLES: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    TABLES.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// Records a table whose generated update requires `ExpectedVersion` or
/// selects assurance per request. Public only for derive output.
#[doc(hidden)]
pub fn __register_guarded_update_table(table_name: &'static str) {
    guarded_tables()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(table_name);
}

/// Whether a derive recorded `table_name` as guarded.
pub(crate) fn is_guarded_update_table(table_name: &str) -> bool {
    guarded_tables()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(table_name)
}
