#[path = "support/ui.rs"]
mod ui;

#[test]
fn schema_only_entities_do_not_emit_graphql_or_crud_types() {
    let t = ui::test_cases();
    t.compile_fail("tests/ui/schema_only_entity_no_graphql.rs");
}
