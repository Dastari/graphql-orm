#![cfg(feature = "sqlite")]

#[path = "support/ui.rs"]
mod ui;

#[test]
fn invalid_typed_projection_declarations_are_rejected() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/projection_unknown_field.rs");
    tests.compile_fail("tests/ui/projection_duplicate_field.rs");
    tests.compile_fail("tests/ui/projection_empty.rs");
    tests.compile_fail("tests/ui/projection_schema_only.rs");
    tests.compile_fail("tests/ui/projection_type_mismatch.rs");
    tests.compile_fail("tests/ui/projection_not_generated.rs");
    tests.compile_fail("tests/ui/projection_wrong_backend.rs");
    tests.compile_fail("tests/ui/projection_public_graphql.rs");
    tests.compile_fail("tests/ui/projection_string_field.rs");
    tests.pass("tests/ui/projection_sqlx_free.rs");
}
