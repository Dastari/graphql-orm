#!/usr/bin/env bash
set -euo pipefail
# Invoke under the caller's build resource wrapper, like any other Cargo lane.
python3 scripts/check-msrv.py
export CARGO_BUILD_JOBS=1
cargo check --workspace --all-targets --locked
for backend in postgres mssql; do
  cargo check -p graphql-orm --no-default-features --features "${backend}" --all-targets --locked
done
cargo check -p graphql-orm-ai --no-default-features --features postgres,provider-openai --locked
cargo check -p graphql-orm-ai --no-default-features --features mssql --locked
cargo check -p graphql-orm-storage --no-default-features --features local,s3,smb,azure --locked
cargo check -p graphql-orm-backup --no-default-features --features local,orm-postgres --locked
cargo check -p graphql-orm-router --features auth-agql --locked
