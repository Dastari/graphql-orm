#[path = "support/ui.rs"]
mod ui;

#[test]
fn conditional_index_references_and_types_are_validated() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/conditional_index_unknown_field.rs");
    tests.compile_fail("tests/ui/conditional_index_non_string_predicate.rs");
}
