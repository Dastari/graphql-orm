#![cfg(feature = "sqlite")]

#[test]
fn search_json_attribute_validation() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/search_json_non_json_field.rs");
    t.compile_fail("tests/ui/search_json_invalid_path.rs");
    t.compile_fail("tests/ui/search_json_private_field.rs");
}
