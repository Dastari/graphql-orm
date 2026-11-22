//! Shorten repeated absolute standard-library paths in generated impls.
//!
//! A generated scope imports the exact same symbols under local aliases. Keeping
//! these imports together avoids widening many thousands of repeated type and
//! constructor paths for a model. The anonymous constant creates a lexical
//! scope without changing how relative user callback paths resolve.

use proc_macro2::{TokenStream, TokenTree};
use quote::{ToTokens, quote};
use syn::visit_mut::{self, VisitMut};

const PATHS: &[(&[&str], &str)] = &[
    (&["boxed", "Box"], "_Box"),
    (&["marker", "Send"], "_Send"),
    (&["marker", "Sync"], "_Sync"),
    (&["option", "Option", "None"], "_None"),
    (&["option", "Option", "Some"], "_Some"),
    (&["option", "Option"], "_Option"),
    (&["result", "Result", "Err"], "_Err"),
    (&["result", "Result", "Ok"], "_Ok"),
    (&["result", "Result"], "_Result"),
    (&["string", "String"], "_String"),
    (&["vec", "Vec"], "_Vec"),
];

fn contains_alias(tokens: TokenStream) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(ident) => {
            let name = ident.to_string();
            let name = name.strip_prefix("r#").unwrap_or(&name);
            PATHS.iter().any(|(_, alias)| *alias == name)
        }
        TokenTree::Group(group) => contains_alias(group.stream()),
        _ => false,
    })
}

pub(crate) fn use_standard_paths(tokens: TokenStream) -> syn::Result<TokenStream> {
    if contains_alias(tokens.clone()) {
        // Preserve the scope of any matching user identifiers.
        return Ok(tokens);
    }
    let mut file: syn::File = syn::parse2(tokens)?;
    let mut rewrite = Imports::default();
    let mut implementations = Vec::new();
    file.items.retain_mut(|item| {
        let syn::Item::Impl(implementation) = item else {
            return true;
        };
        rewrite.changed = false;
        rewrite.visit_item_impl_mut(implementation);
        if !rewrite.changed {
            return true;
        }
        implementations.push(implementation.to_token_stream());
        false
    });
    if implementations.is_empty() {
        return Ok(file.into_token_stream());
    }

    let imports = PATHS
        .iter()
        .zip(rewrite.used)
        .filter_map(|((parts, alias), used)| {
            if !used {
                return None;
            }
            let parts = parts
                .iter()
                .map(|part| syn::Ident::new(part, proc_macro2::Span::call_site()));
            let alias = syn::Ident::new(alias, proc_macro2::Span::call_site());
            Some(quote! { use ::std #( ::#parts )* as #alias; })
        });
    Ok(quote! {
        #file
        const _: () = {
            #(#imports)*
            #(#implementations)*
        };
    })
}

#[derive(Default)]
struct Imports {
    changed: bool,
    used: [bool; PATHS.len()],
}

impl VisitMut for Imports {
    fn visit_impl_item_fn_mut(&mut self, method: &mut syn::ImplItemFn) {
        self.visit_block_mut(&mut method.block);
    }

    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        visit_mut::visit_path_mut(self, path);
        if path.leading_colon.is_none()
            || path
                .segments
                .first()
                .is_none_or(|segment| segment.ident != "std")
            || path
                .segments
                .iter()
                .rev()
                .skip(1)
                .any(|segment| !segment.arguments.is_empty())
        {
            return;
        }
        for (index, (parts, alias)) in PATHS.iter().enumerate() {
            if path.segments.len() <= parts.len()
                || !path
                    .segments
                    .iter()
                    .skip(1)
                    .zip(*parts)
                    .all(|(segment, part)| segment.ident == *part)
            {
                continue;
            }
            let mut remaining = std::mem::take(&mut path.segments)
                .into_iter()
                .skip(parts.len());
            let Some(mut imported) = remaining.next() else {
                return;
            };
            imported.ident = syn::Ident::new(alias, imported.ident.span());
            path.leading_colon = None;
            path.segments = std::iter::once(imported).chain(remaining).collect();
            self.changed = true;
            self.used[index] = true;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_raw_identifier_keeps_its_binding() {
        let expansion = quote! {
            impl Record {
                fn example() {
                    r#_None();
                    let _ = ::std::option::Option::None::<i64>;
                }
            }
        };
        let optimized = use_standard_paths(expansion.clone()).unwrap();
        assert_eq!(expansion.to_string(), optimized.to_string());
    }
    #[test]
    fn typed_variant_callbacks_keep_intermediate_arguments() {
        let expansion = quote! {
            impl Record {
                fn example() {
                    user_callback(
                        ::std::option::Option::<i64>::Some,
                        ::std::result::Result::<i64, CallbackError>::Ok,
                    );
                    let _ = ::std::option::Option::None::<i64>;
                }
            }
        };
        let optimized = use_standard_paths(expansion).unwrap().to_string();
        assert!(optimized.contains(&quote!(::std::option::Option::<i64>::Some).to_string()));
        assert!(
            optimized
                .contains(&quote!(::std::result::Result::<i64, CallbackError>::Ok).to_string())
        );
        assert!(optimized.contains(&quote!(_None::<i64>).to_string()));
    }
}
