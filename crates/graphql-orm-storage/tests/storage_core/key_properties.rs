//! Bounded property suite for blob key validation (#725).
//!
//! Each validator is compared with an independent model of its documented
//! contract, so a key is accepted exactly when the documentation says it is:
//!
//! - [`validate_blob_key`]: a non-empty relative path of normal components,
//!   no `\` or NUL, and no first component naming the reserved
//!   `.graphql-orm-storage` namespace in any ASCII case with or without
//!   trailing dots or spaces;
//! - [`validate_portable_blob_key`]: also no Windows-reserved characters,
//!   controls, trailing dot or space, or device names;
//! - [`validate_canonical_blob_key`]: also lowercase ASCII only.
//!
//! An accepted key joined to a root always stays below that root, and on the
//! local provider it round-trips without creating anything outside its root.
//! Cases run from a fixed seed; set `PROPTEST_CASES` or `PROPTEST_RNG_SEED` to
//! explore further.

use std::path::{Component, Path};

use graphql_orm_storage::{
    StorageError, validate_blob_key, validate_canonical_blob_key, validate_portable_blob_key,
};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, RngSeed};

const INTERNAL_NAMESPACE: &str = ".graphql-orm-storage";

fn config(cases: u32, seed: u64) -> Config {
    let env_u64 = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
    };
    Config {
        cases: env_u64("PROPTEST_CASES").map_or(cases, |value| value as u32),
        rng_seed: RngSeed::Fixed(env_u64("PROPTEST_RNG_SEED").unwrap_or(seed)),
        max_shrink_iters: 2048,
        source_file: Some(file!()),
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("regressions"))),
        ..Config::default()
    }
}

fn key_char() -> impl Strategy<Value = char> {
    prop_oneof![
        8 => proptest::char::range('a', 'z'),
        2 => proptest::char::range('A', 'Z'),
        2 => proptest::char::range('0', '9'),
        6 => proptest::sample::select(vec![
            '/', '/', '.', '.', '\\', '\0', ' ', '-', '_', ':', '<', '>', '"', '|', '?', '*',
            '\u{1}', '\u{1f}', '\u{7f}', '$', '~', '%',
        ]),
        2 => proptest::sample::select(vec![
            'é', '\u{301}', 'ſ', 'K', '\u{212a}', 'İ', 'ı', '¹', '²', '³', '\u{ff0e}', '\u{ff0f}',
            '\u{2215}', '\u{2024}', '\u{a0}', '\u{200b}', '\u{feff}', '雪', '🦀',
        ]),
        1 => any::<char>(),
    ]
}

/// Components that sit on the validators' edges.
fn edge_component() -> impl Strategy<Value = String> {
    let namespace_spelling =
        (any::<u32>(), 0usize..3, 0usize..3).prop_map(|(case, dots, spaces)| {
            let mut name: String = INTERNAL_NAMESPACE
                .chars()
                .enumerate()
                .map(|(index, c)| {
                    if case >> (index % 32) & 1 == 1 {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    }
                })
                .collect();
            name.push_str(&".".repeat(dots));
            name.push_str(&" ".repeat(spaces));
            name
        });
    let device = (
        proptest::sample::select(vec![
            "CON", "prn", "Aux", "nul", "CONIN$", "conout$", "COM", "lpt",
        ]),
        proptest::sample::select(vec!["", "0", "9", "¹", "²", "³", "10", "a"]),
        proptest::sample::select(vec!["", ".txt", " .txt", " ", ".", ".tar.gz"]),
    )
        .prop_map(|(stem, digit, tail)| format!("{stem}{digit}{tail}"));
    prop_oneof![
        Just(String::new()),
        Just(".".to_owned()),
        Just("..".to_owned()),
        Just("...".to_owned()),
        namespace_spelling,
        device,
    ]
}

fn component() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => vec(key_char(), 1..10).prop_map(|chars| chars.into_iter().collect()),
        2 => edge_component(),
    ]
}

fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => vec(component(), 1..5).prop_map(|parts| parts.join("/")),
        1 => (vec(component(), 1..4), any::<bool>(), any::<bool>()).prop_map(|(parts, lead, trail)| {
            format!(
                "{}{}{}",
                if lead { "/" } else { "" },
                parts.join("/"),
                if trail { "/" } else { "" }
            )
        }),
        1 => vec(key_char(), 0..24).prop_map(|chars| chars.into_iter().collect()),
    ]
}

