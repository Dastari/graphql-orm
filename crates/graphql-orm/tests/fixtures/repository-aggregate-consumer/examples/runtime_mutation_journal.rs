//! Compile and execute the public host example without a direct async-graphql dependency.
#[path = "../../../../examples/runtime_mutation_journal.rs"]
mod host_example;
fn main() -> Result<(), Box<dyn std::error::Error>> { host_example::main() }
