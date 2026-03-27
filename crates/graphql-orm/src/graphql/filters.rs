#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct SimilarityInput {
    #[graphql(name = "Value")]
    pub value: String,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct UuidFilter {
    #[graphql(name = "Eq")]
    pub eq: Option<uuid::Uuid>,
    #[graphql(name = "Ne")]
    pub ne: Option<uuid::Uuid>,
    #[graphql(name = "In")]
    pub in_list: Option<Vec<uuid::Uuid>>,
    #[graphql(name = "NotIn")]
    pub not_in: Option<Vec<uuid::Uuid>>,
    #[graphql(name = "IsNull")]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct StringFilter {
    #[graphql(name = "Eq")]
    pub eq: Option<String>,
    #[graphql(name = "Ne")]
    pub ne: Option<String>,
    #[graphql(name = "Contains")]
    pub contains: Option<String>,
    #[graphql(name = "StartsWith")]
    pub starts_with: Option<String>,
    #[graphql(name = "EndsWith")]
    pub ends_with: Option<String>,
    #[graphql(name = "In")]
    pub in_list: Option<Vec<String>>,
    #[graphql(name = "NotIn")]
    pub not_in: Option<Vec<String>>,
    #[graphql(name = "IsNull")]
    pub is_null: Option<bool>,
    #[graphql(name = "Similar")]
    pub similar: Option<SimilarityInput>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct IntFilter {
    #[graphql(name = "Eq")]
    pub eq: Option<i32>,
    #[graphql(name = "Ne")]
    pub ne: Option<i32>,
    #[graphql(name = "Lt")]
    pub lt: Option<i32>,
    #[graphql(name = "Lte")]
    pub lte: Option<i32>,
    #[graphql(name = "Gt")]
    pub gt: Option<i32>,
    #[graphql(name = "Gte")]
    pub gte: Option<i32>,
    #[graphql(name = "In")]
    pub in_list: Option<Vec<i32>>,
    #[graphql(name = "NotIn")]
    pub not_in: Option<Vec<i32>>,
    #[graphql(name = "IsNull")]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct BoolFilter {
    #[graphql(name = "Eq")]
    pub eq: Option<bool>,
    #[graphql(name = "Ne")]
    pub ne: Option<bool>,
    #[graphql(name = "IsNull")]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct DateRangeInput {
    #[graphql(name = "Start")]
    pub start: Option<String>,
    #[graphql(name = "End")]
    pub end: Option<String>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct RelativeDateInput {
    #[graphql(name = "Days")]
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
    #[graphql(name = "Eq")]
    pub eq: Option<String>,
    #[graphql(name = "Ne")]
    pub ne: Option<String>,
    #[graphql(name = "Lt")]
    pub lt: Option<String>,
    #[graphql(name = "Lte")]
    pub lte: Option<String>,
    #[graphql(name = "Gt")]
    pub gt: Option<String>,
    #[graphql(name = "Gte")]
    pub gte: Option<String>,
    #[graphql(name = "Between")]
    pub between: Option<DateRangeInput>,
    #[graphql(name = "IsNull")]
    pub is_null: Option<bool>,
    #[graphql(name = "InPast")]
    pub in_past: Option<bool>,
    #[graphql(name = "InFuture")]
    pub in_future: Option<bool>,
    #[graphql(name = "IsToday")]
    pub is_today: Option<bool>,
    #[graphql(name = "RecentDays")]
    pub recent_days: Option<i32>,
    #[graphql(name = "WithinDays")]
    pub within_days: Option<i32>,
    #[graphql(name = "GteRelative")]
    pub gte_relative: Option<RelativeDateInput>,
    #[graphql(name = "LteRelative")]
    pub lte_relative: Option<RelativeDateInput>,
}
