//! Shared SQL rendering for generated scalar filters.
use super::{DatabaseBackend, SqlDialect, SqlValue};
use crate::graphql::filters::{BoolFilter, IntFilter, StringFilter};

/// Append a generated string filter without changing clause or bind order.
#[doc(hidden)]
pub fn __gom_render_string_filter(
    f: &StringFilter,
    backend: DatabaseBackend,
    column: &str,
    sql_type: &str,
    conditions: &mut Vec<String>,
    values: &mut Vec<SqlValue>,
) {
    let comparison = |value: &String| {
        if backend == DatabaseBackend::Mssql {
            SqlValue::mssql_varchar_comparison(value.clone(), sql_type)
        } else {
            SqlValue::String(value.clone())
        }
    };
    if let Some(ref v) = f.eq {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} = {}", column, placeholder));
        values.push(comparison(v));
    }
    if let Some(ref v) = f.ne {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} != {}", column, placeholder));
        values.push(comparison(v));
    }
    if let Some(ref v) = f.contains {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(backend.ci_like(column, &placeholder));
        values.push(super::SqlValue::String(super::contains_like_pattern(v)));
    }
    if let Some(ref v) = f.starts_with {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(backend.ci_like(column, &placeholder));
        values.push(super::SqlValue::String(super::starts_with_like_pattern(v)));
    }
    if let Some(ref v) = f.ends_with {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(backend.ci_like(column, &placeholder));
        values.push(super::SqlValue::String(super::ends_with_like_pattern(v)));
    }
    if let Some(ref list) = f.in_list {
        if list.is_empty() {
            conditions.push("1 = 0".to_owned());
        } else {
            let base_index = values.len() + 1;
            let placeholders: Vec<String> = (0..list.len())
                .map(|offset| backend.placeholder(base_index + offset))
                .collect();
            conditions.push(format!("{} IN ({})", column, placeholders.join(", ")));
            for v in list {
                values.push(comparison(v));
            }
        }
    }
    if let Some(ref list) = f.not_in {
        if !list.is_empty() {
            let base_index = values.len() + 1;
            let placeholders: Vec<String> = (0..list.len())
                .map(|offset| backend.placeholder(base_index + offset))
                .collect();
            conditions.push(format!("{} NOT IN ({})", column, placeholders.join(", ")));
            for v in list {
                values.push(comparison(v));
            }
        }
    }
    // IsNull / IsNotNull
    if let Some(is_null) = f.is_null {
        if is_null {
            conditions.push(format!("{} IS NULL", column));
        } else {
            conditions.push(format!("{} IS NOT NULL", column));
        }
    }
    // Similar matching uses LIKE for candidate filtering.
    // Actual scoring happens in Rust post-processing
    if let Some(ref sim) = f.similar {
        // Use a broad LIKE pattern to get candidates
        // Generated fallback paths use deterministic substring scoring after fetch.
        let pattern = super::generate_candidate_pattern(&sim.value);
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(backend.ci_like(column, &placeholder));
        values.push(super::SqlValue::String(pattern));
    }
}

/// Render exact SQL Server identity comparisons without changing the default
/// collation-sensitive filters used by existing consumers.
#[doc(hidden)]
pub fn __gom_render_exact_mssql_string_filter(
    f: &StringFilter,
    backend: DatabaseBackend,
    column: &str,
    conditions: &mut Vec<String>,
    values: &mut Vec<SqlValue>,
) {
    render_exact_mssql_string_filter(f, backend, column, conditions, values, false);
}

/// Add seek predicates to exact filters on a declared SQL Server BIN2 Unicode column.
#[doc(hidden)]
pub fn __gom_render_bin2_mssql_string_filter(
    f: &StringFilter,
    backend: DatabaseBackend,
    column: &str,
    conditions: &mut Vec<String>,
    values: &mut Vec<SqlValue>,
) {
    render_exact_mssql_string_filter(f, backend, column, conditions, values, true);
}

