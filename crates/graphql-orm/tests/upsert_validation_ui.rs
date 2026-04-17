#[test]
fn invalid_upsert_configurations_are_rejected() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/upsert_non_unique_target.rs");
    t.compile_fail("tests/ui/upsert_hidden_graphql_target.rs");
}
