//! Print everything the derives generate that a client or router can observe:
//! the SDL, the operation catalogue and the semantic catalogue.
//! `scripts/compare-generated-schema.sh` compares this output between two revisions.
fn main() {
    println!("# SDL");
    print!("{}", full_stack_fixture::export_sdl());
    println!("# Operation catalogue");
    println!("{:#?}", full_stack_fixture::graphql_orm_operation_catalog());
    println!("# Semantic catalogue");
    println!("{:#?}", full_stack_fixture::graphql_orm_semantic_catalog());
}
