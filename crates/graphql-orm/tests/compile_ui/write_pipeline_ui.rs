#![cfg(feature = "sqlite")]
use crate::ui;

#[test]
fn generated_write_authority_cannot_be_forged() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/write_pipeline/forged_authority.rs");
}
