//! Post-processing for dedicated SDL export builds.
use proc_macro2::TokenStream;

pub(crate) fn process(tokens: TokenStream) -> syn::Result<TokenStream> {
    // Keep the normal expansion verbatim, even if it is not a syn::File.
    #[cfg(not(feature = "schema-export-only"))]
    return Ok(tokens);
    #[cfg(feature = "schema-export-only")]
    transform(tokens)
}

#[cfg(any(feature = "schema-export-only", test))]
use quote::{ToTokens, quote};
#[cfg(any(feature = "schema-export-only", test))]
use syn::{GenericArgument, PathArguments, ReturnType, Type, TypeParamBound, visit_mut::VisitMut};

#[cfg(any(feature = "schema-export-only", test))]
fn transform(tokens: TokenStream) -> syn::Result<TokenStream> {
    struct Replace {
        error: Option<syn::Error>,
    }
    impl Replace {
        fn body(&mut self, sig: &syn::Signature, body: &mut syn::Block) -> bool {
            let output = match &sig.output {
                ReturnType::Type(_, ty) => Some(ty.as_ref()),
                ReturnType::Default => None,
            };
            if sig.asyncness.is_none() && !output.is_some_and(is_runtime_type) {
                return false;
            }
            match output.map(witness).transpose() {
                Ok(witness) => {
                    // A generic panic helper avoids exposing a never-typed
                    // expression to async-graphql's subsequent body rewrites.
                    let result = match witness.flatten() {
                        Some(witness) => {
                            quote!(if false { #witness } else { __graphql_orm_export_panic() })
                        }
                        None => quote!(__graphql_orm_export_panic()),
                    };
                    *body = syn::parse_quote!({
                        fn __graphql_orm_export_panic<T>() -> T {
                            panic!("schema-export-only generated body must not be called")
                        }
                        #result
                    });
                }
                Err(error) => self.error = Some(error),
            }
            true
        }
    }
    impl VisitMut for Replace {
        fn visit_impl_item_fn_mut(&mut self, node: &mut syn::ImplItemFn) {
            if self.body(&node.sig, &mut node.block) {
                node.attrs
                    .push(syn::parse_quote!(#[allow(unused_variables)]));
            }
        }
        fn visit_item_fn_mut(&mut self, node: &mut syn::ItemFn) {
            if self.body(&node.sig, &mut node.block) {
                node.attrs
                    .push(syn::parse_quote!(#[allow(unused_variables)]));
            }
        }
        fn visit_trait_item_fn_mut(&mut self, node: &mut syn::TraitItemFn) {
            if let Some(body) = &mut node.default {
                if self.body(&node.sig, body) {
                    node.attrs
                        .push(syn::parse_quote!(#[allow(unused_variables)]));
                }
            }
        }
    }
    let mut file: syn::File = syn::parse2(tokens)?;
    let mut replace = Replace { error: None };
    replace.visit_file_mut(&mut file);
    match replace.error {
        Some(error) => Err(error),
        None => Ok(file.into_token_stream()),
    }
}

#[cfg(any(feature = "schema-export-only", test))]
fn is_runtime_type(ty: &Type) -> bool {
    struct Find(bool);
    impl VisitMut for Find {
        fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
            if let Some(segment) = node.path.segments.last() {
                self.0 |= matches!(
                    segment.ident.to_string().as_str(),
                    "Future"
                        | "Stream"
                        | "BoxFuture"
                        | "LocalBoxFuture"
                        | "BoxStream"
                        | "LocalBoxStream"
                );
            }
            syn::visit_mut::visit_type_path_mut(self, node);
        }
        fn visit_trait_bound_mut(&mut self, node: &mut syn::TraitBound) {
            if let Some(segment) = node.path.segments.last() {
                self.0 |= segment.ident == "Future" || segment.ident == "Stream";
            }
            syn::visit_mut::visit_trait_bound_mut(self, node);
        }
    }
    let mut find = Find(false);
    find.visit_type_mut(&mut ty.clone());
    find.0
}

#[cfg(any(feature = "schema-export-only", test))]
fn has_opaque(ty: &Type) -> bool {
    struct Find(bool);
    impl VisitMut for Find {
        fn visit_type_impl_trait_mut(&mut self, _: &mut syn::TypeImplTrait) {
            self.0 = true;
        }
    }
    let mut find = Find(false);
    find.visit_type_mut(&mut ty.clone());
    find.0
}

// Only a known Result success wrapper is synthesized. Arbitrary public wrappers
// cannot be constructed safely from their spelling alone.
#[cfg(any(feature = "schema-export-only", test))]
fn witness(ty: &Type) -> syn::Result<Option<TokenStream>> {
    if !has_opaque(ty) {
        return Ok(None);
    }
    if let Type::ImplTrait(opaque) = ty {
        for bound in &opaque.bounds {
            if let TypeParamBound::Trait(bound) = bound {
                let segment = bound.path.segments.last().unwrap();
                let associated = match segment.ident.to_string().as_str() {
                    "Stream" => "Item",
                    "Future" => "Output",
                    _ => continue,
                };
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    for arg in &args.args {
                        if let GenericArgument::AssocType(binding) = arg {
                            if binding.ident == associated && !has_opaque(&binding.ty) {
                                let ty = &binding.ty;
                                return Ok(Some(if associated == "Item" {
                                    quote!(::graphql_orm::futures::stream::empty::<#ty>())
                                } else {
                                    quote!(::core::future::pending::<#ty>())
                                }));
                            }
                        }
                    }
                }
            }
        }
    }
    if let Type::Path(path) = ty {
        let segment = path.path.segments.last().unwrap();
        if segment.ident == "Result" {
            if let PathArguments::AngleBracketed(args) = &segment.arguments {
                if let Some(GenericArgument::Type(success)) = args.args.first() {
                    // Error and other type arguments must remain concrete.
                    if !args
                        .args
                        .iter()
                        .skip(1)
                        .any(|arg| matches!(arg, GenericArgument::Type(ty) if has_opaque(ty)))
                    {
                        if let Some(inner) = witness(success)? {
                            return Ok(Some(quote!(::core::result::Result::Ok(#inner))));
                        }
                    }
                }
            }
        }
    }
    Err(syn::Error::new_spanned(
        ty,
        "schema-export-only: unsupported opaque return wrapper; expected impl Stream<Item = T>, impl Future<Output = T>, or Result of one of these",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_async_body() {
        let output = transform(quote!(
            async fn read() -> u32 {
                expensive_runtime()
            }
        ))
        .unwrap()
        .to_string();
        assert!(output.contains("panic"));
        assert!(!output.contains("expensive_runtime"));
    }

    #[test]
    #[cfg(not(feature = "schema-export-only"))]
    fn disabled_is_verbatim_even_for_unparseable_output() {
        let tokens = quote!(this is not a Rust file);
        assert_eq!(
            process(tokens.clone()).unwrap().to_string(),
            tokens.to_string()
        );
        let input: syn::DeriveInput = syn::parse_quote! {
            #[graphql_entity(table = "records", plural = "Records")]
            struct Record { #[primary_key] id: String }
        };
        for generated in [
            crate::operations::generate_graphql_operations(&input).unwrap(),
            crate::relations::generate_graphql_relations(&input).unwrap(),
        ] {
            assert_eq!(
                process(generated.clone()).unwrap().to_string(),
                generated.to_string()
            );
        }
    }

    // Compile the actual post-processed tokens, then execute each body. A tiny
    // local Stream model keeps this test independent of the runtime crate.
    #[test]
    fn witnesses_compile_and_bodies_panic() {
        let generated = transform(
            include_str!("../tests/ui/schema_export_witness.rs")
                .parse()
                .unwrap(),
        )
        .unwrap();
        let source = format!(
            r#"
            extern crate self as graphql_orm;
            pub mod futures {{
                pub trait Stream {{ type Item; }}
                pub mod stream {{
                    pub struct Empty<T>(core::marker::PhantomData<T>);
                    impl<T> super::Stream for Empty<T> {{ type Item = T; }}
                    pub fn empty<T>() -> Empty<T> {{ Empty(core::marker::PhantomData) }}
                }}
            }}
            struct ChangedEvent;
            {generated}
            fn poll<F: core::future::Future>(future: F) {{
                let mut future = Box::pin(future);
                let waker = std::task::Waker::noop();
                let mut cx = std::task::Context::from_waker(waker);
                let _ = future.as_mut().poll(&mut cx);
            }}
            fn main() {{
                assert!(std::panic::catch_unwind(|| poll(plain_async())).is_err());
                assert!(std::panic::catch_unwind(future).is_err());
                assert!(std::panic::catch_unwind(stream).is_err());
                assert!(std::panic::catch_unwind(wrapped).is_err());
                assert!(std::panic::catch_unwind(|| poll(async_wrapped())).is_err());
                assert!(std::panic::catch_unwind(boxed).is_err());
            }}
        "#
        );
        compile_probe(&source, true);
    }

    #[test]
    fn unsupported_wrapper_is_compile_error() {
        let error = transform(
            include_str!("../tests/ui/schema_export_unsupported.rs")
                .parse()
                .unwrap(),
        )
        .unwrap_err();
        compile_probe(&error.into_compile_error().to_string(), false);
    }

    fn compile_probe(source: &str, success: bool) {
        let dir = std::env::temp_dir().join(format!(
            "graphql-orm-export-{}-{}",
            std::process::id(),
            success
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe.rs");
        let binary = dir.join("probe");
        std::fs::write(&path, source).unwrap();
        let output = std::process::Command::new("rustc")
            .args(["--edition=2024", "-Awarnings"])
            .arg(&path)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if success {
            assert!(
                std::process::Command::new(&binary)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        } else {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("schema-export-only: unsupported opaque return wrapper")
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preserves_sync_and_signature() {
        let tokens = quote!(impl X {
            #[doc = "metadata"] pub fn metadata<T>() -> u32 { 42 }
            pub async fn read<T>(&self, value: T) -> Result<T> { runtime(value) }
        });
        let file: syn::File = syn::parse2(transform(tokens).unwrap()).unwrap();
        let syn::Item::Impl(item) = &file.items[0] else {
            panic!()
        };
        let syn::ImplItem::Fn(sync) = &item.items[0] else {
            panic!()
        };
        assert_eq!(
            sync.to_token_stream().to_string(),
            quote!(
                #[doc = "metadata"]
                pub fn metadata<T>() -> u32 {
                    42
                }
            )
            .to_string()
        );
    }
}
