#![allow(clippy::collapsible_if, clippy::if_same_then_else)]

//! Procedural macros for GraphQL/ORM backends.
//!
//! This crate provides macros to reduce boilerplate for an `async-graphql` +
//! SQLx-style backend:
//!
//! - `mutation_result!` - Generate GraphQL mutation result types
//! - `#[derive(GraphQLEntity)]` - Generate GraphQL types, filters, and SQL helpers from a struct
//! - `#[derive(GraphQLSchemaEntity)]` - Generate schema metadata only for migration planning
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
mod naming;
mod operations;
mod relations;
mod schema_roots;

#[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mssql")))]
compile_error!("Enable at least one database backend feature: sqlite, postgres, or mssql.");

#[cfg(any(
    all(feature = "resolver-case-pascal", feature = "resolver-case-snake"),
    all(
        feature = "resolver-case-pascal",
        feature = "resolver-case-screaming-snake"
    ),
    all(feature = "resolver-case-pascal", feature = "resolver-case-lower"),
    all(feature = "resolver-case-pascal", feature = "resolver-case-upper"),
    all(
        feature = "resolver-case-snake",
        feature = "resolver-case-screaming-snake"
    ),
    all(feature = "resolver-case-snake", feature = "resolver-case-lower"),
    all(feature = "resolver-case-snake", feature = "resolver-case-upper"),
    all(
        feature = "resolver-case-screaming-snake",
        feature = "resolver-case-lower"
    ),
    all(
        feature = "resolver-case-screaming-snake",
        feature = "resolver-case-upper"
    ),
    all(feature = "resolver-case-lower", feature = "resolver-case-upper")
))]
compile_error!("Enable at most one resolver-case-* feature at a time.");

#[cfg(any(
    all(feature = "argument-case-pascal", feature = "argument-case-snake"),
    all(
        feature = "argument-case-pascal",
        feature = "argument-case-screaming-snake"
    ),
    all(feature = "argument-case-pascal", feature = "argument-case-lower"),
    all(feature = "argument-case-pascal", feature = "argument-case-upper"),
    all(
        feature = "argument-case-snake",
        feature = "argument-case-screaming-snake"
    ),
    all(feature = "argument-case-snake", feature = "argument-case-lower"),
    all(feature = "argument-case-snake", feature = "argument-case-upper"),
    all(
        feature = "argument-case-screaming-snake",
        feature = "argument-case-lower"
    ),
    all(
        feature = "argument-case-screaming-snake",
        feature = "argument-case-upper"
    ),
    all(feature = "argument-case-lower", feature = "argument-case-upper")
))]
compile_error!("Enable at most one argument-case-* feature at a time.");

#[cfg(any(
    all(feature = "field-case-pascal", feature = "field-case-snake"),
    all(feature = "field-case-pascal", feature = "field-case-screaming-snake"),
    all(feature = "field-case-pascal", feature = "field-case-lower"),
    all(feature = "field-case-pascal", feature = "field-case-upper"),
    all(feature = "field-case-snake", feature = "field-case-screaming-snake"),
    all(feature = "field-case-snake", feature = "field-case-lower"),
    all(feature = "field-case-snake", feature = "field-case-upper"),
    all(feature = "field-case-screaming-snake", feature = "field-case-lower"),
    all(feature = "field-case-screaming-snake", feature = "field-case-upper"),
    all(feature = "field-case-lower", feature = "field-case-upper")
))]
compile_error!("Enable at most one field-case-* feature at a time.");

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
        backup,
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
    GraphQLSchemaEntity,
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
        backup,
        index,
        unique_index
    )
)]
pub fn derive_graphql_schema_entity(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match entity::generate_graphql_schema_entity(&input) {
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
