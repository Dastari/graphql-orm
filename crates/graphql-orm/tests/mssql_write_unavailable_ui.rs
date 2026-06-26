#![cfg(feature = "mssql")]

#[test]
fn mssql_write_helpers_are_not_generated() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/mssql_write_helper_unavailable.rs");
}
