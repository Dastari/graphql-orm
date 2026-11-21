#![cfg(feature = "sqlite")]
use crate::ui;

#[test]
fn write_transforms_fail_closed_for_every_entity_combination() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/transform_write_*.rs");
}
