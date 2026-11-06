#![cfg(feature = "sqlite")]

#[test]
fn mutation_shapes_configuration_is_validated() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/mutation_shapes_unknown.rs");
    t.compile_fail("tests/ui/mutation_shapes_empty.rs");
    t.compile_fail("tests/ui/mutation_shapes_duplicate.rs");
    t.compile_fail("tests/ui/mutation_shapes_declared_twice.rs");
    t.compile_fail("tests/ui/mutation_shapes_append_only_delete.rs");
    t.compile_fail("tests/ui/mutation_shapes_missing_upsert.rs");
    t.compile_fail("tests/ui/mutation_shapes_conflicting_authorization.rs");
    t.compile_fail("tests/ui/mutation_shapes_conflicting_ai_policy.rs");
}
