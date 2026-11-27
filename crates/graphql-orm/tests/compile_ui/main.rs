//! Compile-fail and compile-pass contracts (trybuild), in one test binary.
//!
//! Each module was a separate integration-test binary before #508; they share
//! `support/ui.rs` and its OS lock, so they ran one at a time anyway. Filter a
//! module with `cargo test -p graphql-orm --test compile_ui <module>::`.
#[path = "../support/ui.rs"]
mod ui;

mod append_only_ui;
mod backend_selection_ui;
mod composite_relations_ui;
mod composite_write_unavailable_ui;
mod conditional_index_ui;
mod contract_removals_ui;
mod default_sort_ui;
mod dependent_delete_ui;
mod empty_order_ui;
mod federation_entity_keys_ui;
mod final386_ui;
mod generated_mutation_exposure_postgres_ui;
mod generated_mutation_exposure_ui;
mod generated_root_opt_out_ui;
mod host_timestamp_ui;
mod mssql_write_unavailable_ui;
mod mutation_shapes_ui;
mod nested_create_ui;
mod object_fields_ui;
mod operation_authorization_ui;
mod order_aggregate_ui;
mod ordinary_index_ui;
mod policy_decision_ui;
mod principal_upserts_ui;
mod propagate_change_action_ui;
mod query_hygiene_ui;
mod read_projection_ui;
mod recheck386_ui;
mod record_version_ui;
mod relation_delete_policy_ui;
mod relations_complex_ui;
mod repository_only_entity_ui;
mod retention_boundary_ui;
mod review386_ui;
mod rls_ui;
mod root_field_collisions_ui;
mod schema_only_entity_ui;
mod search_json_ui;
mod search_relation_ui;
mod semantic_catalogue_ui;
mod server_only_keys_ui;
mod simple_object_hidden_ui;
mod skip_false_compat_ui;
mod spatial_field_ui;
mod stable_type_id_ui;
mod transform_write_ui;
mod update_assurance_ui;
mod upsert_validation_ui;
mod write_pipeline_ui;
mod zero_public_write_inputs_ui;
