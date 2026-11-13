#![cfg(feature = "sqlite")]

//! A handwritten root field with the name of a generated operation, or of
//! another handwritten root's field, is a compile error (#282), and the
//! authorization a described root declares must be enforceable (#449).

#[path = "support/ui.rs"]
mod ui;

#[test]
fn colliding_root_fields_are_rejected_at_compile_time() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/root_collision_described_query.rs");
    tests.compile_fail("tests/ui/root_collision_described_mutation.rs");
    tests.compile_fail("tests/ui/root_collision_described_subscription.rs");
    tests.compile_fail("tests/ui/root_collision_extra_mutation.rs");
    tests.compile_fail("tests/ui/root_collision_extra_query_renamed.rs");
    tests.compile_fail("tests/ui/root_collision_between_handwritten_roots.rs");
    tests.compile_fail("tests/ui/root_fields_undeclared_extra_root.rs");
    tests.compile_fail("tests/ui/root_fields_flatten.rs");
    tests.compile_fail("tests/ui/root_fields_cfg_attr_graphql.rs");
    tests.compile_fail("tests/ui/root_collision_between_entities.rs");
    tests.compile_fail("tests/ui/root_collision_within_entity.rs");
    tests.pass("tests/ui/root_collision_unrelated_pass.rs");
}

#[test]
fn unenforceable_custom_authorization_is_rejected_at_compile_time() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/custom_authorization_public_with_scopes.rs");
    tests.compile_fail("tests/ui/custom_authorization_empty_scopes.rs");
    tests.compile_fail("tests/ui/custom_authorization_entity_without_context.rs");
    tests.compile_fail("tests/ui/custom_authorization_replace_unstated.rs");
    tests.compile_fail("tests/ui/custom_authorization_replace_combined.rs");
    tests.compile_fail("tests/ui/custom_authorization_public_resolver_with_scopes.rs");
}
