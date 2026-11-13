#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  echo "usage: scripts/run-owned-database-lanes.sh [sqlite|postgres|postgres-legacy|mssql|runtime-live|runtime-search-catalog|ai-postgres|field-write-denial|bulk-refusals|notify-bulk|denied-relations|static-related-filter|read-predicate|defer-after-commit|delete-propagation|dependent-deletes|changed-subscriptions|all]" >&2
  exit 2
fi

lane=${1:-all}
case "${lane}" in
  sqlite|postgres|postgres-legacy|mssql|runtime-live|runtime-search-catalog|ai-postgres|field-write-denial|bulk-refusals|notify-bulk|denied-relations|static-related-filter|read-predicate|defer-after-commit|delete-propagation|dependent-deletes|changed-subscriptions|all) ;;
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

run_dependent_deletes() {
  cargo test -p graphql-orm --locked --no-default-features --features sqlite --test dependent_delete_repro
  cargo test -p graphql-orm --locked --no-default-features --features postgres --test dependent_delete_repro --test dependent_delete_pg_physical -- --ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres --test dependent_delete_ui declared_parent_opt_in_is_enforced_across_crates
  cargo check -p graphql-orm --locked --no-default-features --features mssql --all-targets
  cargo test -p graphql-orm --locked --no-default-features --features mssql --test dependent_delete_ui
}

run_sqlite() {
  cargo test -p graphql-orm --locked --no-default-features --features sqlite --test dependent_delete_repro --test dependent_delete_ui
  cargo test -p graphql-orm --locked --no-default-features --features sqlite,change-journal \
    --test dependent_delete_repro --test backup_roundtrip --test generated_error_sanitization
  cargo test -p graphql-orm --locked --no-default-features --features sqlite,runtime-graphql \
    --lib --test runtime_graphql --test runtime_strings --test runtime_queries --test runtime_relations \
    --test runtime_mutations --test runtime_migrations --test runtime_schema_ir --test runtime_migration_memory \
    --test migration_planner --test migration_apply --test legacy_migration_history \
    --test runtime_compat_goldens --test runtime_typed_defaults --test runtime_field_formats \
    --test runtime_graphql_formats \
    --test runtime_search_graphql --test runtime_search_writes --test runtime_search_ir \
    -- --test-threads=1
  cargo test -p graphql-orm --no-default-features --features sqlite \
    --test managed_version --test empty_filter_semantics --test keyed_changed_subscriptions --test node_ids --test field_write_denial --locked
  cargo test -p graphql-orm --no-default-features --features sqlite \
    --test grouped_aggregates --test complete_group_pages --test portable_group_pages --test host_timestamps --test implicit_timestamp_defaults --locked
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features sqlite --locked
}

run_postgres() {
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test migration_composite_uniques -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test static_related_filter -- --ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres --test read_predicate -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres,runtime-graphql \
    --lib --test runtime_graphql --test runtime_mutations --test runtime_migrations --test runtime_schema_ir --test runtime_migration_memory \
    --test migration_planner --test migration_apply --test legacy_migration_history \
    --test runtime_compat_goldens --test runtime_typed_defaults --test runtime_field_formats \
    --test runtime_graphql_formats \
    --test runtime_search_graphql --test runtime_search_writes --test runtime_search_ir \
    -- --test-threads=1
  cargo test -p graphql-orm --no-default-features --features postgres \
    --test runtime_strings --test full_text_search --locked -- --include-ignored --test-threads=1
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features postgres --locked --test runtime_strings \
    -- --ignored --test-threads=1
  cargo test -p graphql-orm --no-default-features --features postgres \
    --test grouped_aggregates_postgres --test portable_group_pages --test host_timestamps --test implicit_timestamp_defaults --test point_read_visibility_backends --locked -- --ignored --test-threads=1
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features postgres --locked --test owned_backend --test portable_groups --test host_timestamps \
    -- --ignored --test-threads=1
}

