#![cfg(feature = "sqlite")]
//! Macro-time diagnostics for `federation_key` declarations.
//!
//! A `@key` is a public contract: other subgraphs join on it and the owning
//! subgraph must be able to resolve exactly one row from it. Every way that
//! contract can be broken is diagnosed here rather than deferred to composition
//! or to a runtime miss.

#[test]
fn invalid_federation_key_declarations_are_rejected() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/federation_key_unknown_field.rs");
    cases.compile_fail("tests/ui/federation_key_unknown_option.rs");
    cases.compile_fail("tests/ui/federation_key_private_field.rs");
    cases.compile_fail("tests/ui/federation_key_read_policy_field.rs");
    cases.compile_fail("tests/ui/federation_key_nullable_field.rs");
    cases.compile_fail("tests/ui/federation_key_skipped_field.rs");
    cases.compile_fail("tests/ui/federation_key_not_unique.rs");
    cases.compile_fail("tests/ui/federation_key_redundant_assume_unique.rs");
    cases.compile_fail("tests/ui/federation_key_duplicate.rs");
    cases.compile_fail("tests/ui/federation_key_repository_entity.rs");
    cases.compile_fail("tests/ui/federation_key_schema_only_entity.rs");
    cases.compile_fail("tests/ui/federation_key_without_operations.rs");
}
