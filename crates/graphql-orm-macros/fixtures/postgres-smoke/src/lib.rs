use chrono::{DateTime, Utc};
use graphql_orm_macros::GraphQLEntity;
use uuid::Uuid;

pub mod db {
    pub mod postgres_helpers {
        use chrono::{DateTime, Utc};
        use serde::de::DeserializeOwned;
        use uuid::Uuid;

        pub fn int_to_bool(value: i32) -> bool {
            value != 0
        }

        pub fn str_to_uuid(value: &str) -> Result<Uuid, uuid::Error> {
            Uuid::parse_str(value)
        }

        pub fn str_to_datetime(value: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
            chrono::DateTime::parse_from_rfc3339(value).map(|dt| dt.with_timezone(&Utc))
        }

        pub fn json_to_vec<T>(value: &str) -> Vec<T>
        where
            T: DeserializeOwned,
        {
            serde_json::from_str(value).unwrap_or_default()
        }
    }
}

pub mod graphql {
    pub mod orm {
        #[derive(Clone, Debug, PartialEq)]
        pub enum SqlValue {
            String(String),
            Int(i64),
            Float(f64),
            Bool(bool),
            Null,
        }

        pub trait DatabaseEntity {
            const TABLE_NAME: &'static str;
            const PLURAL_NAME: &'static str;
            const PRIMARY_KEY: &'static str;
            const DEFAULT_SORT: &'static str;

            fn column_names() -> &'static [&'static str];
        }

        #[derive(Clone, Debug, PartialEq)]
        pub struct ColumnDef {
            pub name: &'static str,
            pub sql_type: &'static str,
            pub nullable: bool,
            pub is_primary_key: bool,
            pub is_unique: bool,
            pub default: Option<&'static str>,
            pub references: Option<&'static str>,
        }

