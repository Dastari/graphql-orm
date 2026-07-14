#![cfg(feature = "sqlite")]

#[test]
fn host_retention_callback_compiles_without_sqlx_types() {
    let tests = trybuild::TestCases::new();
    tests.pass("tests/ui/retention_sqlx_free.rs");
}
