#![allow(clippy::collapsible_if, clippy::if_same_then_else)]

//! Procedural macros for `graphql-orm`.
//!
//! Applications normally depend on `graphql-orm` and use these macros through
//! its re-exports. The macros generate `async-graphql` types and resolver code
//! that targets the `graphql-orm` runtime crate.
//!
//! - `mutation_result!` - Generate GraphQL mutation result types
//! - `#[derive(GraphQLEntity)]` - Generate GraphQL types, filters, and SQL helpers from a struct
//! - `#[derive(GraphQLSchemaEntity)]` - Generate schema metadata only for migration planning
//! - `#[derive(GraphQLRelations)]` - Generate relation loading with look_ahead support
//! - `#[derive(GraphQLOperations)]` - Generate Query/Mutation/Subscription structs
//! - `schema_roots!` - Generate root query/mutation/subscription types for a set of entities
//!
//! # Schema Root Mutation Exposure
//!
//! `schema_roots!` accepts `generated_mutations: "all" | "none" |
//! "allowlist" | "denylist"`. The default is `"all"`, preserving existing
//! generated GraphQL mutations. `"none"` omits generated entity mutations from
//! the public `MutationRoot` while keeping generated repository writes and
//! write inputs available. `"allowlist"` uses
//! `generated_mutation_allowlist: [Entity]`; `"denylist"` uses
//! `generated_mutation_denylist: [Entity]`. `extra_mutation_types` are still
//! merged in every mode.
//!
//! ```ignore
//! schema_roots! {
//!     generated_mutations: "none",
//!     entities: [User, Record, Storage],
//!     extra_mutation_types: [AppMutations],
//! }
//! ```
//!
//! # Generated Resolver Auth
//!
//! `schema_roots!` and `#[graphql_entity(...)]` accept
//! `auth = "required" | "optional" | "none"`. Entity-level auth overrides the
//! schema-root mode.
//!
//! ```ignore
//! #[graphql_entity(table = "pages", plural = "Pages", auth = "none")]
//! pub struct Page {
//!     #[primary_key]
//!     pub id: String,
//! }
//!
//! schema_roots! {
//!     auth: "required",
//!     query_custom_ops: [],
//!     entities: [User],
//! }
//! ```
//!
//! `required` calls the runtime `AuthSubject` helper before generated database
//! work. `optional` reads auth only when present and lets policy hooks decide.
//! `none` leaves generated resolvers public.
//!
//! # Backends
//!
//! The macro crate mirrors the runtime backend features: `sqlite`, `postgres`,
//! and `mssql`. When multiple backend features are enabled, entity derives must
//! select a backend explicitly:
//!
//! ```ignore
//! #[graphql_entity(backend = "mssql", table = "dbo.Jobs", schema_policy = "external_read_only")]
//! pub struct Job {
//!     #[primary_key]
//!     pub job_id: i32,
//! }
//! ```
//!
//! Naming feature groups (`resolver-case-*`, `argument-case-*`, and
//! `field-case-*`) remain independent from backend selection.
//!
//! # Search And Spatial Attributes
//!
//! The `graphql_orm` attribute namespace carries backend-neutral feature
//! metadata used by the runtime schema model and generated operations.
//!
//! Spatial fields are represented as GeoJSON in Rust and GraphQL:
//!
//! ```ignore
//! #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326, index = true))]
//! #[filterable(type = "spatial")]
//! pub location: serde_json::Value;
//! ```
//!
//! PostgreSQL stores these fields as PostGIS `geometry` columns. SQLite stores
//! canonical GeoJSON in `TEXT` columns and uses runtime predicate evaluation.
//!
//! Full-text search is enabled with entity, field, and optional relation
//! metadata:
//!
//! ```ignore
//! #[graphql_orm(search(index = true, language = "english"))]
//! pub struct Article {
//!     #[graphql_orm(searchable(weight = "A"))]
//!     pub title: String,
//!
//!     #[graphql(skip)]
//!     #[relation(target = "Tag", from = "id", to = "article_id", multiple)]
//!     #[graphql_orm(search_relation(fields = ["label"], weight = "D"))]
//!     pub tags: Vec<Tag>,
//! }
//! ```
//!
//! Searchable fields must be persisted `String` or `Option<String>` fields.
//! Private fields cannot be searchable, and read-policy-protected fields need
//! an explicit `searchable(policy = "...")` opt-in.

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
/// Generate GraphQL mutation result object types.
///
/// Most users get this macro through `graphql_orm::mutation_result`.
pub fn mutation_result(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as mutation_result::MutationResultInput);
    mutation_result::expand(parsed).into()
}

#[proc_macro_derive(
    GraphQLEntity,
    attributes(
        graphql_entity,
        graphql_rls,
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
/// Derive the full entity runtime contract for a persisted GraphQL object.
///
/// This derive emits metadata, filters, order inputs, row decoding, query SQL
/// helpers, optional write inputs, and backup descriptors depending on field
/// attributes and backend capabilities.
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
        graphql_rls,
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
/// Derive schema metadata without generating GraphQL operations.
///
/// Use this for entities that should participate in schema validation or
/// migration planning but do not need generated GraphQL resolvers.
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
        graphql_rls,
        graphql,
        graphql_orm,
        serde,
        relation,
        transform,
        input_only,
        unique
    )
)]
/// Derive relation resolvers and batched relation loading.
///
/// Supports single-column and composite relation keys. For `async-graphql`
/// object fields, pair relation fields with `#[graphql(skip)]` and
/// `#[graphql(complex)]` on the entity type.
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
        graphql_rls,
        graphql,
        graphql_orm,
        serde,
        primary_key,
        transform,
        input_only,
        unique
    )
)]
/// Derive generated root operation types for one entity.
///
/// Read operations are generated for every backend. Mutation and subscription
/// surfaces are generated only when the selected backend and schema policy allow
/// them.
pub fn derive_graphql_operations(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match operations::generate_graphql_operations(&input) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

#[proc_macro]
/// Generate schema root aliases for a set of generated operation types.
///
/// In multi-backend builds, pass `backend` and `schema_policy` explicitly so
/// the macro can choose the correct root behavior.
pub fn schema_roots(input: TokenStream) -> TokenStream {
    schema_roots::expand(input)
}
