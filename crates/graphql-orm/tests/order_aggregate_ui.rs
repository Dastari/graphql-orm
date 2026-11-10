#![cfg(feature = "sqlite")]
#[path = "support/ui.rs"]
mod ui;

/// GHSA-f7xj-6695-ww2q: a per-record relation read policy cannot be enforced
/// on an ORDER BY count, so the declaration is rejected at compile time.
#[test]
fn order_aggregate_rejects_relation_read_policy() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/order_aggregate_read_policy.rs");
}
