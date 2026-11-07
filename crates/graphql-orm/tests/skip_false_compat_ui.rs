#![cfg(feature = "sqlite")]
#[test]
fn skip_false_keeps_legacy_input_exclusions() {
    trybuild::TestCases::new().pass("tests/ui/simple_object_skip_false_public.rs");
}
