#![cfg(feature = "sqlite")]

//! `schema_roots!` `require_policies: true` makes a listed entity with no
//! explicit policy decision a compile error that names it (#287).

#[path = "support/ui.rs"]
mod ui;

#[test]
fn listed_entities_need_a_policy_decision_when_required() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/require_policies_undeclared.rs");
    tests.compile_fail("tests/ui/policy_open_with_policy_key.rs");
    tests.compile_fail("tests/ui/policy_marker_unknown_value.rs");
    tests.pass("tests/ui/require_policies_pass.rs");
}
