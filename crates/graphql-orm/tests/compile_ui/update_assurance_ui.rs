#![cfg(feature = "sqlite")]
//! Macro-time rules for a required ExpectedVersion and conditional update
//! assurance (#851).

use crate::ui;

#[test]
fn update_expected_version_and_update_assurance_are_validated() {
    let t = ui::test_cases();
    t.compile_fail("tests/ui/update_assurance_requires_managed_version.rs");
    t.compile_fail("tests/ui/update_assurance_requires_no_update_many.rs");
    t.compile_fail("tests/ui/update_assurance_requires_no_upsert.rs");
    t.compile_fail("tests/ui/update_assurance_invalid_options.rs");
    t.compile_fail("tests/ui/update_assurance_selector_signature.rs");
}
