#![cfg(feature = "sqlite")]

#[path = "support/ui.rs"]
mod ui;

#[test]
fn zero_public_write_inputs_compile_and_name_the_missing_operation() {
    let t = ui::test_cases();
    t.pass("tests/ui/zero_public_inputs_read_surface.rs");
    t.compile_fail("tests/ui/zero_public_update_mutation_shape.rs");
    t.compile_fail("tests/ui/zero_public_update_authorization.rs");
    t.compile_fail("tests/ui/zero_public_create_ai_policy.rs");
    t.compile_fail("tests/ui/zero_public_update_dependent_deletes.rs");
}
