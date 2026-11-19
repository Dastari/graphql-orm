//! JWKS documents through the refresh path's key selection.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    graphql_orm_router::fuzzing::jwks_document(data);
});
