#![cfg(feature = "postgres")]

#[path = "support/ui.rs"]
mod ui;

#[test]
fn generated_mutation_exposure_compiles_for_postgres() {
    let t = ui::test_cases();
    t.pass("tests/ui/generated_mutations_postgres_none.rs");
}
