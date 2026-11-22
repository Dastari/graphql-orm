#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

#[path = "support/ui.rs"]
mod ui;

/// A declaration must identify a single column (#793).
#[test]
fn ambiguous_dotted_db_columns_are_refused() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/db_column_qualified_by_other_table.rs");
}

/// The derive refuses a table whose name is reserved for the aliases that
/// generated read-policy SQL renders (#796).
#[test]
fn tables_named_like_generated_aliases_are_refused() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/table_reserved_prefix.rs");
}