run_postgres_legacy() {
  cargo test -p graphql-orm --locked --no-default-features --features postgres --test dependent_delete_repro --test dependent_delete_pg_physical -- --ignored --test-threads=1

  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test static_related_filter \
    --test app_repository_helpers \
    --test backup_roundtrip \
    --test bidirectional_join_relations \
    --test binary_keys \
    --test composite_mutations \
    --test conditional_indexes \
    --test contract_gaps \
    --test entity_policy \
    --test field_write_denial \
    --test generated_subscriptions \
    --test generated_upserts \
    --test graphql_case_features \
    --test graphql_naming \
    --test json_fields \
    --test lifecycle_cleanup_and_deferred \
    --test lifecycle_hooks \
    --test lifecycle_related_writes \
    --test empty_filter_semantics --test keyed_changed_subscriptions \
    --test managed_version \
    --test mutation_context_reads \
    --test node_ids \
    --test nullable_update_clear \
    --test owned_postgres_lifecycle \
    --test portable_primitives_postgres \
    --test postgres_compat \
    --test postgres_rls \
    --test postgres_rls_drift \
    --test propagated_subscriptions \
    --test read_predicate \
    --test read_projections \
    --test recursive_relations \
    --test relation_delete_policy \
    --test renamed_columns \
    --test row_policy_and_input_transform \
    --test row_write_tracker \
    --test schema_only_entity \
    --test schema_stages \
    --test spatial_fields \
    --test transaction_postgres \
    --test trusted_internal_writes \
    --test uuid_contract \
    -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres,change-journal \
    --test postgres_rls_coverage --test postgres_change_journal_paths \
    --test dependent_delete_repro --test backup_roundtrip \
    -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres,change-journal \
    --lib catalog_read_cancellation -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features sqlite \
    --test federation_end_to_end_fixture -- --ignored node_lookup_through_the_router_on_owned_postgres
}

run_mssql() {
  cargo test -p graphql-orm --locked --no-default-features --features mssql \
    --test migration_composite_uniques -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features mssql --test dependent_delete_ui
  cargo test -p graphql-orm --no-default-features --features mssql \
    --test mssql_writes --test mssql_trigger_writes --test mssql_integration --test managed_version --test empty_filter_semantics --test keyed_changed_subscriptions --test node_ids --test field_write_denial --test implicit_timestamp_defaults --test read_predicate --test point_read_visibility_backends --locked -- --ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features sqlite \
    --test federation_end_to_end_fixture -- --ignored node_lookup_through_the_router_on_owned_mssql
}

run_ai_postgres() {
  cargo test -p graphql-orm-ai --no-default-features \
    --features postgres,provider-openai --test postgres_parity --locked \
    -- --test-threads=1
}

run_field_write_denial() {
  for backend in sqlite postgres mssql; do
    cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
      --test field_write_denial -- --include-ignored --test-threads=1
  done
}

run_denied_relations() {
  for backend in sqlite postgres mssql; do
    cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
      --test denied_relations -- --include-ignored --test-threads=1 --nocapture
  done
}

run_static_related_filter() {
  for backend in postgres mssql; do
    cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
      --test static_related_filter -- --include-ignored --test-threads=1
  done
}

run_read_predicate() {
  cargo test -p graphql-orm --locked --no-default-features --features mssql \
    --test read_predicate -- --include-ignored --test-threads=1
}

case "${lane}" in
  dependent-deletes) run_dependent_deletes ;;
  notify-bulk)
    for backend in sqlite postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" --test notify_bulk -- --include-ignored --test-threads=1
    done
    ;;
  defer-after-commit)
    for backend in sqlite postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" --test after_commit_defer -- --include-ignored --test-threads=1
    done
    ;;
  bulk-refusals)
    for backend in sqlite postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
        --lib application_refusal_roundtrips_but_prefix_text_is_not_trusted
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" --test bulk_refusals --test single_row_refusals --test field_write_denial -- --include-ignored --test-threads=1
    done
    ;;
  field-write-denial) run_field_write_denial ;;
  denied-relations) run_denied_relations ;;
  static-related-filter) run_static_related_filter ;;
  read-predicate) run_read_predicate ;;
  delete-propagation)
    for backend in postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" --test delete_propagation -- --include-ignored --test-threads=1
    done
    ;;
  changed-subscriptions)
    for backend in sqlite postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
        --test keyed_changed_subscriptions -- --include-ignored --test-threads=1
    done
    ;;
  runtime-live)
    cargo test -p graphql-orm --locked --no-default-features --features sqlite,runtime-graphql \
      --lib --test runtime_graphql --test runtime_mutations --test runtime_scalar_readback --test runtime_queries \
      --test runtime_relations --test runtime_live --test runtime_search_graphql --test runtime_search_writes -- --test-threads=1
    cargo test -p graphql-orm --locked --no-default-features --features postgres,runtime-graphql \
      --lib --test runtime_graphql --test runtime_mutations --test runtime_scalar_readback --test runtime_live \
      --test runtime_search_graphql --test runtime_search_writes --test runtime_search_ir -- --test-threads=1
    ;;
  runtime-search-catalog)
    for backend in sqlite postgres; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend},runtime-graphql" \
        --test runtime_search_graphql -- --test-threads=1
    done
    ;;
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
