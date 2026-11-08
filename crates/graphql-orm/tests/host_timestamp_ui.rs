#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
#[path = "support/ui.rs"]
mod ui;

#[test]
fn host_timestamps_accept_public_and_string_fields_but_keep_schema_and_input_checks() {
    let cases = ui::test_cases();
    cases.pass("tests/ui/host_timestamp_supported.rs");
    cases.compile_fail("tests/ui/host_timestamp_unsupported.rs");
    cases.compile_fail("tests/ui/host_timestamp_required_create.rs");
}
