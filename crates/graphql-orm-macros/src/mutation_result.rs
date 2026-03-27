use super::*;

pub(crate) struct MutationResultInput {
    name: Ident,
    field: Option<(Ident, syn::Type)>,
}

impl Parse for MutationResultInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;

        let field = if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let field_name: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let field_type: syn::Type = input.parse()?;
            Some((field_name, field_type))
        } else {
            None
        };

        Ok(MutationResultInput { name, field })
    }
}

pub(crate) fn expand(parsed: MutationResultInput) -> proc_macro2::TokenStream {
    let struct_name = &parsed.name;

    if let Some((field_name, field_type)) = parsed.field {
        quote! {
            #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
            pub struct #struct_name {
                pub success: bool,
                pub error: Option<String>,
                pub #field_name: Option<#field_type>,
            }

            impl #struct_name {
                pub fn ok(#field_name: #field_type) -> Self {
                    Self {
                        success: true,
                        error: None,
                        #field_name: Some(#field_name),
                    }
                }

                pub fn err(msg: impl Into<String>) -> Self {
                    Self {
                        success: false,
                        error: Some(msg.into()),
                        #field_name: None,
                    }
                }
            }
        }
    } else {
        quote! {
            #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
            pub struct #struct_name {
                pub success: bool,
                pub error: Option<String>,
            }

            impl #struct_name {
                pub fn ok() -> Self {
                    Self {
                        success: true,
                        error: None,
                    }
                }

                pub fn err(msg: impl Into<String>) -> Self {
                    Self {
                        success: false,
                        error: Some(msg.into()),
                    }
                }
            }
        }
    }
}
