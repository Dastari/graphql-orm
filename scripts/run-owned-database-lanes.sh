#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  echo "usage: scripts/run-owned-database-lanes.sh [sqlite|postgres|mssql|ai-postgres|all]" >&2
  exit 2
fi

lane=${1:-all}
case "${lane}" in
  sqlite|postgres|mssql|ai-postgres|all) ;;
  *)
    echo "owned-database-lanes: unsupported lane: ${lane}" >&2
    exit 2
    ;;
esac

for variable in DATABASE_URL TEST_DATABASE_URL MSSQL_TEST_DATABASE_URL; do
  if [[ -n "${!variable:-}" ]]; then
    echo "owned-database-lanes: refusing ambient ${variable}; required lanes own disposable infrastructure" >&2
    exit 1
  fi
done

repository_root=$(git rev-parse --show-toplevel)
cd "${repository_root}"

run_sqlite() {
  cargo test -p graphql-orm --locked --no-default-features --features sqlite \
    --test runtime_migrations --test runtime_schema_ir --test runtime_migration_memory \
    --test migration_planner --test migration_apply --test legacy_migration_history \
    -- --test-threads=1
  cargo test -p graphql-orm --no-default-features --features sqlite \
    --test grouped_aggregates --test complete_group_pages --test portable_group_pages --test host_timestamps --locked
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features sqlite --locked
}

run_postgres() {
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test runtime_migrations --test runtime_schema_ir --test runtime_migration_memory \
    --test migration_planner --test migration_apply --test legacy_migration_history \
    -- --test-threads=1
  cargo test -p graphql-orm --no-default-features --features postgres \
    --test grouped_aggregates_postgres --test portable_group_pages --test host_timestamps --locked -- --ignored --test-threads=1
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features postgres --locked --test owned_backend --test portable_groups --test host_timestamps \
    -- --ignored --test-threads=1
}

run_mssql() {
  cargo test -p graphql-orm --no-default-features --features mssql \
    --test mssql_writes --locked -- --ignored --test-threads=1
}

run_ai_postgres() {
  cargo test -p graphql-orm-ai --no-default-features \
    --features postgres,provider-openai --test postgres_parity --locked \
    -- --test-threads=1
}

case "${lane}" in
  sqlite) run_sqlite ;;
  postgres) run_postgres ;;
  mssql) run_mssql ;;
  ai-postgres) run_ai_postgres ;;
  all)
    run_sqlite
    run_postgres
    run_mssql
    run_ai_postgres
    ;;
esac

echo "owned-database-lanes: ${lane} passed without an ambient database URL"
