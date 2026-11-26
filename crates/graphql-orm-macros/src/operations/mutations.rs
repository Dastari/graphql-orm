pub(super) mod create;
pub(super) mod delete;
pub(super) mod update;
pub(super) mod upsert;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

// Generated writes call the runtime write pipeline
// (`graphql_orm::graphql::orm::write_pipeline`) for their authorization
// sequence. These helpers emit those calls; the order of the checks lives in
// the runtime, not here.

/// Bind `__gom_entity`: the entity a generated write names in its hook
/// events, as its own declaration names it.
pub(super) fn write_entity(struct_name: &syn::Ident) -> TokenStream {
    quote! {
        let __gom_entity = ::graphql_orm::graphql::orm::GeneratedWriteEntity::of::<#struct_name>();
    }
}

/// Which assurance requirement request admission enforces.
pub(super) enum Assurance {
    /// The root field being resolved.
    Resolver,
    /// The operation's own root: a nested child row enforces its entity's
    /// create root.
    Operation,
}

/// Bind `__gom_request`: request admission of the operation `kind` (a
/// `GeneratedWriteKind` variant) of the entity. The runtime reads the
/// authentication mode, the scope requirement and the operation's coordinate
/// from the entity's `GeneratedWriteConfig`; `scope_arguments` are the
/// request's values of the scalar arguments a scope template names.
pub(super) fn admit_request(
    struct_name: &syn::Ident,
    backend_marker: &TokenStream,
    kind: &str,
    scope_arguments: &TokenStream,
    assurance: Assurance,
) -> TokenStream {
    let kind = format_ident!("{}", kind);
    let assurance = match assurance {
        Assurance::Resolver => format_ident!("Resolver"),
        Assurance::Operation => format_ident!("Operation"),
    };
    quote! {
        let __gom_request = ::graphql_orm::graphql::orm::__admit_generated_write_request::<#struct_name, #backend_marker>(
            ctx,
            ::graphql_orm::graphql::orm::GeneratedWriteKind::#kind,
            ::graphql_orm::graphql::orm::GeneratedWriteAssurance::#assurance,
            #scope_arguments,
        )?;
    }
}

/// One hook dispatch of a generated write.
pub(super) struct Hook<'a> {
    /// `Option<(&Context, &GeneratedWriteAuthority)>`: the request and its
    /// admission, or `None` for a repository write.
    pub(super) request: &'a TokenStream,
    /// `Before` or `After`.
    pub(super) phase: &'a str,
    pub(super) action: TokenStream,
    pub(super) id: TokenStream,
    pub(super) changes: TokenStream,
    pub(super) before_state: TokenStream,
    pub(super) after_state: TokenStream,
}

impl Hook<'_> {
    /// The dispatch future on `hook_ctx`, for the entity bound as
    /// `__gom_entity`. The caller awaits it and maps its error.
    pub(super) fn dispatch(self) -> TokenStream {
        let Self {
            request,
            phase,
            action,
            id,
            changes,
            before_state,
            after_state,
        } = self;
        let phase = format_ident!("{}", phase);
        quote! {
            hook_ctx.__run_generated_write_hook(
                &__gom_entity,
                #request,
                ::graphql_orm::graphql::orm::MutationPhase::#phase,
                #action,
                #id,
                #changes,
                #before_state,
                #after_state,
            )
        }
    }
}

/// The request of a GraphQL write whose authority is bound as
/// `__gom_authority`.
pub(super) fn admitted_request() -> TokenStream {
    quote! { ::std::option::Option::Some((ctx, __gom_authority)) }
}
