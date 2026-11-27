//! External relation and federation fixture workspaces.

#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[path = "../support/fixture_target.rs"]
mod fixture_target;

mod cross_crate_relations_fixture;
mod federation_end_to_end_fixture;
