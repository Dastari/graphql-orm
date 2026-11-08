#![cfg(feature = "sqlite")]
#[test]
fn unreadable_generated_keys_and_search_are_rejected() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/review386/schema_entity_object.rs");
    cases.pass("tests/ui/review386/repository_private_keys.rs");
    cases.compile_fail("tests/ui/review386/private_relation_key.rs");
    cases.compile_fail("tests/ui/review386/read_false_relation_key.rs");
    cases.compile_fail("tests/ui/review386/input_only_relation_key.rs");
    cases.compile_fail("tests/ui/review386/composite_relation_key.rs");
    cases.compile_fail("tests/ui/review386/self_target_key.rs");
    cases.compile_fail("tests/ui/review386/self_target_renamed.rs");
    cases.compile_fail("tests/ui/review386/self_target_casefold.rs");
    cases.compile_fail("tests/ui/review386/self_target_quoted.rs");
    cases.compile_fail("tests/ui/review386/self_target_unknown.rs");
    cases.compile_fail("tests/ui/review386/private_changed_key.rs");
    cases.compile_fail("tests/ui/review386/composite_changed_key.rs");
    cases.compile_fail("tests/ui/review386/read_false_changed_key.rs");
    cases.compile_fail("tests/ui/review386/input_only_changed_key.rs");
    cases.compile_fail("tests/ui/review386/input_only_relation_order.rs");
    cases.compile_fail("tests/ui/review386/input_only_search.rs");
    cases.compile_fail("tests/ui/review386/read_false_search.rs");
    cases.compile_fail("tests/ui/review386/input_only_search_json.rs");
    cases.compile_fail("tests/ui/review386/read_false_search_json.rs");
    cases.compile_fail("tests/ui/review386/input_only_relation_search.rs");
    cases.compile_fail("tests/ui/review386/input_only_relation_filter.rs");
}
