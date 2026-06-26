#[derive(async_graphql::SimpleObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct PageInfo {
    #[cfg_attr(feature = "field-case-lower", graphql(name = "hasnextpage"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "HASNEXTPAGE"))]
    pub has_next_page: bool,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "haspreviouspage"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "HASPREVIOUSPAGE"))]
    pub has_previous_page: bool,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "startcursor"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "STARTCURSOR"))]
    pub start_cursor: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "endcursor"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ENDCURSOR"))]
    pub end_cursor: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "totalcount"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "TOTALCOUNT"))]
    pub total_count: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct Edge<T> {
    pub node: T,
    pub cursor: String,
}

#[derive(Clone, Debug)]
pub struct Connection<T> {
    pub edges: Vec<Edge<T>>,
    pub page_info: PageInfo,
}

pub fn encode_cursor(offset: i64) -> String {
    offset.to_string()
}
