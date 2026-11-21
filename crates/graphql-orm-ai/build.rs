//! Select the AI crate's own GraphQL names for this build (#508).
//!
//! `GRAPHQL_ORM_AI_CASE=PascalCase` gives the AI types the coherent PascalCase
//! contract. It requires `GRAPHQL_ORM_RESOLVER_CASE`,
//! `GRAPHQL_ORM_ARGUMENT_CASE` and `GRAPHQL_ORM_FIELD_CASE` to be
//! `PascalCase` too, so the whole schema follows one contract. Unset, empty or
//! `camelCase` keeps camelCase AI names whatever the ORM uses. Any other value
//! fails the build. graphql-orm's build script validates the ORM variables.

const ORM_VARIABLES: [&str; 3] = [
    "GRAPHQL_ORM_RESOLVER_CASE",
    "GRAPHQL_ORM_ARGUMENT_CASE",
    "GRAPHQL_ORM_FIELD_CASE",
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(graphql_case_pascal)");
    println!("cargo::rerun-if-env-changed=GRAPHQL_ORM_AI_CASE");
    for variable in ORM_VARIABLES {
        println!("cargo::rerun-if-env-changed={variable}");
    }
    match std::env::var("GRAPHQL_ORM_AI_CASE").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("" | "camelCase") => {}
        Ok("PascalCase") => {
            for variable in ORM_VARIABLES {
                if std::env::var(variable).as_deref() != Ok("PascalCase") {
                    panic!("GRAPHQL_ORM_AI_CASE=PascalCase requires {variable}=PascalCase");
                }
            }
            println!("cargo::rustc-cfg=graphql_case_pascal");
        }
        _ => panic!("GRAPHQL_ORM_AI_CASE must be camelCase or PascalCase"),
    }
}
