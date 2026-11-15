//! Every character that is special in a `LIKE` pattern on some backend, as a
//! literal search text. A `contains` pattern built from a needle must match
//! exactly the values that contain it, compared case-insensitively.

/// Search texts: each wildcard, class delimiter and the escape character on
/// its own, and in the combinations a backend would read as a pattern.
pub(crate) const NEEDLES: &[&str] = &[
    "[",
    "]",
    "%",
    "_",
    "^",
    "\\",
    "[a-c]",
    "[^a]",
    "[]",
    "a_c",
    "100%",
    "%_",
    "\\%",
    "\\\\",
    "[[]",
    "50%_[x]\\",
];

/// Values searched: every needle embedded in text, and decoys that only a
/// pattern reading of a needle would match.
pub(crate) fn haystacks() -> Vec<String> {
    let mut values = NEEDLES
        .iter()
        .map(|needle| format!("P{needle}s"))
        .collect::<Vec<_>>();
    values.extend(
        [
            "a", "b", "c", "x", "pas", "pbs", "pxs", "ps", "abc", "a.c", "100", "1000", "p^s",
            "p s", "50", "anything",
        ]
        .map(str::to_owned),
    );
    values
}

/// The values a `contains` search for `needle` must return, in order.
pub(crate) fn expected(needle: &str) -> Vec<String> {
    let needle = needle.to_lowercase();
    haystacks()
        .into_iter()
        .filter(|value| value.to_lowercase().contains(&needle))
        .collect()
}
