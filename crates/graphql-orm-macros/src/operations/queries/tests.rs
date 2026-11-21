use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::{ImplItem, Item, Type, parse_quote};

use crate::operations::generate_graphql_operations;

const ENTITY_SOURCE: &str = include_str!("../../../tests/ui/query_hygiene_entity.inc");

fn generated(composite: bool) -> syn::File {
    let input = if composite {
        parse_quote! {
            #[graphql_entity(table = "records", query_shapes = "list,single", auth = "none")]
            struct Record {
                #[primary_key]
                id: ::std::string::String,
                #[primary_key]
                sequence: i64,
            }
        }
    } else {
        syn::parse_str(ENTITY_SOURCE).unwrap()
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
                    "Option"
                        | "Vec"
                        | "Box"
                        | "Some"
                        | "None"
                        | "Ok"
                        | "Err"
                        | "vec"
                        | "std"
                        | "String"
                        | "Result"
                        | "Send"
                        | "Sync"
                        | "Into"
                        | "IntoIterator"
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

#[test]
fn point_resolver_uses_qualified_standard_library_names() {
    for composite in [false, true] {
        assert_resolver(&generated(composite), "get_by_id");
    }
}

#[test]
fn keyset_and_scan_resolvers_use_qualified_standard_library_names() {
    let generated = generated(false);
    assert_resolver(&generated, "keyset");
    assert_resolver(&generated, "authorized_scan");
}

#[test]
fn search_resolver_uses_qualified_standard_library_names() {
    assert_resolver(&generated(false), "search");
}

#[test]
fn aggregate_resolver_uses_qualified_standard_library_names() {
    assert_resolver(&generated(false), "aggregate");
}

fn targets(implementation: &syn::ItemImpl, name: &str) -> bool {
    matches!(implementation.self_ty.as_ref(), Type::Path(target) if target.path.is_ident(name))
}

#[test]
fn federation_and_generated_query_support_use_qualified_names() {
    let file = generated(false);
    for name in ["__gom_federation_entity_0", "__gom_federation_node_id"] {
        assert_resolver(&file, name);
    }

    for name in [
        "RecordEdge",
        "RecordConnection",
        "RecordSearchEdge",
        "RecordSearchConnection",
        "RecordKeysetOrderByInput",
        "RecordAuthorizedScanPage",
        "RecordAggregateResultRow",
    ] {
        let definition = file
            .items
            .iter()
            .find(|item| matches!(item, Item::Struct(item) if item.ident == name))
            .unwrap_or_else(|| panic!("missing generated {name}"));
        assert_qualified(definition.to_token_stream());
        for item in &file.items {
            if matches!(item, Item::Impl(implementation) if targets(implementation, name)) {
                assert_qualified(item.to_token_stream());
            }
        }
    }

    for name in [
        "AuthorizedKeysetEntity",
        "MutationContextKeysetPage",
        "MutationContextKeysetConnectionPage",
    ] {
        let implementation = file
            .items
            .iter()
            .find(|item| {
                matches!(item, Item::Impl(implementation) if targets(implementation, "Record")
                && implementation.trait_.as_ref().is_some_and(|(_, path, _)| {
                    path.segments.last().is_some_and(|segment| segment.ident == name)
                }))
            })
            .unwrap_or_else(|| panic!("missing generated {name} implementation"));
        assert_qualified(implementation.to_token_stream());
    }

    for name in ["find_by_key", "get"] {
        let method = file
            .items
            .iter()
            .find_map(|item| match item {
                Item::Impl(implementation) if targets(implementation, "Record") => {
                    implementation.items.iter().find_map(|item| match item {
                        ImplItem::Fn(method) if method.sig.ident == name => Some(method),
                        _ => None,
                    })
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing generated {name} method"));
        assert_qualified(method.to_token_stream());
    }

    let input = syn::parse_str(ENTITY_SOURCE).unwrap();
    let (node_fields, names) = crate::entity::generate_complex_identity_fields(&input).unwrap();
    assert!(names.contains(&"NodeId"));
    let implementation: syn::ItemImpl = syn::parse2(quote::quote! {
        impl Record { #node_fields }
    })
    .unwrap();
    let method = implementation
        .items
        .iter()
        .find_map(|item| match item {
            ImplItem::Fn(method) if method.sig.ident == "__gom_node_id" => Some(method),
            _ => None,
        })
        .expect("generated NodeId object field");
    assert_qualified(method.to_token_stream());
}

#[cfg(all(
    feature = "sqlite",
    graphql_orm_resolver_case = "camel",
    graphql_orm_argument_case = "camel",
    graphql_orm_field_case = "camel",
))]
#[test]
fn query_hygiene_trybuild_fixture_matches_the_generator() {
    let file = generated(false);
    let mut items = file
        .items
        .into_iter()
        .filter(|item| match item {
            Item::Struct(item) => {
                item.ident == "RecordQueries" || item.ident == "RecordKeysetOrderByInput"
            }
            Item::Impl(implementation) => {
                targets(implementation, "RecordQueries")
                    || targets(implementation, "RecordKeysetOrderByInput")
            }
            _ => false,
        })
        .collect::<Vec<_>>();
    // Type-check the resolver bodies in the shadowed namespace without
    // expanding async-graphql's Object macro there. Its own generated
    // Federation dispatcher depends on unqualified prelude names.
    for item in &mut items {
        let Item::Impl(implementation) = item else {
            continue;
        };
        if !targets(implementation, "RecordQueries") {
            continue;
        }
        implementation.attrs.retain(|attr| {
            !attr
                .path()
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "Object")
        });
        for item in &mut implementation.items {
            let ImplItem::Fn(method) = item else {
                continue;
            };
            method.attrs.retain(|attr| !attr.path().is_ident("graphql"));
            for input in &mut method.sig.inputs {
                if let syn::FnArg::Typed(argument) = input {
                    argument
                        .attrs
                        .retain(|attr| !attr.path().is_ident("graphql"));
                }
            }
        }
    }
    let emitted = quote::quote! { #(#items)* };
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/ui/query_hygiene_resolvers.inc");
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut formatter = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("workspace toolchain rustfmt");
    formatter
        .stdin
        .take()
        .unwrap()
        .write_all(emitted.to_string().as_bytes())
        .unwrap();
    let output = formatter.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "format the generated query fixture"
    );
    let expected = String::from_utf8(output.stdout).unwrap();
    if std::env::var("UPDATE_QUERY_HYGIENE_FIXTURE").as_deref() == Ok("1") {
        std::fs::write(&fixture, &expected).unwrap();
    }
    let checked_in = std::fs::read_to_string(&fixture).unwrap();
    assert!(
        expected.trim_end() == checked_in.trim_end(),
        "regenerate the query hygiene trybuild fixture with UPDATE_QUERY_HYGIENE_FIXTURE=1"
    );
}
