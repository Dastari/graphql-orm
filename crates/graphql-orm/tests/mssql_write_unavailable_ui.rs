#![cfg(feature = "mssql")]

#[path = "support/ui.rs"]
mod ui;

#[test]
fn mssql_write_helpers_are_not_generated() {
    let t = ui::test_cases();
    t.compile_fail("tests/ui/mssql_composite_mutation_unavailable.rs");
    t.compile_fail("tests/ui/mssql_retention_unavailable.rs");
}
