#![cfg(feature = "sqlite")]
#[path = "support/ui.rs"]
mod ui;

#[test]
fn empty_order_inputs_have_an_entity_owned_diagnostic() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/empty_order_record.rs");
    cases.compile_fail("tests/ui/empty_order_relation.rs");
}
