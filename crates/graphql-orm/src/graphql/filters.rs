#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct SimilarityInput {
    pub value: String,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct UuidFilter {
    pub eq: Option<uuid::Uuid>,
    pub ne: Option<uuid::Uuid>,
    pub in_list: Option<Vec<uuid::Uuid>>,
    pub not_in: Option<Vec<uuid::Uuid>>,
    pub is_null: Option<bool>,
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
        if cfg!(feature = "postgres") {
            format!("CURRENT_DATE + INTERVAL '{} days'", self.days)
        } else {
            format!("date('now', '+{} days')", self.days)
        }
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
