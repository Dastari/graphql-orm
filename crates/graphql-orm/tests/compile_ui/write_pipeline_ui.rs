use crate::ui;

#[cfg(feature = "sqlite")]
#[test]
fn generated_write_authority_cannot_be_forged() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/write_pipeline/forged_authority.rs");
}

/// Runs in the combined SQLite and PostgreSQL lane.
#[cfg(all(feature = "sqlite", feature = "postgres"))]
#[test]
fn a_request_admitted_for_one_backend_is_not_admitted_on_another() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/write_pipeline/backend_mismatch.rs");
}
