#![cfg(any(feature = "sqlite", feature = "postgres"))]

use crate::ui;

#[cfg(feature = "sqlite")]
#[test]
fn incompatible_nested_create_declarations_are_rejected() {
    let t = ui::test_cases();
    t.compile_fail("tests/ui/nested_create_append_only.rs");
    t.compile_fail("tests/ui/nested_create_composite_key.rs");
    t.compile_fail("tests/ui/nested_create_conditional.rs");
    t.compile_fail("tests/ui/nested_create_dependent_deletes.rs");
    t.compile_fail("tests/ui/nested_create_key_mismatch.rs");
    t.compile_fail("tests/ui/nested_create_notify.rs");
    t.compile_fail("tests/ui/nested_create_nullable.rs");
    t.compile_fail("tests/ui/nested_create_parent_ai.rs");
    t.compile_fail("tests/ui/nested_create_parent_upsert.rs");
    t.compile_fail("tests/ui/nested_create_read_only.rs");
    t.compile_fail("tests/ui/nested_create_to_one.rs");
}

#[cfg(feature = "postgres")]
#[test]
fn insert_rls_tenant_must_be_bound() {
    ui::test_cases().compile_fail("tests/ui/nested_create_missing_insert_tenant.rs");
}
