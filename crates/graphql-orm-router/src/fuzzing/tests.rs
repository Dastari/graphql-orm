//! Ordinary-CI smoke for the fuzz harnesses (#726): the retained corpus and
//! minimized regressions replay deterministically, followed by a short
//! seeded property run of each target.

use std::{path::Path, sync::OnceLock, time::Instant};

use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, RngSeed};

use super::*;

const TARGETS: [&str; 3] = ["bearer_structured", "bearer_raw", "jwks_document"];

fn harness() -> &'static BearerHarness {
    static HARNESS: OnceLock<BearerHarness> = OnceLock::new();
    HARNESS.get_or_init(BearerHarness::new)
}

fn run(target: &str, data: &[u8]) -> bool {
    match target {
        "bearer_structured" => harness().structured(data),
        "bearer_raw" => harness().raw(data),
        "jwks_document" => jwks_document(data),
        other => panic!("unknown fuzz target {other}"),
    }
}

fn entries(directory: &Path) -> Vec<(String, Vec<u8>)> {
    let Ok(read) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries = read
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&path).unwrap())
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

#[test]
fn retained_corpus_and_regressions_replay_without_acceptance_violations() {
    let started = Instant::now();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz");
    let mut accepted = 0;
    let mut violations = Vec::new();
    for target in TARGETS {
        let corpus = entries(&root.join("corpus").join(target));
        assert!(
            !corpus.is_empty(),
            "the retained {target} corpus is missing; this smoke needs the repository checkout"
        );
        for (name, data) in corpus
            .iter()
            .chain(&entries(&root.join("regressions").join(target)))
        {
            let outcome = std::panic::catch_unwind(|| run(target, data));
            match outcome {
                Ok(true) => accepted += 1,
                Ok(false) => {}
                Err(_) => violations.push(format!("{target}/{name}")),
            }
        }
    }
    assert!(
        violations.is_empty(),
        "harness contract violated by {violations:?}"
    );
    // The corpus includes valid bearers, so the comparison path is exercised.
    assert!(
        accepted >= 3,
        "only {accepted} corpus entries were accepted"
    );
    assert!(
        started.elapsed().as_secs() < 60,
        "corpus smoke took {:?}",
        started.elapsed()
    );
}

#[test]
fn the_default_case_and_every_valid_variation_is_accepted() {
    // An all-zero input selects every valid choice: an oracle-valid token
    // the router must accept, so a refusal-only harness cannot pass vacuously.
    assert!(harness().check_bearer(harness().valid_token()));
    assert!(harness().structured(&[]));
    assert!(harness().raw(&[1]));
    // An even mode byte: the rest is the header value.
    let mut header = vec![0];
    header.extend(format!("Bearer {}", harness().valid_token()).bytes());
    assert!(harness().raw(&header));
}

fn config(cases: u32, seed: u64) -> Config {
    let env_u64 = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
    };
    Config {
        cases: env_u64("PROPTEST_CASES").map_or(cases, |value| value as u32),
        rng_seed: RngSeed::Fixed(env_u64("PROPTEST_RNG_SEED").unwrap_or(seed)),
        max_shrink_iters: 512,
        source_file: Some(file!()),
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("regressions"))),
        ..Config::default()
    }
}

proptest! {
    #![proptest_config(config(160, 0x7260_0001))]

    #[test]
    fn structured_bearers_are_never_accepted_without_every_check(data in vec(any::<u8>(), 0..20)) {
        harness().structured(&data);
    }
}

proptest! {
    #![proptest_config(config(512, 0x7260_0002))]

    #[test]
    fn raw_and_edited_bearers_are_never_accepted_without_every_check(data in vec(any::<u8>(), 0..64)) {
        harness().raw(&data);
    }

    #[test]
    fn jwks_documents_never_panic_or_exceed_their_bound(
        keys in vec(
            prop_oneof![
                Just(json!({"kty": "RSA", "kid": "a", "n": "sXch", "e": "AQAB"})),
                Just(json!({"kty": "oct", "kid": "a", "k": "c2VjcmV0"})),
                Just(json!({"kty": "EC", "kid": "a", "crv": "P-256", "x": "AA", "y": "AA"})),
                Just(json!({"kty": "RSA", "use": "enc", "kid": "b"})),
                Just(json!(null)),
                Just(json!("key")),
                any::<String>().prop_map(|kid| json!({"kty": "RSA", "kid": kid})),
            ],
            0..140,
        ),
        wrap in 0u8..3,
    ) {
        let document = match wrap {
            0 => json!({ "keys": keys }),
            1 => json!(keys),
            _ => json!({ "keys": keys, "extra": true }),
        };
        jwks_document(document.to_string().as_bytes());
    }
}

/// Minimized cases retained as explicit regressions, with the outcome the
/// router must produce.
#[test]
fn retained_bearer_regressions() {
    let h = harness();
    let refuse = |data: &[u8]| assert!(!h.structured(data), "{data:?} was accepted");
    // Byte order: alg, kid, extra header, iss, aud, exp, nbf, sub, scope,
    // scopes, roles, duplicate, padding (2), encoding, signing.
    refuse(&[1]); // HS256 header signed with the public key as secret.
    refuse(&[2]); // alg "none".
    refuse(&[3]); // RS384: the RSA family, but not the pinned algorithm.
    refuse(&[0, 0, 0, 0, 0, 4]); // exp exactly at now - leeway.
    assert!(h.structured(&[0, 0, 0, 0, 0, 5])); // exp one second inside the leeway.
    refuse(&[0, 0, 0, 0, 0, 0, 4]); // nbf one second past now + leeway.
    assert!(h.structured(&[0, 0, 0, 0, 0, 0, 3])); // nbf exactly at now + leeway.
    refuse(&[0, 0, 0, 0, 0, 8]); // exp = u64::MAX is not a representable time.
    // Found by this harness: critical header extensions were ignored.
    refuse(&[0, 0, 3]); // "crit": ["exp"]
    refuse(&[0, 0, 4]); // "crit": []
    refuse(&[0, 0, 5]); // "b64": false with "crit": ["b64"]
    refuse(&[0, 0, 10]); // "crit": null, which the JWT library reads as absent
    // Found by this harness: an issuer array containing the issuer passed.
    refuse(&[0, 0, 0, 6]); // "iss": [issuer]
    refuse(&[0, 0, 0, 9]); // "iss": [issuer, other]
    refuse(&[0, 0, 0, 7]); // "iss": []
}
