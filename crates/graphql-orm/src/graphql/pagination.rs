#[derive(async_graphql::SimpleObject, Clone, Debug, Default)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
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
