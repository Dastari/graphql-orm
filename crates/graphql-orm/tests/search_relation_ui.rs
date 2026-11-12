#![cfg(feature = "sqlite")]
#[path = "support/ui.rs"]
mod ui;

/// A `search_relation` field list must name persisted fields of the target.
#[test]
fn search_relation_rejects_unknown_target_fields() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/search_relation_unknown_field.rs");
}
