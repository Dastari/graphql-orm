#![cfg(feature = "sqlite")]

#[test]
fn append_only_rejects_incompatible_configuration_and_has_no_mutation_methods() {
    // The absent-method probe lives in the library's `generated_api_absence_probes`
    // compile_fail doctests, which are independent of rustc diagnostic prose. Only the
    // macro-owned diagnostic (stable text) is snapshot-checked here.
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/append_only_upsert.rs");
}
