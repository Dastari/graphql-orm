//! Opt-in transaction seams generated from the direct GraphQL resolver bodies.
use crate::{
    backend::{BackendKind, backend_marker_tokens, resolve_backend},
    entity::{is_option_type, parse_entity_metadata, parse_field_metadata},
};
use quote::{ToTokens, format_ident, quote};
use syn::{spanned::Spanned, visit_mut::VisitMut};

pub(crate) fn generate(
    input: &syn::DeriveInput,
    output: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let meta = parse_entity_metadata(&input.attrs)?;
    if !meta.dependent_deletes {
        return Ok(output);
    }
    let error = |message| syn::Error::new(input.span(), message);
    if meta.mutation_shapes.as_ref().is_some_and(|shapes| {
        !shapes.iter().any(|s| s == "delete") || !shapes.iter().any(|s| s == "update")
    }) {
        return Err(error(
            "dependent_deletes requires enabled direct delete and update mutation shapes",
        ));
    }
    if meta
        .mutation_shapes
        .as_ref()
        .is_some_and(|shapes| shapes.iter().any(|shape| shape == "delete_many"))
    {
        return Err(error(
            "dependent_deletes disables delete_many; use bounded single-row deletes",
        ));
    }
    let backend = resolve_backend(meta.backend.as_deref(), input.span(), "dependent_deletes")?;
    if backend == BackendKind::Mssql {
        return Err(error(
            "dependent_deletes supports SQLite and PostgreSQL only; MSSQL is unsupported",
        ));
    }
    if meta.append_only || meta.schema_only {
        return Err(error(
            "dependent_deletes requires writable GraphQL entities",
        ));
    }
    let syn::Data::Struct(data) = &input.data else {
        return Err(error("dependent_deletes requires a struct"));
    };
    let keys = data
        .fields
        .iter()
        .filter(|f| parse_field_metadata(f).is_ok_and(|m| m.is_primary_key))
        .collect::<Vec<_>>();
    if keys.len() != 1 {
        return Err(error(
            "dependent_deletes requires a single-column primary key; composite keys are unsupported",
        ));
    }
    for f in &data.fields {
        let m = parse_field_metadata(f)?;
        if m.relation_from_fields.as_ref().is_some_and(|f| f.len() > 1)
            || m.relation_to_fields.as_ref().is_some_and(|f| f.len() > 1)
        {
            return Err(error(
                "dependent_deletes does not support composite-key relations",
            ));
        }
    }
    let name = &input.ident;
    let marker = backend_marker_tokens(backend);
    let pk = keys[0].ident.as_ref().unwrap();
    let pkty = &keys[0].ty;
    let update_input = format_ident!("Update{}Input", name);
    let mut file: syn::File = syn::parse2(output)?;
    let target = format!("{}Mutations", name);
    let implementation = file
        .items
        .iter_mut()
        .find_map(|item| match item {
            syn::Item::Impl(i) if i.self_ty.to_token_stream().to_string() == target => Some(i),
            _ => None,
        })
        .ok_or_else(|| error("dependent_deletes requires generated GraphQL mutation roots"))?;
    let result_type = implementation
        .items
        .iter()
        .find_map(|i| match i {
            syn::ImplItem::Fn(f) if f.sig.ident == "delete" => Some(f.sig.output.clone()),
            _ => None,
        })
        .ok_or_else(|| error("dependent_deletes requires generated delete and update mutations"))?;
    let syn::ReturnType::Type(_, result_return) = result_type else {
        return Err(error("invalid delete return"));
    };
    let mut methods = Vec::new();
    let mut structs = Vec::new();
    let mut prep_names = Vec::new();
    for action in ["delete", "update"] {
        let method = implementation
            .items
            .iter_mut()
            .find_map(|i| match i {
                syn::ImplItem::Fn(f) if f.sig.ident == action => Some(f),
                _ => None,
            })
            .ok_or_else(|| error("dependent_deletes requires delete and update mutation shapes"))?;
        let operation_name = method
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("graphql"))
            .filter_map(|a| {
                a.parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .ok()
            })
            .flatten()
            .find_map(|meta| match meta {
                syn::Meta::NameValue(value) if value.path.is_ident("name") => match value.value {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(name),
                        ..
                    }) => Some(name),
                    _ => None,
                },
                _ => None,
            })
            .ok_or_else(|| error("missing generated mutation coordinate"))?;
        let mut body = method.block.clone();
        // The GraphQL Context deliberately retains the caller's root selection.
        // Internal writes must classify their own generated operation instead.
        struct ExplicitAssurance(syn::LitStr);
        impl VisitMut for ExplicitAssurance {
            fn visit_expr_call_mut(&mut self, call: &mut syn::ExprCall) {
                if let syn::Expr::Path(path) = call.func.as_mut()
                    && path.path.segments.iter().map(|s| s.ident.to_string()).eq([
                        "graphql_orm",
                        "graphql",
                        "assurance",
                        "enforce_resolver_assurance",
                    ])
                    && let Some(last) = path.path.segments.last_mut()
                {
                    last.ident = format_ident!("enforce_operation_assurance");
                    let coordinate = &self.0;
                    call.args.push(syn::parse_quote! { #coordinate });
                }
                syn::visit_mut::visit_expr_call_mut(self, call);
            }
        }
        ExplicitAssurance(operation_name).visit_block_mut(&mut body);
        // Both paths share the same generated checks, transforms and hooks. Only
        // transaction ownership and postcommit dispatch belong to the caller.
        body.stmts.retain(|stmt| !matches!(stmt,syn::Stmt::Local(l) if matches!(&l.pat,syn::Pat::Ident(p) if p.ident=="hook_ctx")));
        body.stmts.retain(|s| !matches!(s,syn::Stmt::Local(l) if matches!(&l.pat,syn::Pat::Type(t) if t.pat.to_token_stream().to_string()=="__gom_expected_version")));
        struct Clean {
            handler: Option<syn::Path>,
            notifications: Vec<syn::Stmt>,
        }
        impl VisitMut for Clean {
            fn visit_block_mut(&mut self, block: &mut syn::Block) {
                fn commits(e: &syn::Expr) -> bool {
                    match e {
                        syn::Expr::Try(e) => commits(&e.expr),
                        syn::Expr::Await(e) => commits(&e.base),
                        syn::Expr::MethodCall(e) => {
                            e.method == "commit_and_emit" || commits(&e.receiver)
                        }
                        _ => false,
                    }
                }
                fn notification(e: &syn::Expr, handler: &Option<syn::Path>) -> bool {
                    match e {
                        syn::Expr::Try(e) => notification(&e.expr, handler),
                        syn::Expr::Await(e) => notification(&e.base, handler),
                        syn::Expr::MethodCall(e) => notification(&e.receiver, handler),
                        syn::Expr::If(e) => notification(&e.cond, handler),
                        syn::Expr::Let(e) => notification(&e.expr, handler),
                        syn::Expr::Call(e) => {
                            matches!(e.func.as_ref(),syn::Expr::Path(p) if handler.as_ref()==Some(&p.path))
                        }
                        _ => false,
                    }
                }
                block.stmts.retain(|stmt| {
                    if matches!(stmt,syn::Stmt::Expr(e,_) if notification(e,&self.handler)) {
                        self.notifications.push(stmt.clone());
                        return false;
                    }
                    !matches!(stmt,syn::Stmt::Expr(e,_) if commits(e))
                });
                syn::visit_mut::visit_block_mut(self, block);
            }
            fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
                // Dependent field denials refuse, never silently discard nulls.
                if matches!(expr,syn::Expr::Assign(a) if matches!(a.left.as_ref(),syn::Expr::Field(f) if matches!(f.base.as_ref(),syn::Expr::Path(p) if p.path.is_ident("input"))) && matches!(a.right.as_ref(),syn::Expr::Path(p) if p.path.is_ident("None")))
                {
                    *expr = syn::parse_quote! { return Err(::graphql_orm::async_graphql::Error::new("Dependent delete refused")) };
                } else {
                    syn::visit_mut::visit_expr_mut(self, expr);
                }
            }
        }
        let mut clean = Clean {
            handler: meta
                .notify_handler
                .as_ref()
                .map(|path| syn::parse_str(path))
                .transpose()?,
            notifications: Vec::new(),
        };
        clean.visit_block_mut(&mut body);
        let mut notifications = clean.notifications;
        // Bodies move out of the mutation-root impl. Preserve user callback
        // paths qualified with its original `Self` (including notify_with).
        struct ResolveSelf(syn::Ident);
        impl VisitMut for ResolveSelf {
            fn visit_path_mut(&mut self, path: &mut syn::Path) {
                if let Some(first) = path.segments.first_mut()
                    && first.ident == "Self"
                {
                    first.ident = self.0.clone();
                }
                syn::visit_mut::visit_path_mut(self, path);
            }
        }
        let mut resolve = ResolveSelf(format_ident!("{}Mutations", name));
        resolve.visit_block_mut(&mut body);
        for notification in &mut notifications {
            resolve.visit_stmt_mut(notification);
        }
        let split=body.stmts.iter().position(|s|matches!(s,syn::Stmt::Local(l) if matches!(&l.pat,syn::Pat::Ident(p) if p.ident=="sql"))).ok_or_else(||error("missing generated write boundary"))?;
        let suffix = body.stmts.split_off(split);
        // Retain direct false/not-found results separately from GraphQL errors.
        // The caller preserves root results and sanitizes dependent rejections.
        struct CaptureRejections(syn::Type);
        impl VisitMut for CaptureRejections {
            fn visit_expr_return_mut(&mut self, r: &mut syn::ExprReturn) {
                if r.expr
                    .as_ref()
                    .is_some_and(|e| e.to_token_stream().to_string().starts_with("Ok"))
                {
                    let result = r.expr.take().unwrap();
                    let result_return = &self.0;
                    r.expr = Some(Box::new(syn::parse_quote! {{
                        let result: #result_return = #result;
                        match result {
                            Ok(result) => Ok(::graphql_orm::graphql::orm::StaticWriteResult::Rejected(result.error.unwrap_or_default())),
                            Err(error) => Err(error),
                        }
                    }}));
                } else {
                    syn::visit_mut::visit_expr_return_mut(self, r);
                }
            }
        }
        CaptureRejections((*result_return).clone()).visit_block_mut(&mut body);
        // Prepare all grants before writes, then recheck the same direct
        // GraphQL authority immediately before SQL: earlier dependent hooks may
        // revoke a later grant. Input transforms and mutation hooks run once.
        let auth_end=body.stmts.iter().position(|stmt| {
            if action=="update" {stmt.to_token_stream().to_string().contains("run_before_update_with_context")}
            else {matches!(stmt,syn::Stmt::Local(l) if matches!(&l.pat,syn::Pat::Ident(p) if p.ident=="before_state"))}
        }).ok_or_else(||error("missing direct GraphQL authority boundary"))?;
        let reauthorize = body.stmts[..auth_end].to_vec();
        let reauthorize_fields = if action == "update" {
            body.stmts
                .iter()
                .filter(|stmt| {
                    stmt.to_token_stream()
                        .to_string()
                        .contains("__gom_field_write_allowed")
                })
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if action == "update" {
            body.stmts.insert(
                auth_end + 1,
                syn::parse_quote! {let __gom_authorized_input=input.clone();},
            );
        }
        let prepared = format_ident!("__Gom{}Prepared{}", name, action);
        let prep = format_ident!("__gom_prepare_graphql_{}", action);
        prep_names.push(prep.clone());
        let (extra_params, extra_fields, extra_init, extra_bind) = if action == "update" {
            (
                quote! {,mut input:#update_input},
                quote! {set_clauses:Vec<String>,values:Vec<::graphql_orm::graphql::orm::SqlValue>,input:#update_input},
                quote! {set_clauses,values,input:__gom_authorized_input,},
                quote! {set_clauses,mut values,input,},
            )
        } else {
            (quote! {}, quote! {}, quote! {}, quote! {})
        };
        let statements = &body.stmts;
        let delete_changes = if action == "delete" {
            quote! {let mutation_changes=Vec::new();}
        } else {
            quote! {}
        };
        methods.push(quote!{
            #[allow(unused_variables, unused_mut)]
            async fn #prep(
                ctx:&::graphql_orm::async_graphql::Context<'_>,
                mut hook_ctx:&mut ::graphql_orm::graphql::orm::MutationContext<'_,#marker>,
                id:#pkty,
                __gom_expected_version:Option<i64>
                #extra_params
            )->::graphql_orm::async_graphql::Result<::graphql_orm::graphql::orm::StaticWriteResult<Box<dyn ::graphql_orm::graphql::orm::StaticPreparedWrite<#marker>>>> {
                #(#statements)*
                #delete_changes
                Ok(::graphql_orm::graphql::orm::StaticWriteResult::Ready(Box::new(#prepared {id,__gom_expected_version,before_state,mutation_changes,#extra_init})))
            }
        });
        let completed = format_ident!("__Gom{}Committed{}", name, action);
        let completion = if notifications.is_empty() {
            quote! {Ok(::graphql_orm::graphql::orm::StaticWriteResult::Ready(None))}
        } else {
            let image = if action == "delete" {
                quote! {entity}
            } else {
                quote! {result.entity.ok_or_else(||::graphql_orm::async_graphql::Error::new("Dependent delete refused"))?}
            };
            structs.push(quote!{
                #[allow(non_camel_case_types)]
                struct #completed {entity:#name}
                impl ::graphql_orm::graphql::orm::StaticCommittedNotification<#marker> for #completed {
                    fn notify<'a>(self:Box<Self>,ctx:&'a ::graphql_orm::async_graphql::Context<'_>,db:&'a ::graphql_orm::db::Database<#marker>)->::graphql_orm::futures::future::BoxFuture<'a,::graphql_orm::async_graphql::Result<()>> {
                        Box::pin(async move {
                            let pool=db.pool();
                            let entity=self.entity;
                            #(#notifications)*
                            Ok(())
                        })
                    }
                }
            });
            quote! {Ok(::graphql_orm::graphql::orm::StaticWriteResult::Ready(Some(Box::new(#completed{entity:#image}) as Box<dyn ::graphql_orm::graphql::orm::StaticCommittedNotification<#marker>>)))}
        };
        structs.push(quote!{
            #[allow(non_camel_case_types)]
            struct #prepared {id:#pkty,__gom_expected_version:Option<i64>,before_state: ::graphql_orm::graphql::orm::EntityState,mutation_changes:Vec<::graphql_orm::graphql::orm::MutationFieldValue>,#extra_fields}
            #[allow(unused_variables, unused_mut)]
            impl ::graphql_orm::graphql::orm::StaticPreparedWrite<#marker> for #prepared {
                fn apply<'a>(self:Box<Self>,ctx:&'a ::graphql_orm::async_graphql::Context<'_>,mut hook_ctx:&'a mut ::graphql_orm::graphql::orm::MutationContext<'_,#marker>)->::graphql_orm::futures::future::BoxFuture<'a,::graphql_orm::async_graphql::Result<::graphql_orm::graphql::orm::StaticWriteResult<::graphql_orm::graphql::orm::StaticWriteCompletion<#marker>>>> {
                    Box::pin(async move {
                        let Self{id,__gom_expected_version,before_state,mutation_changes,#extra_bind}=*self;
                        #(#reauthorize)*
                        #(#reauthorize_fields)*
                        let result: #result_return = async {#(#suffix)*}.await;
                        let result=result?;
                        if !result.success {return Ok(::graphql_orm::graphql::orm::StaticWriteResult::Rejected(result.error.unwrap_or_default()));}
                        #completion
                    })
                }
            }
        });
        if action == "delete" {
            let boundary=method.block.stmts.iter().position(|s|matches!(s,syn::Stmt::Local(l) if matches!(&l.pat,syn::Pat::Ident(p) if p.ident=="hook_ctx"))).ok_or_else(||error("missing direct transaction boundary"))?;
            let end = method.block.stmts.iter().position(|s| matches!(s,syn::Stmt::Local(l) if matches!(&l.pat,syn::Pat::Ident(p) if p.ident=="before_state"))).ok_or_else(||error("missing root authorization boundary"))?;
            let mut admission = syn::Block {
                brace_token: Default::default(),
                stmts: method.block.stmts[..end].to_vec(),
            };
            admission.stmts.remove(boundary);
            struct UnlockedAdmission;
            impl VisitMut for UnlockedAdmission {
                fn visit_expr_method_call_mut(&mut self, call: &mut syn::ExprMethodCall) {
                    if call.method == "fetch_one_for_write_in_transaction" {
                        call.method = format_ident!("fetch_one_with_auth");
                        call.args = syn::parse_quote! { db, auth_context.as_ref() };
                    }
                    syn::visit_mut::visit_expr_method_call_mut(self, call);
                }
            }
            UnlockedAdmission.visit_block_mut(&mut admission);
            let admission = &admission.stmts;
            method.block = syn::parse_quote! {{
                #(#admission)*
                let registry=ctx.data::<::graphql_orm::graphql::orm::StaticDependentDeletes<#marker>>()?;
                let __gom_root_rejection=registry.delete::<#name>(ctx,#name::__gom_key_values(&id).remove(0),__gom_expected_version).await?;
                Ok(::std::default::Default::default())
            }};
        }
    }
    // The old bulk route must not bypass dependent authorization or limits.
    implementation.items.retain(
        |item| !matches!(item,syn::ImplItem::Fn(method) if method.sig.ident=="delete_many"),
    );
    // Build the root result using the concrete generated result type.
    // Discover the actual generic Result<T> argument rather than assuming names.
    let syn::Type::Path(path) = result_return.as_ref() else {
        return Err(error("invalid mutation result type"));
    };
    let syn::PathArguments::AngleBracketed(args) = &path.path.segments.last().unwrap().arguments
    else {
        return Err(error("invalid mutation result arguments"));
    };
    let actual = args.args.first().unwrap();
    for item in &mut implementation.items {
        if let syn::ImplItem::Fn(method) = item {
            if method.sig.ident == "delete" {
                let stmt = method.block.stmts.last_mut().unwrap();
                *stmt = syn::Stmt::Expr(
                    syn::parse_quote! { Ok(match __gom_root_rejection {
                        Some(message) => #actual::err(message),
                        None => #actual {success:true,error:None,entity:None},
                    }) },
                    None,
                );
            }
        }
    }
    let mut null_fields = Vec::new();
    for f in &data.fields {
        let m = parse_field_metadata(f)?;
        if m.is_relation
            || m.skip_db
            || m.is_primary_key
            || !is_option_type(&f.ty)
            || !m.write
            || m.timestamp_behavior(&f.ident.as_ref().unwrap().to_string())
                .exclude_input
            || (m.skip_input && !m.input_only)
        {
            continue;
        }
        let ident = f.ident.as_ref().unwrap();
        // Physical metadata owns column naming. Input members remain Rust names.
        let rust = ident.to_string();
        null_fields.push(quote!{
            if <Self as ::graphql_orm::graphql::orm::Entity>::metadata().fields.iter().any(|f|f.rust_name==#rust && f.name==column) { input.#ident=Some(None);matched=true; }
        });
    }
    let delete_prep = &prep_names[0];
    let update_prep = &prep_names[1];
    Ok(quote! {
        #file
        #(#structs)*
        impl #name {#(#methods)*}
        impl ::graphql_orm::graphql::orm::StaticDeleteEntity<#marker> for #name {
            fn select<'a>(tx:&'a mut ::graphql_orm::graphql::orm::MutationContext<'_,#marker>,column:&'a str,value: ::graphql_orm::graphql::orm::SqlValue,limit:usize)->::graphql_orm::futures::future::BoxFuture<'a,::graphql_orm::async_graphql::Result<Vec<::graphql_orm::graphql::orm::StaticDeleteRow>>> {
                Box::pin(async move {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity,EntityQuery,OrmBackend,SqlDialect};
                    let clause=format!("{} = {}",<#marker as OrmBackend>::DIALECT.quote_identifier(column),Self::__gom_placeholder(1));
                    let rows=EntityQuery::<Self,#marker>::new().where_values(&clause,vec![value]).fetch_bounded_mutation_sentinel_in_transaction(tx,::graphql_orm::graphql::orm::MutationLimit::new(limit as u32).map_err(|_|::graphql_orm::async_graphql::Error::new("Dependent delete refused"))?).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                    Ok(rows.into_iter().map(|entity|::graphql_orm::graphql::orm::StaticDeleteRow{key:Self::__gom_key_values(&entity.#pk).remove(0),entity: ::std::sync::Arc::new(entity)}).collect())
                })
            }
            fn prepare<'a>(ctx:&'a ::graphql_orm::async_graphql::Context<'_>,tx:&'a mut ::graphql_orm::graphql::orm::MutationContext<'_,#marker>,row: ::graphql_orm::graphql::orm::StaticDeleteRow,nulls:Vec<String>,expected:Option<i64>)->::graphql_orm::futures::future::BoxFuture<'a,::graphql_orm::async_graphql::Result<::graphql_orm::graphql::orm::StaticWriteResult<Box<dyn ::graphql_orm::graphql::orm::StaticPreparedWrite<#marker>>>>> {
                Box::pin(async move {
                    let entity=row.entity.downcast_ref::<Self>().ok_or_else(||::graphql_orm::async_graphql::Error::new("Dependent delete refused"))?;
                    if nulls.is_empty() {Self::#delete_prep(ctx,tx,entity.#pk.clone(),expected).await}
                    else {
                        let mut input=#update_input::default();
                        for column in nulls {
                            let mut matched=false;
                            #(#null_fields)*
                            if !matched {return Err(::graphql_orm::async_graphql::Error::new("Dependent delete refused"));}
                        }
                        Self::#update_prep(ctx,tx,entity.#pk.clone(),expected,input).await
                    }
                })
            }
        }
    })
}
