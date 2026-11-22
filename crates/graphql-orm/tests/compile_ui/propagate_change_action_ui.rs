use crate::ui;

#[test]
fn rejects_misplaced_or_unsupported_propagation_actions() {
    let t = ui::test_cases();
    t.compile_fail("tests/ui/propagate_change_action_misplaced.rs");
}