// ----- the documented contract, written independently ------------------------

fn names_internal_namespace(component: &str) -> bool {
    component
        .trim_end_matches(['.', ' '])
        .eq_ignore_ascii_case(INTERNAL_NAMESPACE)
}

fn model_blob(key: &str) -> bool {
    let components: Vec<&str> = key.split('/').collect();
    !key.is_empty()
        && !key.contains(['\\', '\0'])
        && components
            .iter()
            .all(|component| !matches!(*component, "" | "." | ".."))
        && !names_internal_namespace(components[0])
}

fn is_device(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&upper.as_str()) {
        return true;
    }
    let mut characters = stem.chars();
    let prefix: String = characters.by_ref().take(3).collect();
    let rest: Vec<char> = characters.collect();
    matches!(prefix.to_ascii_uppercase().as_str(), "COM" | "LPT")
        && matches!(rest.as_slice(), [c] if c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
}

fn model_portable(key: &str) -> bool {
    model_blob(key)
        && key.split('/').all(|component| {
            !component.contains([':', '<', '>', '"', '|', '?', '*'])
                && !component.chars().any(|c| ('\u{1}'..='\u{1f}').contains(&c))
                && !component.ends_with(['.', ' '])
                && !is_device(component)
        })
}

fn model_canonical(key: &str) -> bool {
    model_portable(key)
        && key
            .bytes()
            .all(|byte| byte.is_ascii() && !byte.is_ascii_uppercase())
}

fn refused_as_invalid_key(result: Result<(), StorageError>, key: &str) -> bool {
    matches!(result, Err(StorageError::InvalidStorageKey { key: reported }) if reported == key)
}

proptest! {
    #![proptest_config(config(4096, 0x7250_0301))]

    #[test]
    fn validators_accept_exactly_their_documented_keys(key in key()) {
        for (name, accepted, expected) in [
            ("blob", validate_blob_key(&key), model_blob(&key)),
            ("portable", validate_portable_blob_key(&key), model_portable(&key)),
            ("canonical", validate_canonical_blob_key(&key), model_canonical(&key)),
        ] {
            if expected {
                prop_assert!(accepted.is_ok(), "{} refused {:?}: {:?}", name, key, accepted);
            } else {
                prop_assert!(refused_as_invalid_key(accepted, &key), "{} accepted {:?}", name, key);
            }
        }
    }

    #[test]
    fn accepted_keys_stay_below_their_root(key in key()) {
        if validate_blob_key(&key).is_ok() {
            let path = Path::new(&key);
            prop_assert!(path.is_relative());
            // Every component is a plain name: none can climb, reset to a
            // root or name a prefix, and none is dropped by normalisation.
            prop_assert!(path.components().all(|c| matches!(c, Component::Normal(_))));
            prop_assert_eq!(path.components().count(), key.split('/').count());
            let root = Path::new("/srv/store");
            prop_assert!(root.join(path).starts_with(root));
            // The first component never names the internal namespace.
            let first = key.split('/').next().unwrap();
            prop_assert!(!names_internal_namespace(first));
        }
    }

    #[test]
    fn reserved_name_further_down_a_key_is_ordinary(
        prefix in vec(component(), 1..3),
        spelling in edge_component(),
    ) {
        let key = format!("{}/{}/{spelling}", prefix.join("/"), "data");
        if names_internal_namespace(&spelling) && model_blob(&prefix.join("/")) {
            prop_assert!(validate_blob_key(&key).is_ok(), "{:?}", key);
        }
    }
}

