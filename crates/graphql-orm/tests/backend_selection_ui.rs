#![cfg(all(feature = "sqlite", feature = "mssql"))]

#[test]
fn multi_backend_builds_require_explicit_backend_selection() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/multi_backend_entity_requires_backend.rs");
    t.compile_fail("tests/ui/multi_backend_schema_requires_backend.rs");
}
