use crate::backend::BackendKind;
use crate::entity::is_option_type;
use crate::entity::{
    EntityMetadata, FieldMetadata, parse_field_metadata, relation_column_identity,
};
use crate::naming::graphql_field_name;
use crate::relations::classify_relation_value_type;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Field, Ident};

pub(super) struct Declaration {
    pub(super) keys: Vec<(Ident, syn::Type)>,
    relations: Vec<Relation>,
    pub(super) child_columns: Vec<String>,
    pub(super) child_tenant: Option<usize>,
    pub(super) child_owner: Option<usize>,
}

struct Relation {
    field: Ident,
    graphql_name: String,
    target: Ident,
    from: Vec<Ident>,
    to: Vec<String>,
    tenant_index: Option<usize>,
    owner_index: Option<usize>,
}

impl Declaration {
    pub(super) fn parse(
        fields: &syn::punctuated::Punctuated<Field, syn::Token![,]>,
        entity: &EntityMetadata,
        backend: BackendKind,
        pk: &Ident,
        rename_fields: Option<&str>,
        serde_rename_all: Option<&str>,
    ) -> syn::Result<Self> {
        let mut child_columns = Vec::new();
        let parsed = fields
            .iter()
            .map(|field| Ok((field, parse_field_metadata(field)?)))
            .collect::<syn::Result<Vec<_>>>()?;

        let find_field = |name: &str, source: &Field| {
            parsed
                .iter()
                .find(|(field, _)| field.ident.as_ref().is_some_and(|id| id == name))
                .ok_or_else(|| {
                    syn::Error::new_spanned(source, format!("unknown nested_create field `{name}`"))
                })
        };
        let validate_binding = |field: &Field, meta: &FieldMetadata, allow_pk: bool| {
            let id = field.ident.as_ref().expect("named field");
            if meta.is_relation
                || meta.skip_db
                || !meta.write
                || meta.managed_version
                || meta.timestamp_behavior(&id.to_string()).exclude_input
                || is_option_type(&field.ty)
                || (!allow_pk
                    && (meta.is_primary_key || id == pk || (meta.skip_input && !meta.input_only)))
                || classify_relation_value_type(&field.ty).is_none()
            {
                return Err(syn::Error::new_spanned(
                    field,
                    "nested_create bindings require non-nullable, writable relation key fields without managed values or write transforms; a child binding must be a public create field other than its primary key",
                ));
            }
            Ok(())
        };
        let key_fields = if let Some((names, span)) = &entity.nested_create_keys {
            names
                .iter()
                .map(|name| {
                    let (field, meta) = parsed
                        .iter()
                        .find(|(field, _)| field.ident.as_ref().is_some_and(|id| id == name))
                        .ok_or_else(|| {
                            syn::Error::new(
                                *span,
                                format!("unknown nested_create_keys field `{name}`"),
                            )
                        })?;
                    validate_binding(field, meta, false)?;
                    child_columns.push(relation_column_identity(
                        backend,
                        meta.db_column.as_deref().unwrap_or(name),
                        false,
                    ));
                    Ok((field.ident.clone().expect("named field"), field.ty.clone()))
                })
                .collect::<syn::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let binding_index = |fields: &[Ident],
                             column: Option<&str>|
         -> syn::Result<Option<usize>> {
            let Some(column) = column else {
                return Ok(None);
            };
            let index = fields.iter().position(|id| {
                parsed.iter().any(|(field, meta)| {
                    field.ident.as_ref() == Some(id)
                        && meta
                            .db_column
                            .as_deref()
                            .map_or_else(|| id == column, |db| db == column)
                })
            });
            index.map(Some).ok_or_else(|| syn::Error::new(pk.span(),
                format!("nested_create bindings must include the declared INSERT RLS scope column `{column}`")))
        };
        let rls = entity.rls.as_ref().and_then(|rls| rls.insert.as_ref());
        let tenant = rls.and_then(|rls| rls.tenant_column.as_deref());
        let owner = rls.and_then(|rls| rls.owner_column.as_deref());
        let mut relations = Vec::new();
        for (field, meta) in &parsed {
            if !meta.relation_nested_create {
                continue;
            }
            if !meta.relation_multiple
                || meta.relation_source_condition.is_some()
                || meta.relation_target_condition.is_some()
            {
                return Err(syn::Error::new_spanned(
                    field,
                    "nested_create requires an unconditional collection relation",
                ));
            }
            let target = meta
                .relation_target
                .as_deref()
                .and_then(|name| syn::parse_str::<Ident>(name).ok())
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        field,
                        "nested_create requires a target entity identifier",
                    )
                })?;
            let source_names = meta
                .relation_from_fields
                .clone()
                .unwrap_or_else(|| vec![pk.to_string()]);
            let to = meta.relation_to_fields.clone().ok_or_else(|| {
                syn::Error::new_spanned(field, "nested_create requires explicit `to` fields")
            })?;
            if source_names.len() != to.len()
                || !source_names.iter().any(|name| pk == name)
                || has_duplicate(&source_names)
                || has_duplicate(&to)
            {
                return Err(syn::Error::new_spanned(
                    field,
                    "nested_create requires distinct, paired from/to fields including the parent's primary key",
                ));
            }
            let to = to
                .iter()
                .map(|name| relation_column_identity(backend, name, true))
                .collect();
            let from = source_names
                .iter()
                .map(|name| {
                    let (source, meta) = find_field(name, field)?;
                    validate_binding(source, meta, true)?;
                    Ok(source.ident.clone().expect("named field"))
                })
                .collect::<syn::Result<Vec<_>>>()?;
            let field = field.ident.clone().expect("named field");
            let graphql_name =
                graphql_field_name(meta, &field.to_string(), rename_fields, serde_rename_all);
            relations.push(Relation {
                tenant_index: binding_index(&from, tenant)?,
                owner_index: binding_index(&from, owner)?,
                field,
                graphql_name,
                target,
                from,
                to,
            });
        }
        let (child_tenant, child_owner) = if key_fields.is_empty() {
            (None, None)
        } else {
            let keys = key_fields
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            (binding_index(&keys, tenant)?, binding_index(&keys, owner)?)
        };
        if (!key_fields.is_empty() || !relations.is_empty())
            && (entity.schema_only || entity.append_only || entity.dependent_deletes)
        {
            return Err(syn::Error::new(
                pk.span(),
                "nested_create requires ordinary writable entities and is incompatible with schema_only, append_only and dependent_deletes",
            ));
        }
        if !relations.is_empty()
            && (entity.upsert.is_some() || entity.ai_mutations.contains_key("create"))
        {
            return Err(syn::Error::new(
                pk.span(),
                "nested_create is incompatible with upsert or an AI create policy on the parent",
            ));
        }
        Ok(Self {
            keys: key_fields,
            relations,
            child_columns,
            child_tenant,
            child_owner,
        })
    }

    pub(super) fn has_relations(&self) -> bool {
        !self.relations.is_empty()
    }

    pub(super) fn validate_input_names(
        &self,
        names: &mut std::collections::HashSet<String>,
    ) -> syn::Result<()> {
        for relation in &self.relations {
            if !names.insert(relation.graphql_name.clone()) {
                return Err(syn::Error::new(
                    relation.field.span(),
                    format!(
                        "duplicate nested_create input field `{}`",
                        relation.graphql_name
                    ),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn argument_description(&self) -> String {
        let lists = self
            .relations
            .iter()
            .map(|relation| {
                format!(
                    "{} [{}!]",
                    relation.graphql_name,
                    child_input(&relation.target)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Parent values and optional child lists ({lists}). The server supplies relation keys. At most 50 child rows total."
        )
    }

    pub(super) fn parent(&self, pk: &Ident) -> Parent {
        let mut parent = Parent::default();
        for relation in &self.relations {
            let Relation {
                field,
                target,
                from,
                to,
                graphql_name,
                tenant_index,
                owner_index,
            } = relation;
            let child_input = child_input(target);
            let nested = format_ident!("__gom_children_{}", field);
            let bind = format_ident!("__gom_binding_{}", field);
            let prepared = format_ident!("__gom_prepared_{}", field);
            let from_values = from.iter().map(|field| {
                if field == pk {
                    quote! { created_pk.clone() }
                } else {
                    quote! { input.#field.clone() }
                }
            });
            let matches = from.iter().enumerate().map(|(idx, field)| {
                let idx = syn::Index::from(idx);
                quote! { entity.#field != #bind.#idx }
            });
            let tenant = position(*tenant_index);
            let owner = position(*owner_index);

            parent.fields.extend(quote! {
                #[doc = "Create related rows in list order in the same transaction. The server supplies the relation keys. At most 50 child rows may be supplied across all lists."]
                #[graphql(name = #graphql_name)]
                #field: ::std::option::Option<::std::vec::Vec<#child_input>>,
            });
            parent.assertions.extend(quote! {
                const _: () = #target::__gom_assert_nested_create_binding(&[#(#to),*], #tenant, #owner);
            });
            parent.extract.extend(quote! {
                let #nested = input.#field.take().unwrap_or_default();
                __gom_child_count = __gom_child_count.saturating_add(#nested.len());
            });
            parent.prepare.extend(quote! {
                let #bind = (#(#from_values,)*);
                let mut #prepared = ::std::vec::Vec::with_capacity(#nested.len());
                let values = ::graphql_orm::graphql::input_validation::__nested_create_list(
                    __gom_nested_values.shift_remove(#graphql_name),
                );
                if values.len() != #nested.len() {
                    return ::std::result::Result::Err(
                        ::graphql_orm::graphql::errors::OrmPublicError::new(
                            ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                        ).into_graphql_error()
                    );
                }
                for (child, values) in #nested.into_iter().zip(values) {
                    let row = #target::__gom_prepare_nested_create(ctx, &__gom_write, db, child, values, &#bind).await?;
                    let (supplied, prepared) = row.__gom_nested_create_keys();
                    let table = #target::__GOM_NESTED_CREATE_TABLE;
                    if supplied.is_some_and(|key| !__gom_supplied_keys.insert((table, key.to_owned())))
                        || !__gom_child_keys.insert((table, prepared.to_owned()))
                    {
                        return ::std::result::Result::Err(
                            ::graphql_orm::graphql::errors::OrmPublicError::with_message(
                                ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                                "duplicate child primary key in nested create",
                            ).into_graphql_error()
                        );
                    }
                    #prepared.push(row);
                }
            });
            parent.write.extend(quote! {
                if !#prepared.is_empty() && (#(#matches)||*) {
                    return ::std::result::Result::Err(
                        ::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error()
                    );
                }
                for row in #prepared {
                    #target::__gom_write_nested_create(ctx, &mut hook_ctx, &__gom_outcome, row).await?;
                }
            });
        }
        parent
    }
}

fn has_duplicate(names: &[String]) -> bool {
    names
        .iter()
        .enumerate()
        .any(|(index, name)| names[..index].contains(name))
}

pub(super) fn position(value: Option<usize>) -> TokenStream {
    match value {
        Some(index) => quote! { ::std::option::Option::Some(#index) },
        None => quote! { ::std::option::Option::<usize>::None },
    }
}

pub(super) fn child_input(entity: &Ident) -> Ident {
    format_ident!("NestedCreate{}Input", entity)
}

#[derive(Default)]
pub(super) struct Parent {
    fields: TokenStream,
    assertions: TokenStream,
    extract: TokenStream,
    prepare: TokenStream,
    write: TokenStream,
}

impl Parent {
    pub(super) fn assertions(&self) -> &TokenStream {
        &self.assertions
    }

    pub(super) fn fields(&self) -> &TokenStream {
        &self.fields
    }

    pub(super) fn extract(&self) -> TokenStream {
        let extract = &self.extract;
        quote! {
            let mut input = input;
            let mut __gom_child_count = 0usize;
            #extract
            if __gom_child_count > 50 {
                return ::std::result::Result::Err(
                    ::graphql_orm::graphql::errors::OrmPublicError::with_message(
                        ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                        "nested create supports at most 50 child rows",
                    ).into_graphql_error()
                );
            }
            let __gom_nested = __gom_child_count != 0;
        }
    }

    pub(super) fn prepare(&self, input_arg_name: &str) -> TokenStream {
        let prepare = &self.prepare;
        quote! {
            let mut __gom_nested_values = if __gom_nested {
                let ::graphql_orm::async_graphql::Value::Object(values) =
                    ::graphql_orm::graphql::input_validation::__nested_create_arguments(ctx, #input_arg_name)?
                else {
                    return ::std::result::Result::Err(
                        ::graphql_orm::graphql::errors::OrmPublicError::new(
                            ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                        ).into_graphql_error()
                    );
                };
                values
            } else {
                ::std::default::Default::default()
            };
            let mut __gom_child_keys = ::std::collections::HashSet::with_capacity(__gom_child_count);
            let mut __gom_supplied_keys = ::std::collections::HashSet::with_capacity(__gom_child_count);
            #prepare
        }
    }

    pub(super) fn write(&self) -> &TokenStream {
        &self.write
    }
}

pub(in crate::operations) struct Child<'a> {
    pub(in crate::operations) keys: &'a [(syn::Ident, syn::Type)],
    pub(in crate::operations) columns: &'a [String],
    pub(in crate::operations) context_bindings: &'a [TokenStream],
    pub(in crate::operations) supplied_pk: Option<&'a syn::Ident>,
    pub(in crate::operations) fields: &'a [TokenStream],
    pub(in crate::operations) conversion: &'a [TokenStream],
    pub(in crate::operations) pk_type: &'a syn::Type,
    pub(in crate::operations) tenant: Option<usize>,
    pub(in crate::operations) owner: Option<usize>,
}

pub(in crate::operations) struct ChildPipeline<'a> {
    pub(in crate::operations) struct_name: &'a Ident,
    pub(in crate::operations) field_case_rule: &'a str,
    pub(in crate::operations) table_name: &'a str,
    pub(in crate::operations) backend_marker: &'a TokenStream,
    pub(in crate::operations) create_mutation_name: &'a str,
    pub(in crate::operations) input_arg_name: &'a str,
    pub(in crate::operations) create_input: &'a Ident,
    pub(in crate::operations) created_key_id: &'a TokenStream,
    /// Request and entity admission of one child row; binds `__gom_entity`
    /// and `__gom_authority`.
    pub(in crate::operations) admit: &'a TokenStream,
    /// Binds `__gom_entity` alone.
    pub(in crate::operations) entity: &'a TokenStream,
    pub(in crate::operations) write_setup: &'a TokenStream,
}

impl Child<'_> {
    pub(in crate::operations) fn generate(
        self,
        pipeline: ChildPipeline<'_>,
        prepare_row: impl FnOnce(TokenStream) -> TokenStream,
        write_row: impl FnOnce(TokenStream) -> TokenStream,
    ) -> TokenStream {
        let ChildPipeline {
            struct_name,
            field_case_rule,
            table_name,
            backend_marker,
            create_mutation_name,
            input_arg_name,
            create_input,
            created_key_id,
            admit,
            entity,
            write_setup,
        } = pipeline;
        let Child {
            keys,
            columns,
            context_bindings,
            supplied_pk,
            fields,
            conversion,
            pk_type,
            tenant,
            owner,
        } = self;
        let nested_input = child_input(struct_name);
        let prepared_type = quote::format_ident!("__GomPreparedNestedCreate{}", struct_name);
        let graphql_name = nested_input.to_string();
        let types = keys.iter().map(|(_, ty)| ty);
        let binding_type = quote! { (#(#types,)*) };
        let supplied_key = supplied_pk.map_or_else(
            || quote! { ::std::option::Option::None },
            |field| {
                quote! {
                    {
                        let created_pk = &input.#field;
                        ::std::option::Option::Some(#created_key_id)
                    }
                }
            },
        );
        let tenant = position(tenant);
        let owner = position(owner);
        let verify_binding = |row: TokenStream| {
            let matches = keys.iter().enumerate().map(|(index, (field, _))| {
                let index = syn::Index::from(index);
                quote! { #row.#field != binding.#index }
            });
            quote! {
                if #(#matches)||* {
                    return ::std::result::Result::Err(
                        ::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error()
                    );
                }
            }
        };
        let prepare_child = prepare_row(verify_binding(quote! { input }));
        let write_child = write_row(verify_binding(quote! { entity }));

        quote! {
            #[doc(hidden)]
            #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug)]
            #[graphql(name = #graphql_name, rename_fields = #field_case_rule)]
            pub struct #nested_input {
                #(#fields)*
            }

            #[doc(hidden)]
            pub struct #prepared_type {
                supplied_key: ::std::option::Option<::std::string::String>,
                prepared_key: ::std::string::String,
                created_pk: #pk_type,
                bind_values: ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue>,
                binding: #binding_type,
                field: ::graphql_orm::async_graphql::Positioned<::graphql_orm::async_graphql::parser::types::Field>,
                authority: ::graphql_orm::graphql::orm::GeneratedWriteAuthority,
            }

            impl #prepared_type {
                #[doc(hidden)]
                pub fn __gom_nested_create_keys(&self) -> (::std::option::Option<&str>, &str) {
                    (self.supplied_key.as_deref(), &self.prepared_key)
                }
            }

            impl #struct_name {
                #[doc(hidden)]
                pub const __GOM_NESTED_CREATE_TABLE: &'static str = #table_name;

                #[doc(hidden)]
                pub const fn __gom_assert_nested_create_binding(
                    fields: &[&str],
                    parent_tenant: ::std::option::Option<usize>,
                    parent_owner: ::std::option::Option<usize>,
                ) {
                    let expected: &[&str] = &[#(#columns),*];
                    assert!(fields.len() == expected.len(),
                        "nested_create relation to fields must match the child's nested_create_keys");
                    let mut field = 0;
                    while field < fields.len() {
                        let actual = fields[field].as_bytes();
                        let expected = expected[field].as_bytes();
                        assert!(actual.len() == expected.len(),
                            "nested_create relation to fields must match the child's nested_create_keys");
                        let mut byte = 0;
                        while byte < actual.len() {
                            assert!(actual[byte] == expected[byte],
                                "nested_create relation to fields must match the child's nested_create_keys");
                            byte += 1;
                        }
                        field += 1;
                    }
                    if let (::std::option::Option::Some(parent), ::std::option::Option::Some(child)) =
                        (parent_tenant, #tenant)
                    {
                        assert!(parent == child, "nested_create must map the parent INSERT RLS tenant column to the child's");
                    }
                    if let (::std::option::Option::Some(parent), ::std::option::Option::Some(child)) =
                        (parent_owner, #owner)
                    {
                        assert!(parent == child, "nested_create must map the parent INSERT RLS owner column to the child's");
                    }
                }

                #[doc(hidden)]
                #[allow(unused_imports)]
                pub async fn __gom_prepare_nested_create(
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                    __gom_parent: &::graphql_orm::graphql::orm::GeneratedWriteAdmission<'_, #backend_marker>,
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    input: #nested_input,
                    values: ::graphql_orm::async_graphql::Value,
                    binding: &#binding_type,
                ) -> ::graphql_orm::async_graphql::Result<#prepared_type> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
                    let ::graphql_orm::async_graphql::Value::Object(mut values) = values else {
                        return ::std::result::Result::Err(
                            ::graphql_orm::graphql::errors::OrmPublicError::new(
                                ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                            ).into_graphql_error()
                        );
                    };
                    #(#context_bindings)*
                    let field = ::graphql_orm::graphql::input_validation::__nested_create_field(
                        ctx, #create_mutation_name, #input_arg_name,
                        ::graphql_orm::async_graphql::Value::Object(values),
                    );
                    let ctx = &ctx.with_field(&field);
                    #admit
                    let __gom_nested = true;
                    let mut input = #create_input { #(#conversion)* };
                    let supplied = #supplied_key;
                    #prepare_child
                    ::std::result::Result::Ok(#prepared_type {
                        supplied_key: supplied,
                        prepared_key: #created_key_id,
                        created_pk,
                        bind_values,
                        binding: binding.clone(),
                        field,
                        authority: __gom_authority,
                    })
                }

                #[doc(hidden)]
                #[allow(unused_imports)]
                pub async fn __gom_write_nested_create(
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                    mut hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    __gom_outcome: &::graphql_orm::graphql::errors::MutationOutcomeTracker,
                    row: #prepared_type,
                ) -> ::graphql_orm::async_graphql::Result<()> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
                    let #prepared_type { created_pk, bind_values, binding, field, authority, .. } = row;
                    let __gom_authority = &authority;
                    let ctx = &ctx.with_field(&field);
                    #entity
                    #write_setup
                    #write_child
                    ::std::result::Result::Ok(())
                }
            }
        }
    }
}
