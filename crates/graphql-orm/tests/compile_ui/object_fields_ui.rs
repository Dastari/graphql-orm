#![cfg(feature = "sqlite")]
//! Macro-time diagnostics for handwritten fields on a generated object.

use crate::ui;

#[test]
fn handwritten_object_fields_compose_only_with_an_opted_in_generated_object() {
    let cases = ui::test_cases();
    cases.pass("tests/ui/object_fields_pass.rs");
    cases.compile_fail("tests/ui/object_fields_without_opt_in.rs");
    cases.compile_fail("tests/ui/object_fields_name_collision.rs");
    cases.compile_fail("tests/ui/object_fields_with_complex.rs");
    cases.compile_fail("tests/ui/object_fields_unmounted.rs");
    cases.compile_fail("tests/ui/object_fields_invalid_field.rs");
}
