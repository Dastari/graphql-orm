#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
#[test]
fn host_timestamps_require_repository_integer_fields_and_create_values() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/host_timestamp_unsupported.rs");
    cases.compile_fail("tests/ui/host_timestamp_required_create.rs");
}
