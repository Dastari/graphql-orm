#![cfg(feature = "sqlite")]

#[test]
fn composite_key_write_helpers_are_not_generated() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/composite_write_helper_unavailable.rs");
}
