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

/// Bind `__gom_entity`: the entity a generated write names in its policy
/// decisions and hook events.
pub(super) fn write_entity(
    entity_name: &str,
    table_name: &str,
    struct_name: &syn::Ident,
) -> TokenStream {
    quote! {
        let __gom_entity = ::graphql_orm::graphql::orm::GeneratedWriteEntity::new(
            #entity_name,
            #table_name,
            <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata,
        );
    }
}

/// Which assurance requirement request admission enforces.
pub(super) enum Assurance<'a> {
    /// The root field being resolved.
    Resolver,
    /// A named mutation: a nested child row enforces its own create root.
    Operation(&'a str),
}

/// Bind `__gom_request`: resolver authentication, the RLS request context,
/// the operation's scopes and its assurance, in the runtime's order.
pub(super) fn admit_request(
    struct_name: &syn::Ident,
    backend_marker: &TokenStream,
    resolver_auth_mode: &TokenStream,
    scope_enforcement: &TokenStream,
    assurance: Assurance<'_>,
) -> TokenStream {
    let assurance = match assurance {
        Assurance::Resolver => {
            quote! { ::graphql_orm::graphql::orm::GeneratedWriteAssurance::Resolver }
        }
        Assurance::Operation(name) => {
            quote! { ::graphql_orm::graphql::orm::GeneratedWriteAssurance::Operation(#name) }
        }
    };
    quote! {
        let __gom_request = ::graphql_orm::graphql::orm::__admit_generated_write_request::<#struct_name, #backend_marker>(
            ctx,
            #resolver_auth_mode,
            &|| {
                #scope_enforcement
                ::std::result::Result::Ok(())
            },
            #assurance,
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
