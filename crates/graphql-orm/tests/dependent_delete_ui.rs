#[test]
fn unsupported_dependent_delete_shapes_are_rejected() {
    let t = trybuild::TestCases::new();
    #[cfg(not(feature = "mssql"))]
    t.compile_fail("tests/ui/dependent_delete_composite.rs");
    #[cfg(feature = "mssql")]
    t.compile_fail("tests/ui/dependent_delete_mssql.rs");
}
