#[path = "support/ui.rs"]
mod ui;

#[test]
fn ordinary_index_names_columns_and_directions_are_validated() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/ordinary_index_direction_arity.rs");
    tests.compile_fail("tests/ui/ordinary_index_invalid_direction.rs");
    tests.compile_fail("tests/ui/ordinary_index_empty_name.rs");
    tests.compile_fail("tests/ui/ordinary_index_unknown_field.rs");
}
