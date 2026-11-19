#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

#[path = "support/ui.rs"]
mod ui;

/// The derive refuses a `db_column` whose qualifier names another table, so
/// no generated filter or policy can bind it to a different row (#782).
#[test]
fn db_columns_qualified_by_another_table_are_refused() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/db_column_qualified_by_other_table.rs");
}
