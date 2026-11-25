#[test]
fn reports_invalid_generated_root_options() {
    crate::ui::test_cases().compile_fail("tests/ui/root_opt_out_*.rs");
}
