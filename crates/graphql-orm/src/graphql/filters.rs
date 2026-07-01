#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct SimilarityInput {
    pub value: String,
}

#[derive(
    async_graphql::Enum, serde::Serialize, serde::Deserialize, Copy, Clone, Debug, Eq, PartialEq,
)]
/// Full-text search query interpretation used by generated search resolvers.
pub enum SearchMode {
    /// Tokenize the query as plain words and require backend-default term matching.
    Plain,
    /// Treat the query as a phrase where the backend supports phrase search.
    Phrase,
    /// Use web-search style syntax where the backend supports it.
    Web,
    /// Match sanitized query tokens as prefixes.
    Prefix,
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::Plain
    }
}

#[derive(async_graphql::InputObject, Clone, Debug)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
/// Input accepted by generated per-entity full-text search resolvers.
pub struct SearchInput {
    /// User-provided search text.
    pub query: String,
    /// Optional query mode. Defaults to [`SearchMode::Plain`].
    pub mode: Option<SearchMode>,
    /// Optional minimum relevance score applied before results are returned.
    #[cfg_attr(feature = "field-case-lower", graphql(name = "minscore"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "MINSCORE"))]
    pub min_score: Option<f64>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct UuidFilter {
    pub eq: Option<uuid::Uuid>,
    pub ne: Option<uuid::Uuid>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "inlist"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INLIST"))]
    pub in_list: Option<Vec<uuid::Uuid>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "notin"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "NOTIN"))]
    pub not_in: Option<Vec<uuid::Uuid>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct StringFilter {
    pub eq: Option<String>,
    pub ne: Option<String>,
    pub contains: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "startswith"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "STARTSWITH"))]
    pub starts_with: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "endswith"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ENDSWITH"))]
    pub ends_with: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "inlist"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INLIST"))]
    pub in_list: Option<Vec<String>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "notin"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "NOTIN"))]
    pub not_in: Option<Vec<String>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
    pub similar: Option<SimilarityInput>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
/// Spatial predicate filter for GeoJSON geometry fields.
pub struct SpatialFilter {
    /// Topological equality predicate.
    pub equals: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological disjoint predicate.
    pub disjoint: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological intersection predicate.
    pub intersects: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological touches predicate.
    pub touches: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological crosses predicate.
    pub crosses: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological within predicate.
    pub within: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological contains predicate.
    pub contains: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological overlaps predicate.
    pub overlaps: Option<async_graphql::Json<serde_json::Value>>,
    /// Null check. When set, no geometry value is bound for this predicate.
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct IntFilter {
    pub eq: Option<i32>,
    pub ne: Option<i32>,
    pub lt: Option<i32>,
    pub lte: Option<i32>,
    pub gt: Option<i32>,
    pub gte: Option<i32>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "inlist"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INLIST"))]
    pub in_list: Option<Vec<i32>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "notin"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "NOTIN"))]
    pub not_in: Option<Vec<i32>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct BoolFilter {
    pub eq: Option<bool>,
    pub ne: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct DateRangeInput {
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
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
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct DateFilter {
    pub eq: Option<String>,
    pub ne: Option<String>,
    pub lt: Option<String>,
    pub lte: Option<String>,
    pub gt: Option<String>,
    pub gte: Option<String>,
    pub between: Option<DateRangeInput>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "inpast"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INPAST"))]
    pub in_past: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "infuture"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INFUTURE"))]
    pub in_future: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "istoday"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISTODAY"))]
    pub is_today: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "recentdays"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "RECENTDAYS"))]
    pub recent_days: Option<i32>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "withindays"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "WITHINDAYS"))]
    pub within_days: Option<i32>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "gterelative"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "GTERELATIVE"))]
    pub gte_relative: Option<RelativeDateInput>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "lterelative"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "LTERELATIVE"))]
    pub lte_relative: Option<RelativeDateInput>,
}
