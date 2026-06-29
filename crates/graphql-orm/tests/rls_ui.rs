#[test]
fn rls_backend_and_attribute_validation() {
    let t = trybuild::TestCases::new();

    #[cfg(feature = "sqlite")]
    t.compile_fail("tests/ui/rls_sqlite_backend.rs");

    #[cfg(feature = "mssql")]
    t.compile_fail("tests/ui/rls_mssql_backend.rs");

    #[cfg(feature = "postgres")]
    {
        t.pass("tests/ui/rls_postgres_backend.rs");
        t.pass("tests/ui/rls_empty_postgres.rs");
        t.compile_fail("tests/ui/rls_mixed_predicate.rs");
    }
}
