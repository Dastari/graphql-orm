#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  echo "usage: scripts/run-owned-database-lanes.sh [sqlite|postgres|postgres-legacy|mssql|ai-postgres|all]" >&2
  exit 2
fi

lane=${1:-all}
case "${lane}" in
  sqlite|postgres|postgres-legacy|mssql|ai-postgres|all) ;;
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
  cargo test -p graphql-orm --locked --no-default-features --features sqlite,runtime-graphql \
    --lib --test runtime_graphql --test runtime_strings --test runtime_queries --test runtime_relations \
    --test runtime_mutations --test runtime_migrations --test runtime_schema_ir --test runtime_migration_memory \
    --test migration_planner --test migration_apply --test legacy_migration_history \
    -- --test-threads=1
  cargo test -p graphql-orm --no-default-features --features sqlite \
    --test grouped_aggregates --test complete_group_pages --test portable_group_pages --test host_timestamps --locked
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features sqlite --locked
}

run_postgres() {
  cargo test -p graphql-orm --locked --no-default-features --features postgres,runtime-graphql \
    --lib --test runtime_graphql --test runtime_mutations --test runtime_migrations --test runtime_schema_ir --test runtime_migration_memory \
    --test migration_planner --test migration_apply --test legacy_migration_history \
    -- --test-threads=1
  cargo test -p graphql-orm --no-default-features --features postgres \
    --test runtime_strings --locked -- --include-ignored --test-threads=1
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features postgres --locked --test runtime_strings \
    -- --ignored --test-threads=1
  cargo test -p graphql-orm --no-default-features --features postgres \
    --test grouped_aggregates_postgres --test portable_group_pages --test host_timestamps --locked -- --ignored --test-threads=1
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features postgres --locked --test owned_backend --test portable_groups --test host_timestamps \
    -- --ignored --test-threads=1
}

run_postgres_legacy() {
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test app_repository_helpers \
    --test backup_roundtrip \
    --test bidirectional_join_relations \
    --test binary_keys \
    --test composite_mutations \
    --test conditional_indexes \
    --test contract_gaps \
    --test entity_policy \
    --test generated_subscriptions \
    --test generated_upserts \
    --test graphql_case_features \
    --test graphql_naming \
    --test json_fields \
    --test lifecycle_cleanup_and_deferred \
    --test lifecycle_hooks \
    --test lifecycle_related_writes \
    --test mutation_context_reads \
    --test nullable_update_clear \
    --test owned_postgres_lifecycle \
    --test portable_primitives_postgres \
    --test postgres_compat \
    --test postgres_rls \
    --test propagated_subscriptions \
    --test read_projections \
    --test recursive_relations \
    --test relation_delete_policy \
    --test renamed_columns \
    --test row_policy_and_input_transform \
    --test schema_only_entity \
    --test schema_stages \
    --test spatial_fields \
    --test transaction_postgres \
    --test trusted_internal_writes \
    --test uuid_contract \
    -- --include-ignored --test-threads=1
}

run_mssql() {
  cargo test -p graphql-orm --no-default-features --features mssql \
    --test mssql_writes --test mssql_integration --locked -- --ignored --test-threads=1
}

run_ai_postgres() {
  cargo test -p graphql-orm-ai --no-default-features \
    --features postgres,provider-openai --test postgres_parity --locked \
    -- --test-threads=1
}

case "${lane}" in
  sqlite) run_sqlite ;;
  postgres) run_postgres ;;
  postgres-legacy) run_postgres_legacy ;;
  mssql) run_mssql ;;
  ai-postgres) run_ai_postgres ;;
  all)
    run_sqlite
    run_postgres
    run_postgres_legacy
    run_mssql
    run_ai_postgres
    ;;
esac

echo "owned-database-lanes: ${lane} passed without an ambient database URL"
