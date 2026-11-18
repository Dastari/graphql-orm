//! Structured bearer tokens: header, claim, encoding and signing choices.
#![no_main]

use std::sync::OnceLock;

use graphql_orm_router::fuzzing::BearerHarness;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    static HARNESS: OnceLock<BearerHarness> = OnceLock::new();
    HARNESS.get_or_init(BearerHarness::new).structured(data);
});
