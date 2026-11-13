pub mod assurance;
pub mod auth;
#[cfg(feature = "auth-agql")]
pub mod auth_agql;
pub mod errors;
pub mod federation;
pub mod filters;
pub mod loaders;
pub mod orm;
pub mod pagination;
/// Compile-time witness for `#[graphql_orm::graphql_policy_object]`.
///
/// Reached by generated code through this absolute path only; it is not part
/// of the prelude.
#[doc(hidden)]
pub mod __policy_object {
    /// Implemented by `#[graphql_orm::graphql_policy_object]` for the struct it
    /// rewrites. ORM derives require it for a `SimpleObject` field
    /// `read_policy`, so the inert marker attribute alone does not stand in for
    /// the guard.
    #[diagnostic::on_unimplemented(
        message = "`{Self}` carries the policy-object marker without `#[graphql_orm::graphql_policy_object]`",
        label = "apply `#[graphql_orm::graphql_policy_object]` before the derives instead of the marker"
    )]
    pub trait Installed {}

    /// Asserts that `T` was rewritten by `graphql_policy_object`.
    pub fn assert_installed<T: Installed>() {}
}
pub mod structural_auth;
pub mod subscriptions;

#[cfg(feature = "runtime-graphql")]
pub mod runtime;

pub(crate) mod subscription_admission;