        #[derive(Clone, Debug, PartialEq)]
        pub struct IndexDef {
            pub name: &'static str,
            pub columns: &'static [&'static str],
            pub is_unique: bool,
        }

        impl IndexDef {
            pub fn new(name: &'static str, columns: &'static [&'static str]) -> Self {
                Self {
                    name,
                    columns,
                    is_unique: false,
                }
            }

            pub fn unique(mut self) -> Self {
                self.is_unique = true;
                self
            }
        }

        pub trait DatabaseSchema {
            fn columns() -> &'static [ColumnDef];
            fn indexes() -> &'static [IndexDef];
            fn composite_unique_indexes() -> &'static [&'static [&'static str]];
        }

        pub trait FromSqlRow: Sized {
            fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error>;
        }

        pub trait DatabaseFilter {
            fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>);
            fn is_empty(&self) -> bool;
        }

        pub trait DatabaseOrderBy {
            fn to_sql_order(&self) -> Option<String>;
        }

        #[derive(async_graphql::Enum, Copy, Clone, Debug, Eq, PartialEq)]
        pub enum OrderDirection {
            Asc,
            Desc,
        }

        impl OrderDirection {
            pub fn to_sql(self) -> &'static str {
                match self {
                    Self::Asc => "ASC",
                    Self::Desc => "DESC",
                }
            }
        }

        pub fn generate_candidate_pattern(value: &str) -> String {
            format!("%{}%", value)
        }
    }

    pub mod filters {
        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct SimilarityInput {
            pub value: String,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct StringFilter {
            pub eq: Option<String>,
            pub ne: Option<String>,
            pub contains: Option<String>,
            pub starts_with: Option<String>,
            pub ends_with: Option<String>,
            pub in_list: Option<Vec<String>>,
            pub not_in: Option<Vec<String>>,
            pub is_null: Option<bool>,
            pub similar: Option<SimilarityInput>,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct IntFilter {
            pub eq: Option<i32>,
            pub ne: Option<i32>,
            pub lt: Option<i32>,
            pub lte: Option<i32>,
            pub gt: Option<i32>,
            pub gte: Option<i32>,
            pub in_list: Option<Vec<i32>>,
            pub not_in: Option<Vec<i32>>,
            pub is_null: Option<bool>,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct BoolFilter {
            pub eq: Option<bool>,
            pub ne: Option<bool>,
            pub is_null: Option<bool>,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct DateRangeInput {
            pub start: Option<String>,
            pub end: Option<String>,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct RelativeDateInput {
            pub days: i32,
        }

        impl RelativeDateInput {
            pub fn to_sql_expr(&self) -> String {
                format!("CURRENT_DATE + INTERVAL '{} days'", self.days)
            }
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct DateFilter {
            pub eq: Option<String>,
            pub ne: Option<String>,
            pub lt: Option<String>,
            pub lte: Option<String>,
            pub gt: Option<String>,
            pub gte: Option<String>,
            pub between: Option<DateRangeInput>,
            pub is_null: Option<bool>,
            pub in_past: Option<bool>,
            pub in_future: Option<bool>,
            pub is_today: Option<bool>,
            pub recent_days: Option<i32>,
            pub within_days: Option<i32>,
            pub gte_relative: Option<RelativeDateInput>,
            pub lte_relative: Option<RelativeDateInput>,
        }
    }
}

#[derive(GraphQLEntity, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "widgets",
    plural = "Widgets",
    default_sort = "created_at DESC"
)]
pub struct Widget {
    #[primary_key]
    pub id: Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "boolean")]
    pub active: bool,

    #[filterable(type = "date")]
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::graphql::filters::{BoolFilter, DateFilter, StringFilter};
    use super::graphql::orm::{DatabaseFilter, DatabaseSchema, FromSqlRow, SqlValue};
    use super::{Widget, WidgetWhereInput};
    use chrono::Utc;
    use sqlx::Row;
    use uuid::Uuid;

    fn database_url() -> String {
        std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string())
    }

    #[test]
    fn postgres_filter_sql_uses_postgres_syntax() {
        let where_input = WidgetWhereInput {
            name: Some(StringFilter {
                contains: Some("alpha".to_string()),
                ..Default::default()
            }),
            active: Some(BoolFilter {
                eq: Some(true),
                ..Default::default()
            }),
            created_at: Some(DateFilter {
                recent_days: Some(7),
                ..Default::default()
            }),
            ..Default::default()
        };

        let (conditions, values) = where_input.to_sql_conditions();

        assert_eq!(conditions[0], "name ILIKE $1");
        assert_eq!(conditions[1], "active = $2");
        assert_eq!(
            conditions[2],
            "created_at >= (CURRENT_DATE - INTERVAL '7 days')::date AND created_at <= CURRENT_DATE"
        );
        assert_eq!(
            values,
            vec![
                SqlValue::String("%alpha%".to_string()),
                SqlValue::Bool(true),
            ]
        );
    }

    #[test]
    fn postgres_rebind_sql_numbers_placeholders() {
        assert_eq!(
            Widget::__gom_rebind_sql("name = ? AND active = ? AND id = ?", 1),
            "name = $1 AND active = $2 AND id = $3"
        );
        assert_eq!(
            Widget::__gom_rebind_sql("name = ?2 AND active = ?9", 4),
            "name = $4 AND active = $5"
        );
    }

    #[test]
    fn postgres_schema_types_are_native() {
        let columns = Widget::columns();
        let id_col = columns.iter().find(|c| c.name == "id").unwrap();
        let active_col = columns.iter().find(|c| c.name == "active").unwrap();
        let created_col = columns.iter().find(|c| c.name == "created_at").unwrap();

        assert_eq!(id_col.sql_type, "UUID");
        assert_eq!(active_col.sql_type, "BOOLEAN");
        assert_eq!(created_col.sql_type, "TIMESTAMPTZ");
        assert_eq!(
            created_col.default,
            Some("(EXTRACT(EPOCH FROM NOW())::bigint)")
        );
    }

    #[tokio::test]
    async fn postgres_row_decode_handles_native_types() -> Result<(), Box<dyn std::error::Error>> {
        let pool = sqlx::PgPool::connect(&database_url()).await?;

        sqlx::query("DROP TABLE IF EXISTS widgets")
            .execute(&pool)
            .await?;
        sqlx::query(
            "CREATE TABLE widgets (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL,
                active BOOLEAN NOT NULL,
                created_at TIMESTAMPTZ NOT NULL
            )",
        )
        .execute(&pool)
        .await?;

        let id = Uuid::new_v4();
        let now = Utc::now();

        sqlx::query("INSERT INTO widgets (id, name, active, created_at) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind("postgres-row")
            .bind(true)
            .bind(now)
            .execute(&pool)
            .await?;

        let row = sqlx::query("SELECT id, name, active, created_at FROM widgets WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;

        let decoded = Widget::from_row(&row)?;
        assert_eq!(decoded.id, id);
        assert_eq!(decoded.name, "postgres-row");
        assert!(decoded.active);
        assert_eq!(row.try_get::<bool, _>("active")?, decoded.active);

        Ok(())
    }
}
