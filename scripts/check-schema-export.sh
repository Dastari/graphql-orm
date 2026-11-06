#!/usr/bin/env bash
# Run under the caller's Cargo resource limits and target directory.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"
manifest=crates/graphql-orm-macros/fixtures/full-stack/Cargo.toml
logs=$(mktemp -d)
trap 'rm -rf "$logs"' EXIT
cargo fmt --manifest-path "$manifest" --all -- --check
cargo test -p graphql-orm-macros
cargo test -p graphql-orm-macros --features schema-export-only
cargo test --manifest-path "$manifest" --locked --features schema-export-only --lib
for backend in sqlite postgres; do
  cargo run --manifest-path "$manifest" --locked --no-default-features \
    --features "$backend" --example export_sdl >"$logs/normal.sdl"
  cargo run --manifest-path "$manifest" --locked --no-default-features \
    --features "$backend,schema-export-only" --example export_sdl >"$logs/export.sdl"
  cmp "$logs/normal.sdl" "$logs/export.sdl"
  echo "schema-export: $backend SDL is byte-identical ($(wc -c <"$logs/normal.sdl") bytes)"
done
