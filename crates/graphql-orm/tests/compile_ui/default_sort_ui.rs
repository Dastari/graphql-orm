#![cfg(feature = "sqlite")]

#[test]
fn default_sort_requires_known_columns_and_optional_directions() {
    let tests = crate::ui::test_cases();
    tests.compile_fail("tests/ui/default_sort_extra_tokens.rs");
    tests.compile_fail("tests/ui/default_sort_unknown_column.rs");
    tests.compile_fail("tests/ui/default_sort_bad_direction.rs");
    tests.pass("tests/ui/default_sort_columns.rs");
}
