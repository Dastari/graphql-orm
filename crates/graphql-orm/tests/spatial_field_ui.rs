#[test]
#[cfg(feature = "postgres")]
fn spatial_field_requires_geojson_value() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/spatial_non_geojson_type.rs");
}

#[test]
#[cfg(feature = "sqlite")]
fn sqlite_spatial_field_is_rejected() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/spatial_sqlite_backend.rs");
}

#[test]
#[cfg(feature = "mssql")]
fn mssql_spatial_field_is_rejected() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/spatial_mssql_backend.rs");
}
