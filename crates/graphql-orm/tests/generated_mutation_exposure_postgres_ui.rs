#![cfg(feature = "postgres")]

#[test]
fn generated_mutation_exposure_compiles_for_postgres() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/generated_mutations_postgres_none.rs");
}
