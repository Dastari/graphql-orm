//! Host composition without a direct async-graphql dependency.
#[path = "../../../../examples/runtime_graphql_reads.rs"]
mod host_example;
#[cfg(all(feature = "runtime-graphql", any(feature = "sqlite", feature = "postgres")))]
fn main() -> Result<(), Box<dyn std::error::Error>> { host_example::main() }
#[cfg(not(all(feature = "runtime-graphql", any(feature = "sqlite", feature = "postgres"))))]
fn main() { host_example::main(); }
