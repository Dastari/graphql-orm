//! A bound column test that takes the original nested closure rendering.
//! Hand-written filters preserve their evaluation sites.
use graphql_orm::prelude::*;
use std::{any::TypeId, marker::PhantomData};

struct ColumnTest<B, E> {
    sql: String,
    values: Vec<SqlValue>,
    entity: PhantomData<(B, E)>,
}

impl<B, E> DatabaseFilter for ColumnTest<B, E> {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
        (vec![self.sql.clone()], self.values.clone())
    }

    fn is_empty(&self) -> bool {
        false
    }
}

impl<B: OrmBackend, E: Entity + 'static> ReadVisibilityFilter for ColumnTest<B, E> {
    fn entity_type_id() -> TypeId {
        TypeId::of::<E>()
    }

    fn backend() -> DatabaseBackend {
        B::DIALECT
    }

    fn table_name() -> Option<&'static str> {
        Some(E::TABLE_NAME)
    }
}

pub fn predicate<B: OrmBackend, E: Entity + 'static>(
    column: &str,
    comparison: &str,
    value: SqlValue,
) -> ReadPredicate {
    column_test::<B, E>(column, &format!("{comparison} ?"), vec![value])
}

#[allow(dead_code)] // This helper is only needed by the backend parity fixture.
pub fn null_test<B: OrmBackend, E: Entity + 'static>(column: &str, is_null: bool) -> ReadPredicate {
    column_test::<B, E>(
        column,
        if is_null { "IS NULL" } else { "IS NOT NULL" },
        vec![],
    )
}

fn column_test<B: OrmBackend, E: Entity + 'static>(
    column: &str,
    test: &str,
    values: Vec<SqlValue>,
) -> ReadPredicate {
    ReadPredicate::from_filter::<B, _>(&ColumnTest::<B, E> {
        sql: format!(
            "__gom_self.{} {test}",
            B::DIALECT.delimited_column_identifier(column),
        ),
        values,
        entity: PhantomData,
    })
    .unwrap()
}
