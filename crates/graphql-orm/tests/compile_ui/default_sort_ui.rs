#[cfg(feature = "sqlite")]
#[test]
fn default_sort_requires_known_columns_and_optional_directions() {
    let tests = crate::ui::test_cases();
    tests.compile_fail("tests/ui/default_sort_extra_tokens.rs");
    tests.compile_fail("tests/ui/default_sort_unknown_column.rs");
    tests.compile_fail("tests/ui/default_sort_bad_direction.rs");
    tests.compile_fail("tests/ui/default_sort_malformed.rs");
    tests.pass("tests/ui/default_sort_columns.rs");
}

#[cfg(feature = "mssql")]
#[test]
fn default_sort_rejects_unclosed_sql_server_brackets() {
    crate::ui::test_cases().compile_fail("tests/ui/default_sort_unclosed_bracket.rs");
}
