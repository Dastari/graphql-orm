use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::{ImplItem, Item, Type, parse_quote};

use crate::operations::generate_graphql_operations;

fn generated(composite: bool) -> syn::File {
    let input = if composite {
        parse_quote! {
            #[graphql_entity(table = "records", query_shapes = "list,single")]
            struct Record {
                #[primary_key]
                id: String,
                #[primary_key]
                sequence: i64,
            }
        }
    } else {
        parse_quote! {
            #[graphql_entity(table = "records", aggregate = true, keyset = "id asc")]
            struct Record {
                #[primary_key]
                #[sortable]
                id: String,
                #[graphql_orm(searchable(weight = "A"))]
                title: String,
            }
        }
    };
    syn::parse2(generate_graphql_operations(&input).unwrap()).unwrap()
}

fn assert_qualified(tokens: TokenStream) {
    let mut colons = 0;
    for token in tokens {
        match token {
            TokenTree::Punct(ref punct) if punct.as_char() == ':' => {
                colons += 1;
                continue;
            }
            TokenTree::Ident(ref ident) => {
                let name = ident.to_string();
                if matches!(
                    name.as_str(),
                    "Option" | "Vec" | "Box" | "Some" | "None" | "Ok" | "Err" | "vec"
                ) {
                    assert!(colons >= 2, "unqualified emitted name: {name}");
                }
            }
            TokenTree::Group(group) => assert_qualified(group.stream()),
            _ => {}
        }
        colons = 0;
    }
}

fn assert_resolver(file: &syn::File, name: &str) {
    let resolver = file.items.iter().find_map(|item| {
        let Item::Impl(item) = item else { return None };
        let Type::Path(target) = item.self_ty.as_ref() else {
            return None;
        };
        if !target.path.is_ident("RecordQueries") {
            return None;
        }
        item.items.iter().find_map(|item| match item {
            ImplItem::Fn(method) if method.sig.ident == name => Some(method),
            _ => None,
        })
    });
    let tokens = resolver
        .expect("generated query resolver")
        .to_token_stream();
    assert_qualified(tokens);
}

#[test]
fn list_resolver_uses_qualified_standard_library_names() {
    for composite in [false, true] {
        assert_resolver(&generated(composite), "list");
    }
}
