#![cfg(feature = "sqlite")]

#[test]
fn append_only_rejects_incompatible_configuration_and_has_no_mutation_methods() {
    // The absent-method probe lives in the library's `generated_api_absence_probes`
    // compile_fail doctests, which are independent of rustc diagnostic prose. Only the
    // macro-owned diagnostic (stable text) is snapshot-checked here.
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/append_only_upsert.rs");
    tests.compile_fail("tests/ui/retention_requires_append_only.rs");
    tests.compile_fail("tests/ui/retention_wrong_entity.rs");
    tests.compile_fail("tests/ui/retention_unavailable_in_normal_transaction.rs");
    tests.compile_fail("tests/ui/retention_context_cannot_escape.rs");
    tests.compile_fail("tests/ui/retention_self_cascade.rs");
}