fn render_exact_mssql_string_filter(
    f: &StringFilter,
    backend: DatabaseBackend,
    column: &str,
    conditions: &mut Vec<String>,
    values: &mut Vec<SqlValue>,
    seek: bool,
) {
    let mut equal = |value: &String, seek: bool| {
        let value_bind = backend.placeholder(values.len() + 1);
        // Keep both the residual and the bare seek bind as SqlValue::String
        // (NVARCHAR), even if the live column has drifted to VARCHAR.
        values.push(SqlValue::String(value.clone()));
        let unicode_column = format!("CONVERT(nvarchar(max), {column})");
        if seek {
            // SQL Server pads operands for equality even under BIN2. A
            // non-space suffix makes trailing spaces significant. MAX keeps
            // the suffix from being truncated at the 4,000-code-unit limit.
            // CASE keeps a NULL column UNKNOWN under NOT even when the session
            // treats NULL as an empty string in concatenation. Binds are non-NULL.
            return format!(
                "((CASE WHEN {column} IS NOT NULL THEN {unicode_column} + N'.' END) COLLATE Latin1_General_100_BIN2 = CONVERT(nvarchar(max), {value_bind}) + N'.')"
            );
        }
        let length_bind = backend.placeholder(values.len() + 1);
        values.push(SqlValue::String(value.clone()));
        // The explicit collation on the Unicode bind wins the comparison.
        // This lets SQL Server promote VARCHAR using the column's code page
        // before comparing. DATALENGTH needs an explicit Unicode conversion.
        format!(
            "({column} = {value_bind} COLLATE Latin1_General_100_BIN2 AND DATALENGTH({unicode_column}) = DATALENGTH({length_bind}))"
        )
    };
    if let Some(value) = &f.eq {
        conditions.push(equal(value, seek));
    }
    if let Some(value) = &f.ne {
        conditions.push(format!("NOT {}", equal(value, false)));
    }
    for (list, negated) in [(&f.in_list, false), (&f.not_in, true)] {
        if let Some(list) = list {
            if list.is_empty() {
                if !negated {
                    conditions.push("1 = 0".to_owned());
                }
            } else {
                let comparisons = list
                    .iter()
                    .map(|value| equal(value, seek && !negated))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                let not = if negated { "NOT " } else { "" };
                conditions.push(format!("{not}({comparisons})"));
            }
        }
    }
    let mut remaining = f.clone();
    // The bare Unicode column's implicit collation wins over the parameter's
    // coercible-default collation. The residual BIN2 comparison enforces
    // exactness. With a NULL column, both comparisons stay UNKNOWN, including
    // under NOT.
    if !seek {
        remaining.eq = None;
        remaining.in_list = None;
    } else if remaining.in_list.as_ref().is_some_and(Vec::is_empty) {
        remaining.in_list = None; // The exact branch already emitted 1 = 0.
    }
    remaining.ne = None;
    remaining.not_in = None;
    __gom_render_string_filter(&remaining, backend, column, "", conditions, values);
}

/// Append a generated number filter without changing clause or bind order.
#[doc(hidden)]
pub fn __gom_render_number_filter(
    f: &IntFilter,
    backend: DatabaseBackend,
    column: &str,
    conditions: &mut Vec<String>,
    values: &mut Vec<SqlValue>,
) {
    if let Some(v) = f.eq {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} = {}", column, placeholder));
        values.push(super::SqlValue::Int(v as i64));
    }
    if let Some(v) = f.ne {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} != {}", column, placeholder));
        values.push(super::SqlValue::Int(v as i64));
    }
    if let Some(v) = f.lt {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} < {}", column, placeholder));
        values.push(super::SqlValue::Int(v as i64));
    }
    if let Some(v) = f.lte {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} <= {}", column, placeholder));
        values.push(super::SqlValue::Int(v as i64));
    }
    if let Some(v) = f.gt {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} > {}", column, placeholder));
        values.push(super::SqlValue::Int(v as i64));
    }
    if let Some(v) = f.gte {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} >= {}", column, placeholder));
        values.push(super::SqlValue::Int(v as i64));
    }
    if let Some(ref list) = f.in_list {
        if list.is_empty() {
            conditions.push("1 = 0".to_owned());
        } else {
            let base_index = values.len() + 1;
            let placeholders: Vec<String> = (0..list.len())
                .map(|offset| backend.placeholder(base_index + offset))
                .collect();
            conditions.push(format!("{} IN ({})", column, placeholders.join(", ")));
            for v in list {
                values.push(super::SqlValue::Int(*v as i64));
            }
        }
    }
    if let Some(ref list) = f.not_in {
        if !list.is_empty() {
            let base_index = values.len() + 1;
            let placeholders: Vec<String> = (0..list.len())
                .map(|offset| backend.placeholder(base_index + offset))
                .collect();
            conditions.push(format!("{} NOT IN ({})", column, placeholders.join(", ")));
            for v in list {
                values.push(super::SqlValue::Int(*v as i64));
            }
        }
    }
    // IsNull / IsNotNull
    if let Some(is_null) = f.is_null {
        if is_null {
            conditions.push(format!("{} IS NULL", column));
        } else {
            conditions.push(format!("{} IS NOT NULL", column));
        }
    }
}

/// Append a generated boolean filter without changing clause or bind order.
#[doc(hidden)]
pub fn __gom_render_boolean_filter(
    f: &BoolFilter,
    backend: DatabaseBackend,
    column: &str,
    conditions: &mut Vec<String>,
    values: &mut Vec<SqlValue>,
) {
    if let Some(v) = f.eq {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} = {}", column, placeholder));
        values.push(
            if matches!(backend, DatabaseBackend::Postgres | DatabaseBackend::Mssql) {
                SqlValue::Bool(v)
            } else {
                SqlValue::Int(if v { 1 } else { 0 })
            },
        );
    }
    if let Some(v) = f.ne {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} != {}", column, placeholder));
        values.push(
            if matches!(backend, DatabaseBackend::Postgres | DatabaseBackend::Mssql) {
                SqlValue::Bool(v)
            } else {
                SqlValue::Int(if v { 1 } else { 0 })
            },
        );
    }
    // IsNull / IsNotNull
    if let Some(is_null) = f.is_null {
        if is_null {
            conditions.push(format!("{} IS NULL", column));
        } else {
            conditions.push(format!("{} IS NOT NULL", column));
        }
    }
}

/// Fill the canonical sorted, unique operator list from a field's filter input.
#[doc(hidden)]
pub fn __gom_semantic_field(
    mut field: super::GraphqlSemanticFieldMetadata,
) -> super::GraphqlSemanticFieldMetadata {
    field.filter_operators = field
        .filter_input
        .iter()
        .flat_map(|input| input.operators.iter().map(|op| op.operator))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    field
}
