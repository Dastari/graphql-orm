#![cfg(feature = "sqlite")]

#[test]
fn composite_key_write_helpers_are_not_generated() {
    // The absent-helper probe lives in the library's `generated_api_absence_probes`
    // compile_fail doctests, which are independent of rustc diagnostic prose. Only
    // macro-owned diagnostics (stable text) are snapshot-checked here.
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/composite_mutation_nullable_key.rs");
    t.compile_fail("tests/ui/composite_mutation_generated_key.rs");
    t.pass("tests/ui/composite_mutations_sqlx_free.rs");
}
