#!/usr/bin/env bash
# Bounded #107 reproducer. Does not change the reviewed CI compiler baseline.
set -euo pipefail
toolchain=${1:-1.99.0}
[[ "$toolchain" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo 'expected numeric Rust release' >&2; exit 2; }
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir "$fixture/src"
cat > "$fixture/Cargo.toml" <<'TOML'
[package]
name = "async-trait-clippy-compatibility"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
async-trait = "=0.1.89"
TOML
cat > "$fixture/src/lib.rs" <<'RUST'
#[async_trait::async_trait]
pub trait Reader: Send + Sync {
    async fn read(&self) -> usize;
}
RUST
cargo "+$toolchain" generate-lockfile --manifest-path "$fixture/Cargo.toml"
cargo "+$toolchain" clippy --manifest-path "$fixture/Cargo.toml" --locked --all-targets -- -D warnings
