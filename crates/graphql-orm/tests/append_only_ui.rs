#![cfg(feature = "sqlite")]

#[test]
fn append_only_rejects_incompatible_configuration_and_has_no_mutation_methods() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/append_only_upsert.rs");
    tests.compile_fail("tests/ui/append_only_update_unavailable.rs");
}
