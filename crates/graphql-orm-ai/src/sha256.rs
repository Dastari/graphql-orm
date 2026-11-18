//! Validation for stored SHA-256 digest text.

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(all(
    any(feature = "sqlite", feature = "postgres"),
    feature = "provider-openai-core"
))]
pub(crate) fn valid_sha256_any_case(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
