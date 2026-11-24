#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  echo "usage: scripts/run-owned-database-lanes.sh [unique-index-preflight|sqlite|postgres|postgres-table-case|backup-postgres|postgres-legacy|migration-journal|mssql|runtime-live|runtime-search-catalog|ai-postgres|ai-mssql|field-write-denial|bulk-refusals|notify-bulk|denied-relations|static-related-filter|read-predicate|defer-after-commit|delete-propagation|dependent-deletes|bounded-set-delete|changed-subscriptions|bulk-write-targets|query-loader-batch|page-limit-contract|authorized-page|callback-errors|all]" >&2
  exit 2
fi

lane=${1:-all}
case "${lane}" in
  unique-index-preflight|sqlite|postgres|postgres-table-case|backup-postgres|postgres-legacy|migration-journal|mssql|runtime-live|runtime-search-catalog|ai-postgres|ai-mssql|field-write-denial|bulk-refusals|notify-bulk|denied-relations|static-related-filter|read-predicate|defer-after-commit|delete-propagation|dependent-deletes|bounded-set-delete|changed-subscriptions|bulk-write-targets|query-loader-batch|page-limit-contract|authorized-page|callback-errors|all) ;;
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

run_backup_postgres() {
  cargo test -p graphql-orm-backup --locked --no-default-features --features local,orm-postgres,graphql-orm/change-journal \
    --test orm_016_postgres_conformance -- --include-ignored --test-threads=1
}

run_dependent_deletes() {
  cargo test -p graphql-orm --locked --no-default-features --features sqlite --test dependent_delete_repro
  cargo test -p graphql-orm --locked --no-default-features --features postgres --test dependent_delete_repro --test dependent_delete_pg_physical -- --ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres --test compile_ui dependent_delete_ui::declared_parent_opt_in_is_enforced_across_crates
  cargo check -p graphql-orm --locked --no-default-features --features mssql --all-targets
  cargo test -p graphql-orm --locked --no-default-features --features mssql --test compile_ui dependent_delete_ui::
}

run_bounded_set_delete() {
  cargo test -p graphql-orm --locked --no-default-features --features "$1" \
    --test bounded_set_delete -- --test-threads=1
}

run_identifier_quoting() {
  cargo test -p graphql-orm --locked --no-default-features --features "$1" \
    --test identifier_quoting_properties --test qualified_read_predicates --test read_predicate_row_binding --test read_predicate_row_scopes \
    -- --test-threads=1
}

run_sqlite() {
  run_bounded_set_delete sqlite
  cargo test -p graphql-orm --locked --no-default-features --features sqlite \
    --test read_snapshot_transaction --test computed_field_invalidation -- --include-ignored --test-threads=1
  run_identifier_quoting sqlite
  cargo test -p graphql-orm --locked --no-default-features --features sqlite --test dependent_delete_repro
  cargo test -p graphql-orm --locked --no-default-features --features sqlite --test compile_ui dependent_delete_ui::
  cargo test -p graphql-orm --locked --no-default-features --features sqlite,change-journal \
    --test dependent_delete_repro --test backup_roundtrip --test generated_error_sanitization
  cargo test -p graphql-orm --locked --no-default-features --features sqlite,runtime-graphql \
    --lib --test runtime_graphql --test runtime_strings --test sqlite_runtime_queries \
    --test runtime_mutations --test runtime_migrations --test runtime_schema_ir --test runtime_migration_memory \
    --test migration_planner --test migration_apply --test migration_safety --test legacy_migration_history \
    --test runtime_compat_goldens --test runtime_typed_defaults --test runtime_field_formats \
    --test runtime_graphql_formats \
    --test runtime_search_graphql --test runtime_search_writes --test runtime_search_ir --test runtime_search_long_tables \
    --test runtime_many_to_many --test runtime_relation_bind_limit \
    -- --test-threads=1
  cargo test -p graphql-orm --no-default-features --features sqlite \
    --test managed_version --test update_assurance --test update_assurance_registry --test empty_filter_semantics --test keyed_changed_subscriptions --test node_ids --test field_write_denial --test page_limit_contract --test repository_explicit_limits \
    --test bulk_write_filter_authority --test static_input_object_shape --test client_error_contract --test client_facing_metadata --locked
  cargo test -p graphql-orm --no-default-features --features sqlite \
    --test read_predicate_closure --test read_predicate_closure_parity --test read_predicate_closure_delivery --test read_predicate_budget_shapes --test read_predicate_statement_budget --locked
  cargo test -p graphql-orm --no-default-features --features sqlite \
    --test temporal_bind_compatibility --test grouped_aggregates --test complete_group_pages --test portable_group_pages --test host_timestamps --test implicit_timestamp_defaults --locked
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features sqlite --locked
}

