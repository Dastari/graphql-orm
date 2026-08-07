use graphql_orm_router::{RouterBuilder, StaticSubgraph};

fn main() {
    let config = RouterBuilder::new("127.0.0.1:4000".parse().unwrap())
        .allow_anonymous_development(true)
        .with_subgraph(StaticSubgraph::new(
            "catalog",
            "http://127.0.0.1:4100/graphql",
            "http://127.0.0.1:4100/schema.graphql",
        ));

    // Production hosts install a resource-server authentication provider and
    // then call `config.prepare().await?.run_until_shutdown(signal).await`.
    println!("{config:?}");
}
