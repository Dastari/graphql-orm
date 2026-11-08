#![cfg(feature = "sqlite")]
#[path = "support/ui.rs"]
mod ui;

#[test]
fn relation_columns_cannot_disclose_hidden_values() {
    let cases = ui::test_cases();
    cases.pass("tests/ui/recheck386/pass.rs");
    cases.compile_fail("tests/ui/recheck386/composite_target_key.rs");
    cases.compile_fail("tests/ui/recheck386/has_many_target_key.rs");
    cases.compile_fail("tests/ui/recheck386/missing_target_metadata.rs");
    cases.compile_fail("tests/ui/recheck386/self_target_condition.rs");
    cases.compile_fail("tests/ui/recheck386/source_condition_input_only.rs");
    cases.compile_fail("tests/ui/recheck386/source_condition_private.rs");
    cases.compile_fail("tests/ui/recheck386/source_condition_read_false.rs");
    cases.compile_fail("tests/ui/recheck386/target_condition.rs");
    cases.compile_fail("tests/ui/recheck386/target_key_input_only.rs");
    cases.compile_fail("tests/ui/recheck386/target_key_private.rs");
    cases.compile_fail("tests/ui/recheck386/target_key_read_false.rs");
    cases.compile_fail("tests/ui/recheck386/unresolved_target_key.rs");
}
