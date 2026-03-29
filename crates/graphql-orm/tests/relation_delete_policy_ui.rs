#[test]
fn invalid_set_null_on_non_nullable_foreign_key_is_rejected() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/relation_set_null_non_nullable.rs");
}
