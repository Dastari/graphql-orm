#![cfg(feature = "sqlite")]
#[path = "support/ui.rs"]
mod ui;

#[test]
fn schema_only_entities_prove_relation_target_readability() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/recheck386/schema_only_target_key.rs");
    // Both derives emit anonymous assertions; valid schema-only relations must
    // remain usable without conflicting implementations or duplicate names.
    cases.pass("tests/ui/relations_schema_only_supported.rs");
}