/// Minimized and boundary cases retained as explicit regressions.
#[test]
fn retained_key_cases() {
    for (key, blob, portable, canonical) in [
        ("a", true, true, true),
        (".graphql-orm-storage", false, false, false),
        (".GRAPHQL-ORM-STORAGE. .", false, false, false),
        (".graphql-orm-storage/x", false, false, false),
        ("x/.graphql-orm-storage", true, true, true),
        // A Unicode lookalike is a different name lexically; local storage
        // reserves the namespace by file identity, and canonical keys refuse it.
        (".graphql-orm-ſtorage/x", true, true, false),
        ("a//b", false, false, false),
        ("/a", false, false, false),
        ("a/", false, false, false),
        ("a/./b", false, false, false),
        ("a/../b", false, false, false),
        ("a\\b", false, false, false),
        ("a\0b", false, false, false),
        ("...", true, false, false),
        ("con.txt", true, false, false),
        ("COM¹", true, false, false),
        ("com10", true, true, true),
        ("report.", true, false, false),
        ("a:b", true, false, false),
        ("Report", true, true, false),
        ("雪", true, true, false),
    ] {
        assert_eq!(validate_blob_key(key).is_ok(), blob, "blob {key:?}");
        assert_eq!(
            validate_portable_blob_key(key).is_ok(),
            portable,
            "portable {key:?}"
        );
        assert_eq!(
            validate_canonical_blob_key(key).is_ok(),
            canonical,
            "canonical {key:?}"
        );
    }
}

#[cfg(all(feature = "local", not(windows)))]
mod local_provider {
    use super::*;
    use bytes::Bytes;
    use graphql_orm_storage::{
        BlobPutOptions, BlobStore, LocalStorageBackend, StorageByteStream, collect_storage_stream,
    };
    use proptest::test_runner::{TestCaseError, TestRunner};

    /// Every path below `base`, relative to it.
    fn tree(base: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut pending = vec![base.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                found.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
                if entry.file_type().unwrap().is_dir() {
                    pending.push(path);
                }
            }
        }
        found.sort();
        found
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_keys_round_trip_and_never_leave_their_root() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let backend = LocalStorageBackend::new(&root);
        let handle = tokio::runtime::Handle::current();
        let case = |key: String| -> Result<(), TestCaseError> {
            tokio::task::block_in_place(|| {
                handle.block_on(async {
                    let body = Bytes::from(format!("body:{key}"));
                    let put = backend
                        .put_blob(
                            &key,
                            StorageByteStream::from_bytes(body.clone()),
                            BlobPutOptions::default(),
                        )
                        .await;
                    // Nothing is ever created beside the root.
                    let outside = std::fs::read_dir(base.path()).unwrap().count();
                    prop_assert_eq!(outside, 1, "{:?} wrote beside the root", key);
                    if validate_blob_key(&key).is_err() {
                        prop_assert!(
                            matches!(put, Err(StorageError::InvalidStorageKey { .. })),
                            "{:?} was not refused: {:?}",
                            key,
                            put.map(|_| ())
                        );
                        return Ok(());
                    }
                    match put {
                        Ok(_) => {}
                        // A key whose file would sit where a directory of an
                        // earlier key is, or below an earlier file, is a
                        // provider conflict rather than a validation result.
                        Err(StorageError::InvalidStorageKey { .. }) => {
                            return Err(TestCaseError::fail(format!("{key:?} refused locally")));
                        }
                        Err(_) => return Ok(()),
                    }
                    let read = backend
                        .get_blob(&key)
                        .await
                        .map_err(|e| TestCaseError::fail(e.to_string()))?;
                    let bytes = collect_storage_stream(read.body)
                        .await
                        .map_err(|e| TestCaseError::fail(e.to_string()))?;
                    prop_assert_eq!(bytes, body);
                    prop_assert!(root.join(&key).is_file(), "{:?} is not at its path", key);
                    let listed = backend
                        .list_blobs("")
                        .await
                        .map_err(|e| TestCaseError::fail(e.to_string()))?;
                    prop_assert!(listed.contains(&key), "{:?} missing from {:?}", key, listed);
                    backend
                        .delete_blob(&key)
                        .await
                        .map_err(|e| TestCaseError::fail(e.to_string()))?;
                    Ok(())
                })
            })
        };
        TestRunner::new(config(512, 0x7250_0302))
            .run(&key(), case)
            .unwrap();
        // Everything below the root is the provider's own namespace or
        // directories left by deleted keys.
        for path in tree(&root) {
            let first = path.split('/').next().unwrap();
            assert!(
                first == INTERNAL_NAMESPACE
                    || std::fs::metadata(root.join(&path)).unwrap().is_dir(),
                "{path:?} left below the root"
            );
        }
    }
}
