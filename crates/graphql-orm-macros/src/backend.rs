use super::*;

pub(crate) fn backend_row_type_tokens() -> proc_macro2::TokenStream {
    if cfg!(feature = "sqlite") {
        quote! { ::graphql_orm::DbRow }
    } else if cfg!(feature = "postgres") {
        quote! { ::graphql_orm::DbRow }
    } else if cfg!(feature = "mysql") {
        quote! { ::graphql_orm::DbRow }
    } else if cfg!(feature = "mssql") {
        quote! { ::graphql_orm::DbRow }
    } else {
        proc_macro2::TokenStream::new()
    }
}

pub(crate) fn backend_pool_type_tokens() -> proc_macro2::TokenStream {
    if cfg!(feature = "sqlite") {
        quote! { ::graphql_orm::DbPool }
    } else if cfg!(feature = "postgres") {
        quote! { ::graphql_orm::DbPool }
    } else if cfg!(feature = "mysql") {
        quote! { ::graphql_orm::DbPool }
    } else if cfg!(feature = "mssql") {
        quote! { ::graphql_orm::DbPool }
    } else {
        proc_macro2::TokenStream::new()
    }
}

pub(crate) fn backend_database_type_tokens() -> proc_macro2::TokenStream {
    if cfg!(feature = "sqlite") {
        quote! { ::graphql_orm::sqlx::Sqlite }
    } else if cfg!(feature = "postgres") {
        quote! { ::graphql_orm::sqlx::Postgres }
    } else if cfg!(feature = "mysql") {
        quote! { ::graphql_orm::sqlx::MySql }
    } else if cfg!(feature = "mssql") {
        quote! { ::graphql_orm::db::mssql::Mssql }
    } else {
        proc_macro2::TokenStream::new()
    }
}

pub(crate) fn backend_helper_import_tokens() -> proc_macro2::TokenStream {
    if cfg!(feature = "sqlite") {
        quote! { use ::graphql_orm::db::sqlite_helpers::*; }
    } else if cfg!(feature = "postgres") {
        quote! { use ::graphql_orm::db::postgres_helpers::*; }
    } else if cfg!(feature = "mysql") {
        quote! { use ::graphql_orm::db::mysql_helpers::*; }
    } else if cfg!(feature = "mssql") {
        quote! { use ::graphql_orm::db::mssql_helpers::*; }
    } else {
        proc_macro2::TokenStream::new()
    }
}

pub(crate) fn backend_current_epoch_expr() -> &'static str {
    if cfg!(feature = "postgres") {
        "(EXTRACT(EPOCH FROM NOW())::bigint)"
    } else if cfg!(feature = "mssql") {
        "DATEDIFF_BIG(second, '1970-01-01', SYSUTCDATETIME())"
    } else {
        "(unixepoch())"
    }
}

pub(crate) fn backend_quote_identifier(identifier: &str) -> String {
    if cfg!(feature = "mssql") {
        if identifier.starts_with('[') && identifier.ends_with(']') {
            identifier.to_string()
        } else {
            format!("[{}]", identifier.replace(']', "]]"))
        }
    } else {
        identifier.to_string()
    }
}

pub(crate) fn backend_quote_identifier_path(identifier: &str) -> String {
    if cfg!(feature = "mssql") {
        identifier
            .split('.')
            .filter(|part| !part.is_empty())
            .map(backend_quote_identifier)
            .collect::<Vec<_>>()
            .join(".")
    } else {
        identifier.to_string()
    }
}

pub(crate) fn backend_relation_key_cast(column: &str) -> String {
    if cfg!(feature = "mssql") {
        format!(
            "CAST({} AS NVARCHAR(4000))",
            backend_quote_identifier_path(column)
        )
    } else {
        format!("CAST({column} AS TEXT)")
    }
}
