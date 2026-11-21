#![cfg(feature = "sqlite")]

#[test]
fn generated_query_families_compile_with_shadowed_prelude_names() {
    crate::ui::test_cases().pass("tests/ui/query_hygiene.rs");
}

#[test]
fn malformed_keyset_diagnostic_points_to_the_keyset_literal() {
    crate::ui::test_cases().compile_fail("tests/ui/keyset_order_diagnostic_span.rs");
}
