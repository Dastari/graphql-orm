#![cfg(feature = "sqlite")]
use crate::ui;

#[test]
fn server_only_relation_keys_are_narrow_and_explicit() {
    let cases = ui::test_cases();
    cases.pass("tests/ui/server_only_keys/pass.rs");
    cases.compile_fail("tests/ui/server_only_keys/duplicate.rs");
    cases.compile_fail("tests/ui/server_only_keys/duplicate_column.rs");
    cases.compile_fail("tests/ui/server_only_keys/every_key_server_only.rs");
    cases.compile_fail("tests/ui/server_only_keys/filterable_target.rs");
    cases.compile_fail("tests/ui/server_only_keys/float_key.rs");
    cases.compile_fail("tests/ui/server_only_keys/hand_written_target.rs");
    cases.compile_fail("tests/ui/server_only_keys/implicit_id.rs");
    cases.compile_fail("tests/ui/server_only_keys/missing_to.rs");
    cases.compile_fail("tests/ui/server_only_keys/order_aggregate.rs");
    cases.compile_fail("tests/ui/server_only_keys/propagate_change.rs");
    cases.compile_fail("tests/ui/server_only_keys/read_false_target.rs");
    cases.compile_fail("tests/ui/server_only_keys/readable_target.rs");
    cases.compile_fail("tests/ui/server_only_keys/related_filter.rs");
    cases.compile_fail("tests/ui/server_only_keys/schema_entity_without_skip.rs");
    cases.compile_fail("tests/ui/server_only_keys/search_relation.rs");
    cases.compile_fail("tests/ui/server_only_keys/self_target_readable.rs");
    cases.compile_fail("tests/ui/server_only_keys/skip_db_source.rs");
    cases.compile_fail("tests/ui/server_only_keys/transform_read_source.rs");
    cases.compile_fail("tests/ui/server_only_keys/unknown_pair.rs");
}
