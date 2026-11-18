#![cfg(feature = "sqlite")]
//! Compile-time contract of the breaking batch: removed and refused API.

use crate::ui;

#[test]
fn removed_preloads_and_unsafe_key_only_deletes_do_not_compile() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/relation_preload_removed.rs");
    cases.compile_fail("tests/ui/relation_string_loader_removed.rs");
    cases.compile_fail("tests/ui/relation_loader_not_in_prelude.rs");
    cases.compile_fail("tests/ui/relation_key_literal_refused.rs");
    cases.compile_fail("tests/ui/key_only_deletes_key_policy.rs");
    cases.compile_fail("tests/ui/key_only_deletes_key_unsubscribed.rs");
    cases.compile_fail("tests/ui/key_only_deletes_key_skipped.rs");
}
