#[test]
fn conditional_index_references_and_types_are_validated() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/conditional_index_unknown_field.rs");
    tests.compile_fail("tests/ui/conditional_index_non_string_predicate.rs");
}
