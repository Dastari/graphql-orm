#![cfg(feature = "sqlite")]

#[path = "support/ui.rs"]
mod ui;

#[test]
fn generated_mutation_exposure_config_is_validated() {
    let t = ui::test_cases();
    t.compile_fail("tests/ui/generated_mutations_invalid_mode.rs");
    t.compile_fail("tests/ui/generated_mutations_allowlist_missing.rs");
    t.compile_fail("tests/ui/generated_mutations_denylist_missing.rs");
    t.compile_fail("tests/ui/generated_mutations_wrong_list_mode.rs");
    t.compile_fail("tests/ui/generated_mutations_unknown_entity.rs");
}
