#![cfg(feature = "sqlite")]
#[path = "support/ui.rs"]
mod ui;

#[test]
fn skip_false_keeps_legacy_input_exclusions() {
    ui::test_cases().pass("tests/ui/simple_object_skip_false_public.rs");
}
