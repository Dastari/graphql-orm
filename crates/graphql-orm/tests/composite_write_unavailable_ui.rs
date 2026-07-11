#![cfg(feature = "sqlite")]

#[test]
fn composite_key_write_helpers_are_not_generated() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/composite_write_helper_unavailable.rs");
    t.compile_fail("tests/ui/composite_mutation_nullable_key.rs");
    t.compile_fail("tests/ui/composite_mutation_generated_key.rs");
    t.pass("tests/ui/composite_mutations_sqlx_free.rs");
}
