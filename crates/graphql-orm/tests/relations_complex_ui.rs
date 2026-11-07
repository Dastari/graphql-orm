#![cfg(feature = "sqlite")]
//! `GraphQLRelations` beside a generated GraphQL object needs `SimpleObject`
//! and `#[graphql(complex)]`; the bare form is refused with one message that
//! names that declaration. Schema-only entities have no generated object and
//! keep the relation resolvers as their object.

#[test]
fn relations_without_complex_name_the_supported_declaration() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/relations_without_complex.rs");
    cases.pass("tests/ui/relations_schema_only_supported.rs");
}
