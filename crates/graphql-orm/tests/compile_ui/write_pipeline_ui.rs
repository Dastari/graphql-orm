#![cfg(feature = "sqlite")]
use crate::ui;

#[test]
fn generated_write_authority_cannot_be_forged() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/write_pipeline/forged_authority.rs");
}

#[cfg(not(any(feature = "postgres", feature = "mssql")))]
#[test]
fn an_upsert_lookup_cannot_be_supplied_by_the_host() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/write_pipeline/foreign_upsert_lookup.rs");
}

/// Runs in the combined SQLite and PostgreSQL lane.
#[cfg(feature = "postgres")]
#[test]
fn a_request_admitted_for_one_backend_is_not_admitted_on_another() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/write_pipeline/backend_mismatch.rs");
}