run_postgres() {
  run_bounded_set_delete postgres
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test search_filter_binding -- --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test read_snapshot_transaction --test computed_field_invalidation -- --include-ignored --test-threads=1
  run_identifier_quoting postgres
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test postgres_owned_contracts --test bounded_mutation_sentinels \
    --test bulk_refusals --test delete_propagation --test denied_relations \
    --test host_primary_keys --test postgres_runtime_repository \
    --test single_row_refusals \
    -- --ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test temporal_bind_compatibility --test postgres_table_case --test migration_safety --test migration_composite_uniques --test postgres_qualified_tables --test retention_purge_postgres \
    -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres,runtime-graphql \
    --test runtime_many_to_many -- --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test static_related_filter -- --ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres --test read_predicate --test read_predicate_closure --test read_predicate_closure_parity --test read_predicate_budget_shapes --test visible_row_pages --test like_escape_matrix --test loader_chunks_postgres --test query_loader_batch --test order_aggregate_visibility --test authorized_page_context_binding --test page_limit_contract --test bulk_write_filter_authority --test client_error_contract --test client_facing_metadata -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres,runtime-graphql \
    --lib --test runtime_graphql --test runtime_mutations --test runtime_migrations --test runtime_schema_ir --test runtime_migration_memory \
    --test migration_planner --test migration_apply --test migration_safety --test legacy_migration_history \
    --test runtime_compat_goldens --test runtime_typed_defaults --test runtime_field_formats \
    --test runtime_graphql_formats \
    --test runtime_search_graphql --test runtime_search_writes --test runtime_search_ir --test runtime_search_long_tables \
    -- --test-threads=1
  cargo test -p graphql-orm --no-default-features --features postgres \
    --test runtime_strings --test full_text_search --test search_helper_conflicts --locked -- --include-ignored --test-threads=1
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features postgres --locked --test runtime_strings \
    -- --ignored --test-threads=1
  cargo test -p graphql-orm --no-default-features --features postgres \
    --test grouped_aggregates_postgres --test portable_group_pages --test host_timestamps --test implicit_timestamp_defaults --test point_read_visibility_backends --locked -- --ignored --test-threads=1
  cargo test --manifest-path \
    crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
    --no-default-features --features postgres --locked --test owned_backend --test portable_groups --test host_timestamps --test host_integer_key \
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
    --test update_assurance --test update_assurance_registry \
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
    --test read_snapshot_transaction --test postgres_rls_coverage --test postgres_change_journal_paths --test postgres_change_journal_truncate \
    --test dependent_delete_repro --test backup_roundtrip \
    -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres,change-journal \
    --lib catalog_read_cancellation -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features sqlite \
    --test federation_end_to_end_fixture -- --ignored node_lookup_through_the_router_on_owned_postgres
}

# Focused recovery lane for the journal portion of postgres-legacy.
run_migration_journal() {
  cargo test -p graphql-orm --locked --no-default-features --features postgres,change-journal \
    --test dependent_delete_repro dependent_deletes_are_journaled -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres,change-journal \
    --test postgres_rls_coverage --test postgres_change_journal_paths --test postgres_change_journal_truncate \
    --test backup_roundtrip -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features postgres,change-journal \
    --lib catalog_read_cancellation -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features sqlite \
    --test federation_end_to_end_fixture -- --ignored node_lookup_through_the_router_on_owned_postgres
}

run_mssql() {
  run_bounded_set_delete mssql
  cargo test -p graphql-orm --locked --no-default-features --features mssql \
    --test search_filter_binding -- --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features mssql \
    --test read_snapshot_transaction -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features mssql \
    --lib alter_column_looks_up_the_exact_catalogue_identifier
  cargo test -p graphql-orm --locked --no-default-features --features mssql \
    --lib snapshot_start_preserves_3952_from_restore_gate -- --ignored --test-threads=1
  run_identifier_quoting mssql
  cargo test -p graphql-orm --locked --no-default-features --features mssql \
    --test migration_composite_uniques --test migration_safety -- --include-ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features mssql --test compile_ui dependent_delete_ui::
  cargo test -p graphql-orm --locked --no-default-features --features mssql \
    --lib delivery_is_at_least_once_through_failures_and_cancellation -- --ignored --test-threads=1
  cargo test -p graphql-orm --locked --no-default-features --features mssql \
    --lib the_probe_reads_the_rows_the_filtered_index_covers -- --ignored --test-threads=1
  cargo test -p graphql-orm --no-default-features --features mssql \
    --test mssql_owned_writes --test mssql_restore_marker --test mssql_owned_queries --test computed_field_invalidation --test managed_version --test update_assurance --test update_assurance_registry --test empty_filter_semantics --test keyed_changed_subscriptions --test node_ids --test field_write_denial --test implicit_timestamp_defaults --test read_predicate --test read_predicate_closure --test read_predicate_closure_parity --test read_predicate_closure_delivery --test read_predicate_budget_shapes --test point_read_visibility_backends --test visible_row_pages --test query_loader_batch --test order_aggregate_visibility --test page_limit_contract --locked -- --ignored --test-threads=1
  # Closure rendering and depth refusals need no server.
  cargo test -p graphql-orm --locked --no-default-features --features mssql --test read_predicate_closure
  cargo test -p graphql-orm --locked --no-default-features --features sqlite \
    --test federation_end_to_end_fixture -- --ignored node_lookup_through_the_router_on_owned_mssql
}

