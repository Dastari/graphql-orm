#[test]
fn invalid_composite_relation_definitions_are_rejected() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/composite_relation_arity_mismatch.rs");
    t.compile_fail("tests/ui/composite_relation_unknown_source.rs");
}
