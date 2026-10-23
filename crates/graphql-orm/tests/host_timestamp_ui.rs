#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
#[test]
fn host_timestamps_accept_public_and_string_fields_but_keep_schema_and_input_checks() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/host_timestamp_supported.rs");
    cases.compile_fail("tests/ui/host_timestamp_unsupported.rs");
    cases.compile_fail("tests/ui/host_timestamp_required_create.rs");
}
