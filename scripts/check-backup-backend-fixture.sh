#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
manifest=crates/graphql-orm/tests/fixtures/backup-backend-coexistence/Cargo.toml
check=(cargo check --color never --locked --workspace --manifest-path "$manifest")

"${check[@]}"
"${check[@]}" --no-default-features --features backup-service/postgres
"${check[@]}" --no-default-features --features backup-service/postgres,mssql-service/sqlite

if output=$("${check[@]}" --features backup-service/postgres 2>&1); then
  echo 'Both backup adapter features unexpectedly compiled' >&2
  exit 1
fi
printf '%s\n' "$output"
grep -Fxq 'error: select only one graphql-orm-backup adapter feature: orm-sqlite or orm-postgres' <<<"$output"