run_ai_postgres() {
  # The AI effect guard is enforced by the ORM's transaction-bound planner.
  cargo test -p graphql-orm --locked --no-default-features --features postgres \
    --test dependent_delete_repro transaction_effect_guard -- --ignored --test-threads=1
  run_ai_parity postgres
}

run_ai_parity() {
  local backend=$1
  cargo test -p graphql-orm-ai --locked --no-default-features \
    --features "${backend},provider-openai" --test durable_parity -- --test-threads=1
  cargo test -p graphql-orm-ai --locked --no-default-features \
    --features "${backend},provider-openai" --lib database_parity_ -- --ignored --test-threads=1
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
    --test read_predicate --test read_predicate_closure --test read_predicate_closure_parity \
    --test read_predicate_closure_delivery --test read_predicate_budget_shapes -- --include-ignored --test-threads=1
}

case "${lane}" in
  unique-index-preflight)
    for backend in sqlite postgres; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
        --test conditional_indexes -- --include-ignored --test-threads=1
    done
    ;;
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
  bulk-write-targets)
    for backend in sqlite postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
        --test bulk_write_targets --test mssql_bulk_write_key_paths --test transaction_scope_backends --test ordered_sort -- --include-ignored --test-threads=1
    done
    ;;
  query-loader-batch)
    for backend in sqlite postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
        --test query_loader_batch --test order_aggregate_visibility -- --include-ignored --test-threads=1
    done
    ;;
  authorized-page)
    for backend in sqlite postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
        --test authorized_page_context_binding -- --include-ignored --test-threads=1
    done
    ;;
  callback-errors)
    cargo test -p graphql-orm --locked --no-default-features --features sqlite \
      --test mutation_hook_public_errors --test sqlite_compat --test keyset_pagination
    cargo test -p graphql-orm --locked --no-default-features --features postgres \
      --test mutation_hook_public_errors --test postgres_compat --test keyset_pagination \
      -- --include-ignored --test-threads=1
    ;;
  page-limit-contract)
    for backend in sqlite postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
        --test page_limit_contract -- --include-ignored --test-threads=1
    done
    ;;
  changed-subscriptions)
    for backend in sqlite postgres mssql; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend}" \
        --test keyed_changed_subscriptions --test computed_field_invalidation -- --include-ignored --test-threads=1
    done
    ;;
  runtime-live)
    cargo test -p graphql-orm --locked --no-default-features --features sqlite,runtime-graphql \
      --lib --test runtime_graphql --test runtime_mutations --test runtime_scalar_readback --test sqlite_runtime_queries \
      --test runtime_live --test runtime_search_graphql --test runtime_search_writes \
      --test runtime_operation_assurance --test runtime_graphql_formats -- --test-threads=1
    cargo test -p graphql-orm --locked --no-default-features --features postgres,runtime-graphql \
      --lib --test runtime_graphql --test runtime_mutations --test runtime_scalar_readback --test runtime_live \
      --test runtime_search_graphql --test runtime_search_writes --test runtime_search_ir \
      --test runtime_many_to_many --test runtime_search_long_tables -- --test-threads=1
    ;;
  postgres-table-case)
    cargo test -p graphql-orm --locked --no-default-features --features postgres --lib \
      static_registry_ownership_contains_only_physical_identifiers -- --test-threads=1
    cargo test -p graphql-orm --locked --no-default-features --features postgres \
      --test postgres_table_case --test search_helper_conflicts -- --include-ignored --test-threads=1
    run_backup_postgres
    ;;
  backup-postgres) run_backup_postgres ;;
  runtime-search-catalog)
    for backend in sqlite postgres; do
      cargo test -p graphql-orm --locked --no-default-features --features "${backend},runtime-graphql" \
        --test runtime_search_graphql -- --test-threads=1
    done
    ;;
  bounded-set-delete)
    for backend in sqlite postgres mssql; do
      run_bounded_set_delete "$backend"
    done
    ;;
  sqlite) run_sqlite ;;
  postgres) run_postgres ;;
  postgres-legacy) run_postgres_legacy ;;
  migration-journal) run_migration_journal ;;
  mssql) run_mssql ;;
  ai-postgres) run_ai_postgres ;;
  ai-mssql) run_ai_parity mssql ;;
  all)
    run_sqlite
    run_postgres
    run_postgres_legacy
    run_mssql
    run_ai_postgres
    run_ai_parity mssql
    ;;
esac

echo "owned-database-lanes: ${lane} passed without an ambient database URL"
