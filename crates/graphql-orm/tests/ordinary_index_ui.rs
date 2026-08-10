#[test]
fn ordinary_index_names_columns_and_directions_are_validated() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/ordinary_index_direction_arity.rs");
    tests.compile_fail("tests/ui/ordinary_index_invalid_direction.rs");
    tests.compile_fail("tests/ui/ordinary_index_empty_name.rs");
    tests.compile_fail("tests/ui/ordinary_index_unknown_field.rs");
}
