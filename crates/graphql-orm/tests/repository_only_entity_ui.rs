#[test]
fn repository_only_entities_enforce_the_non_graphql_boundary() {
    #[cfg(any(feature = "sqlite", feature = "mssql"))]
    let cases = trybuild::TestCases::new();
    #[cfg(feature = "sqlite")]
    {
        cases.compile_fail("tests/ui/repository_only_not_graphql_types.rs");
        cases.compile_fail("tests/ui/repository_only_schema_roots.rs");
        cases.compile_fail("tests/ui/repository_only_graphql_operations.rs");
        cases.compile_fail("tests/ui/repository_only_graphql_relations.rs");
    }
    #[cfg(feature = "mssql")]
    cases.compile_fail("tests/ui/repository_only_mssql_writes.rs");
}
