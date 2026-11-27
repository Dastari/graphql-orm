//! Runtime format, compatibility and typed-default contracts.

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[allow(dead_code)]
#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

mod runtime_compat_goldens;
mod runtime_field_formats;
mod runtime_typed_defaults;
