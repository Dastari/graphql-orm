//! Procedural macros for GraphQL/ORM backends.
//!
//! This crate provides macros to reduce boilerplate for an `async-graphql` +
//! SQLx-style backend:
//!
//! - `mutation_result!` - Generate GraphQL mutation result types
//! - `#[derive(GraphQLEntity)]` - Generate GraphQL types, filters, and SQL helpers from a struct
//! - `#[derive(GraphQLRelations)]` - Generate relation loading with look_ahead support
//! - `#[derive(GraphQLOperations)]` - Generate Query/Mutation/Subscription structs

use convert_case::{Case, Casing};
use proc_macro::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{
    Data, DeriveInput, Field, Fields, Ident, Meta, Token, parse::Parse, parse::ParseStream,
    parse_macro_input,
};

mod backend;
mod entity;
mod mutation_result;
mod operations;
mod relations;
mod schema_roots;

#[cfg(not(any(
    feature = "sqlite",
    feature = "postgres",
    feature = "mysql",
    feature = "mssql"
)))]
compile_error!("Enable exactly one database backend feature: sqlite, postgres, mysql, or mssql.");

#[cfg(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mysql"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mysql"),
    all(feature = "postgres", feature = "mssql"),
    all(feature = "mysql", feature = "mssql")
))]
compile_error!("Enable only one database backend feature at a time.");

#[proc_macro]
pub fn mutation_result(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as mutation_result::MutationResultInput);
    mutation_result::expand(parsed).into()
}

#[proc_macro_derive(
    GraphQLEntity,
    attributes(
        graphql_entity,
        graphql,
        graphql_orm,
        serde,
        primary_key,
        filterable,
        sortable,
        unique,
        db_column,
        relation,
        skip_db,
        date_field,
        boolean_field,
        json_field,
        transform,
        input_only,
        index,
        unique_index
    )
)]
pub fn derive_graphql_entity(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match entity::generate_graphql_entity(&input) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

#[proc_macro_derive(
    GraphQLRelations,
    attributes(
        graphql_entity,
        graphql,
        graphql_orm,
        serde,
        relation,
        transform,
        input_only,
        unique
    )
)]
pub fn derive_graphql_relations(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match relations::generate_graphql_relations(&input) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

#[proc_macro_derive(
    GraphQLOperations,
    attributes(
        graphql_entity,
        graphql,
        graphql_orm,
        serde,
        primary_key,
        transform,
        input_only,
        unique
    )
)]
pub fn derive_graphql_operations(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match operations::generate_graphql_operations(&input) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

#[proc_macro]
pub fn schema_roots(input: TokenStream) -> TokenStream {
    schema_roots::expand(input)
}
