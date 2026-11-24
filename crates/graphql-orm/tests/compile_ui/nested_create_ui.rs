#![cfg(feature = "sqlite")]

use crate::ui;

#[test]
fn incompatible_nested_create_declarations_are_rejected() {
    let t = ui::test_cases();
    t.compile_fail("tests/ui/nested_create_append_only.rs");
    t.compile_fail("tests/ui/nested_create_dependent_deletes.rs");
    t.compile_fail("tests/ui/nested_create_read_only.rs");
    t.compile_fail("tests/ui/nested_create_to_one.rs");
}
