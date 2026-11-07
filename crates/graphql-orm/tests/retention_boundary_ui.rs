#![cfg(feature = "sqlite")]

#[path = "support/ui.rs"]
mod ui;

#[test]
fn host_retention_callback_compiles_without_sqlx_types() {
    let tests = ui::test_cases();
    tests.pass("tests/ui/retention_sqlx_free.rs");
}
