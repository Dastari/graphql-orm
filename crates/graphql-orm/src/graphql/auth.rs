pub trait AuthExt {
    fn auth_user(&self) -> async_graphql::Result<String>;
}

impl AuthExt for async_graphql::Context<'_> {
    fn auth_user(&self) -> async_graphql::Result<String> {
        self.data_opt::<String>()
            .cloned()
            .ok_or_else(|| async_graphql::Error::new("missing auth user in GraphQL context"))
    }
}
