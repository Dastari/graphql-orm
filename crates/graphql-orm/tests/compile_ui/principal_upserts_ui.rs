#![cfg(feature = "sqlite")]
use crate::ui;

#[test]
fn principal_bound_upserts_refuse_unsafe_declarations() {
    let cases = ui::test_cases();
    cases.pass("tests/ui/principal_upserts/pass.rs");
    cases.compile_fail("tests/ui/principal_upserts/acknowledge_false.rs");
    cases.compile_fail("tests/ui/principal_upserts/acknowledge_without_upsert.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_duplicate.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_filterable.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_missing_binding.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_not_a_binding.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_not_conflict_member.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_not_private.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_nullable.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_primary_key.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_public_alias.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_repository_entity.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_upsert_not_selected.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_with_create.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_with_create_selected.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_without_upsert.rs");
    cases.compile_fail("tests/ui/principal_upserts/bound_wrong_key_type.rs");
    cases.compile_fail("tests/ui/principal_upserts/transient_duplicate.rs");
    cases.compile_fail("tests/ui/principal_upserts/transient_missing_on.rs");
    cases.compile_fail("tests/ui/principal_upserts/transient_named_input.rs");
    cases.compile_fail("tests/ui/principal_upserts/transient_unsupported_on.rs");
    cases.compile_fail("tests/ui/principal_upserts/transient_without_upsert.rs");
}
