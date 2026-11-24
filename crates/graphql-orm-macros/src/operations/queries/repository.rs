use proc_macro2::TokenStream;
use quote::quote;

use super::QueryContext;

pub(in crate::operations) struct RepositoryKeyMethodsInput<'a> {
    pub(in crate::operations) pk_type_ty: &'a syn::Type,
    pub(in crate::operations) pool_type: &'a TokenStream,
    pub(in crate::operations) key_type: &'a syn::Ident,
    pub(in crate::operations) has_composite_primary_key: bool,
}

pub(in crate::operations) fn repository_key_methods(
    context: &QueryContext<'_>,
    input: RepositoryKeyMethodsInput<'_>,
) -> TokenStream {
    let RepositoryKeyMethodsInput {
        pk_type_ty,
        pool_type,
        key_type,
        has_composite_primary_key,
    } = input;
    let QueryContext {
        backend_marker,
        entity_name_lit,
        ..
    } = *context;
    let find_by_id_method = if has_composite_primary_key {
        quote! {}
    } else {
        quote! {
            #[allow(clippy::ptr_arg)]
            pub async fn find_by_id(
                db: &::graphql_orm::db::Database<#backend_marker>,
                id: &#pk_type_ty,
            ) -> ::graphql_orm::Result<::std::option::Option<Self>> {
                Self::find_by_key(db, id).await
            }
        }
    };
    let get_by_id_method = if has_composite_primary_key {
        quote! {}
    } else {
        quote! {
            pub async fn get(pool: &#pool_type, id: &#pk_type_ty) -> ::graphql_orm::Result<::std::option::Option<Self>> {
                Self::get_by_key(pool, id).await
            }
        }
    };
    quote! {
        #find_by_id_method

        pub async fn find_by_key(
            db: &::graphql_orm::db::Database<#backend_marker>,
            key: &#key_type,
        ) -> ::graphql_orm::Result<::std::option::Option<Self>> {
            db.ensure_entity_access(
                ::std::option::Option::None,
                #entity_name_lit,
                <Self as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
            ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

            // The Repository-surface predicate narrows the SQL, as for
            // repository list reads; the row still passes `can_read_row`.
            let entity = ::graphql_orm::graphql::orm::with_repository_read_visibility(
                db,
                ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                    .where_values(&Self::__gom_key_where_clause(), Self::__gom_key_values(key)),
            )
            .await?
            .fetch_one(db)
            .await?;

            if let ::std::option::Option::Some(ref loaded) = entity {
                if !db.can_read_row(
                    ::std::option::Option::None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    loaded as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)? {
                    return ::std::result::Result::Ok(::std::option::Option::None);
                }
                for field in <Self as ::graphql_orm::graphql::orm::Entity>::repository_field_policies() {
                    db.ensure_repository_readable_field(
                        ::std::option::Option::None,
                        #entity_name_lit,
                        field.api_name,
                        field.read_policy,
                        ::std::option::Option::Some(loaded as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync)),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                }
            }

            ::std::result::Result::Ok(entity)
        }

        #get_by_id_method

        pub async fn get_by_key(pool: &#pool_type, key: &#key_type) -> ::graphql_orm::Result<::std::option::Option<Self>> {
            ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                .where_values(&Self::__gom_key_where_clause(), Self::__gom_key_values(key))
                .fetch_one(pool)
                .await
        }
    }
}
