//! Shared SQL rendering for generated scalar filters.
use super::{DatabaseBackend, SqlDialect, SqlValue};
use crate::graphql::filters::{BoolFilter, IntFilter, StringFilter};

/// Append a generated string filter without changing clause or bind order.
#[doc(hidden)]
pub fn __gom_render_string_filter(
    f: &StringFilter,
    backend: DatabaseBackend,
    column: &str,
    conditions: &mut Vec<String>,
    values: &mut Vec<SqlValue>,
) {
    if let Some(ref v) = f.eq {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} = {}", column, placeholder));
        values.push(super::SqlValue::String(v.clone()));
    }
    if let Some(ref v) = f.ne {
        let placeholder = backend.placeholder(values.len() + 1);
        conditions.push(format!("{} != {}", column, placeholder));
        values.push(super::SqlValue::String(v.clone()));
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
                values.push(super::SqlValue::String(v.clone()));
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
                values.push(super::SqlValue::String(v.clone()));
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
