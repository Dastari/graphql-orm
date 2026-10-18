//! Qualify opt-in replay at build time, never by probing the caller's runtime.
use std::{env, process::Command};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(graphql_orm_replay_qualified_profile)");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RUSTC");
    println!("cargo:rerun-if-env-changed=RUSTC_WRAPPER");
    println!("cargo:rerun-if-env-changed=RUSTC_WORKSPACE_WRAPPER");
    // Ordinary builds do not query the compiler or acquire replay qualification.
    if env::var_os("CARGO_FEATURE_OWNED_PLANNING_REPLAY").is_none() {
        return;
    }
    // A wrapper could substitute a different compiler than the queried RUSTC.
    // Only the directly invoked compiler has been qualified for this recipe.
    if ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"]
        .iter()
        .any(|name| env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return;
    }
    const TARGET: &str = "x86_64-unknown-linux-gnu";
    if env::var("TARGET").as_deref() != Ok(TARGET) || env::var("HOST").as_deref() != Ok(TARGET) {
        return;
    }
    let Some(rustc) = env::var_os("RUSTC") else {
        return;
    };
    let Ok(output) = Command::new(rustc).arg("-vV").output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let Ok(version) = std::str::from_utf8(&output.stdout) else {
        return;
    };
    if version.lines().any(|s| s == "release: 1.97.1")
        && version
            .lines()
            .any(|s| s == "commit-hash: 8bab26f4f68e0e26f0bb7960be334d5b520ea452")
        && version
            .lines()
            .any(|s| s == "host: x86_64-unknown-linux-gnu")
    {
        println!("cargo:rustc-cfg=graphql_orm_replay_qualified_profile");
    }
}
