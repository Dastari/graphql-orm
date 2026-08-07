#![cfg(feature = "sqlite")]

#[test]
fn invalid_operation_authorization_declarations_are_rejected() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/operation_authorization_templates_without_router_protocol.rs");
    cases.compile_fail("tests/ui/operation_authorization_duplicate_category.rs");
    cases.compile_fail("tests/ui/operation_authorization_duplicate_declaration.rs");
    cases.compile_fail("tests/ui/operation_authorization_empty_categories.rs");
    cases.compile_fail("tests/ui/operation_authorization_empty_scope_alternatives.rs");
    cases.compile_fail("tests/ui/operation_authorization_empty_scopes.rs");
    cases.compile_fail("tests/ui/operation_authorization_empty_all_scopes.rs");
    cases.compile_fail("tests/ui/operation_authorization_invalid_scope.rs");
    cases.compile_fail("tests/ui/operation_authorization_mixed_scope_modes.rs");
    cases.compile_fail("tests/ui/operation_authorization_missing_generated_category.rs");
    cases.compile_fail("tests/ui/operation_authorization_missing_keyset.rs");
    cases.compile_fail("tests/ui/operation_authorization_missing_search.rs");
    cases.compile_fail("tests/ui/operation_authorization_template_complex_argument.rs");
    cases.compile_fail("tests/ui/operation_authorization_template_malformed.rs");
    cases.compile_fail("tests/ui/operation_authorization_template_unknown_argument.rs");
    cases.compile_fail("tests/ui/operation_authorization_unknown_option.rs");
    cases.compile_fail("tests/ui/operation_authorization_unsupported_category.rs");
}
