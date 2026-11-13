#![cfg(feature = "sqlite")]
#[path = "support/ui.rs"]
mod ui;
#[test]
fn invalid_stable_id() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/stable_type_id_*.rs");
}
