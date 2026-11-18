//! Deterministic harnesses at the router's bearer-authentication boundary
//! (#726), shared by the cargo-fuzz targets in `fuzz/` and the corpus smoke
//! tests that run in ordinary CI.
//!
//! The module is compiled only for this crate's tests and for builds with
//! `--cfg fuzzing`; it is not part of the public API. Every key and token is
//! synthetic, the clock is fixed, and no harness touches the network: the
//! verification key set is installed directly, and an unknown key ID never
//! fetches because no runtime is running.
//!
//! Each harness panics when the router accepts a bearer that the
//! independent [`Oracle`] refuses, or returns a principal other than the one
//! the token's claims name. A refusal is never a failure here: the harness
//! checks that nothing is accepted without the normal checks, not that every
//! valid token is accepted. The oracle re-implements the claim rules from
//! RFC 7519 and the router's documented policy, and checks signatures with
//! `jsonwebtoken::crypto::verify` directly rather than through `decode`.
//!
//! Parsing that belongs to the external `agql-auth` crate (its access-token
//! validator and signed role catalogue) is outside these targets.

use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey,
    jwk::{Jwk, JwkSet, PublicKeyUse},
};
use serde_json::{Value as JsonValue, json};

use crate::{
    AuthenticatedPrincipal, AuthenticationClock, AuthenticationProvider, JwksAuthenticationConfig,
    JwksAuthenticationProvider,
};

/// The issuer the harness provider trusts.
pub const ISSUER: &str = "https://issuer.example.test";
/// The audience the harness provider accepts.
pub const AUDIENCE: &str = "graphql-router";
/// Another resource's audience, never accepted.
pub const OTHER_AUDIENCE: &str = "another-resource";
/// The trusted key's ID.
pub const KID: &str = "fuzz-key-1";
/// The fixed authentication time, in Unix seconds.
pub const NOW: u64 = 1_800_000_000;
/// The configured clock leeway, in seconds.
pub const LEEWAY: u64 = 30;

#[derive(Debug)]
struct FixedClock {
    wall: SystemTime,
    monotonic: Instant,
}

impl AuthenticationClock for FixedClock {
    fn now(&self) -> SystemTime {
        self.wall
    }

    fn monotonic_now(&self) -> Instant {
        self.monotonic
    }
}

/// A provider with one trusted RS256 key and a fixed clock, plus the
/// synthetic material that builds hostile tokens for it.
pub struct BearerHarness {
    provider: JwksAuthenticationProvider,
    trusted: EncodingKey,
    untrusted: EncodingKey,
    /// The trusted key's public PEM used as an HMAC secret: the classic
    /// algorithm-confusion forgery.
    public_key_as_secret: EncodingKey,
    verifying: DecodingKey,
    valid_token: String,
}

impl Default for BearerHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl BearerHarness {
    /// Builds the harness. Panics only on a defect in its own fixed material.
    pub fn new() -> Self {
        let trusted = EncodingKey::from_rsa_pem(SYNTHETIC_RSA_PRIVATE_KEY.as_bytes())
            .expect("synthetic trusted key");
        let untrusted = EncodingKey::from_rsa_pem(UNTRUSTED_RSA_PRIVATE_KEY.as_bytes())
            .expect("synthetic untrusted key");
        let mut jwk = Jwk::from_encoding_key(&trusted, Algorithm::RS256).expect("public JWK");
        jwk.common.key_id = Some(KID.to_owned());
        jwk.common.public_key_use = Some(PublicKeyUse::Signature);
        let verifying = DecodingKey::from_jwk(&jwk).expect("decoding key");
        let public_pem = public_key_pem(&jwk);
        let jwks = serde_json::to_string(&JwkSet { keys: vec![jwk] }).expect("JWKS");
        let config = JwksAuthenticationConfig::new(
            "https://issuer.example.test/.well-known/jwks.json",
            ISSUER,
            [AUDIENCE],
        )
        .expect("config")
        .with_cache_ttl(Duration::from_secs(24 * 60 * 60))
        .with_refresh_interval(Duration::from_secs(60))
        .with_leeway(Duration::from_secs(LEEWAY))
        .with_clock(Arc::new(FixedClock {
            wall: UNIX_EPOCH + Duration::from_secs(NOW),
            monotonic: Instant::now(),
        }));
        let provider = JwksAuthenticationProvider::new(config).expect("provider");
        provider
            .install_key_set_for_harness(&jwks)
            .expect("install key set");
        let mut harness = Self {
            provider,
            trusted,
            untrusted,
            public_key_as_secret: EncodingKey::from_secret(public_pem.as_bytes()),
            verifying,
            valid_token: String::new(),
        };
        harness.valid_token = harness.token(&TokenCase::default());
        harness
    }

    /// A correctly signed token with every claim valid.
    pub fn valid_token(&self) -> &str {
        &self.valid_token
    }

    /// Authenticates one bearer and checks the outcome against the oracle.
    ///
    /// # Panics
    ///
    /// When the router accepts a bearer the oracle refuses, or returns a
    /// different principal than the token names.
    pub fn check_bearer(&self, token: &str) -> bool {
        let outcome = self.provider.authenticate_bearer(token);
        let expected = Oracle {
            verifying: &self.verifying,
        }
        .expected_principal(token);
        match (outcome, expected) {
            (Ok(principal), Ok(expected)) => {
                assert_eq!(
                    principal, expected,
                    "accepted principal differs from the token's claims: {token}"
                );
                true
            }
            (Ok(_), Err(reason)) => {
                panic!("router accepted a bearer the oracle refuses ({reason}): {token}")
            }
            (Err(_), _) => false,
        }
    }

    /// Structured target: decodes `data` into header, claim, encoding and
    /// signing choices, builds that token, and checks it.
    pub fn structured(&self, data: &[u8]) -> bool {
        let case = TokenCase::decode(&mut Input::new(data));
        self.check_bearer(&self.token(&case))
    }

    /// Raw target: an `Authorization` header value, or byte edits of a valid
    /// token, through the router's strict bearer extraction.
    pub fn raw(&self, data: &[u8]) -> bool {
        let Some((&mode, rest)) = data.split_first() else {
            return false;
        };
        let header = if mode % 2 == 0 {
            String::from_utf8_lossy(rest).into_owned()
        } else {
            let mut token = self.valid_token.clone().into_bytes();
            for edit in rest.chunks(3) {
                let [high, low, operation] = [
                    edit[0],
                    *edit.get(1).unwrap_or(&0),
                    *edit.get(2).unwrap_or(&0),
                ];
                let position = usize::from(u16::from_be_bytes([high, low])) % (token.len() + 1);
                match operation % 4 {
                    0 if position < token.len() => token[position] ^= operation | 1,
                    1 => token.insert(position, operation),
                    2 if position < token.len() => {
                        token.remove(position);
                    }
                    3 => token.truncate(position),
                    _ => {}
                }
            }
            format!("Bearer {}", String::from_utf8_lossy(&token))
        };
        match crate::server::strict_bearer_token(&header) {
            Some(token) => self.check_bearer(token),
            None => false,
        }
    }

    fn token(&self, case: &TokenCase) -> String {
        let header = case.header_json();
        let payload = case.payload_json();
        let (header_part, payload_part) = case.encoding.encode(&header, &payload);
        let message = format!("{header_part}.{payload_part}");
        let signature = self.signature(case, message.as_bytes());
        case.encoding.frame(&message, &signature)
    }

    fn signature(&self, case: &TokenCase, message: &[u8]) -> String {
        let sign = |key: &EncodingKey, algorithm: Algorithm| {
            jsonwebtoken::crypto::sign(message, key, algorithm).unwrap_or_default()
        };
        // The algorithm the header claims, when the harness can produce it.
        let claimed = match case.alg {
            Alg::Rs384 => Some((Algorithm::RS384, &self.trusted)),
            Alg::Rs512 => Some((Algorithm::RS512, &self.trusted)),
            Alg::Ps256 => Some((Algorithm::PS256, &self.trusted)),
            Alg::Hs256 => Some((Algorithm::HS256, &self.public_key_as_secret)),
            _ => None,
        };
        let valid = || match claimed {
            Some((algorithm, key)) => sign(key, algorithm),
            None => sign(&self.trusted, Algorithm::RS256),
        };
        match case.signing {
            Signing::Valid => valid(),
            Signing::UntrustedKey => sign(&self.untrusted, Algorithm::RS256),
            Signing::PublicKeyAsHmacSecret => sign(&self.public_key_as_secret, Algorithm::HS256),
            Signing::Empty => String::new(),
            Signing::Truncated => {
                let signature = valid();
                signature[..signature.len() / 2].to_owned()
            }
            Signing::FlippedByte => {
                let mut bytes = URL_SAFE_NO_PAD.decode(valid()).unwrap_or_default();
                if let Some(last) = bytes.last_mut() {
                    *last ^= 1;
                }
                URL_SAFE_NO_PAD.encode(bytes)
            }
            Signing::OtherMessage => {
                jsonwebtoken::crypto::sign(b"e30.e30", &self.trusted, Algorithm::RS256)
                    .unwrap_or_default()
            }
            Signing::Padded => URL_SAFE.encode(URL_SAFE_NO_PAD.decode(valid()).unwrap_or_default()),
            Signing::StandardAlphabet => {
                STANDARD_NO_PAD.encode(URL_SAFE_NO_PAD.decode(valid()).unwrap_or_default())
            }
            Signing::Rs384 => sign(&self.trusted, Algorithm::RS384),
            Signing::Ps256 => sign(&self.trusted, Algorithm::PS256),
        }
    }
}

/// JWKS document target: parses one document as the refresh path does.
///
/// # Panics
///
/// When a key set the router keeps exceeds its documented bound.
pub fn jwks_document(data: &[u8]) -> bool {
    match crate::jwt::key_set_from_document_for_harness(data) {
        Ok(kept) => {
            assert!((1..=128).contains(&kept), "kept {kept} keys");
            true
        }
        Err(_) => false,
    }
}

/// The public half of an RSA JWK as PEM, as an algorithm-confusion attacker
/// would copy it from the issuer's published key set.
fn public_key_pem(jwk: &Jwk) -> String {
    let jsonwebtoken::jwk::AlgorithmParameters::RSA(rsa) = &jwk.algorithm else {
        unreachable!("the synthetic key is RSA");
    };
    // The exact text is irrelevant: any public, issuer-published bytes model
    // the forgery, and the router must never accept an HMAC.
    format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n{}\n-----END PUBLIC KEY-----\n",
        rsa.n, rsa.e
    )
}

// ----- decoding fuzz input into a token ---------------------------------------

struct Input<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Input<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn byte(&mut self) -> u8 {
        let byte = self.data.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        byte
    }

    /// One of `count` choices; an exhausted input selects choice 0.
    fn choice(&mut self, count: usize) -> usize {
        usize::from(self.byte()) % count
    }

    fn u16(&mut self) -> u16 {
        u16::from_be_bytes([self.byte(), self.byte()])
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Alg {
    #[default]
    Rs256,
    Hs256,
    None,
    Rs384,
    Rs512,
    Ps256,
    Es256,
    Lowercase,
    Empty,
    Absent,
    Number,
}

#[derive(Clone, Copy, Default)]
enum Signing {
    #[default]
    Valid,
    UntrustedKey,
    PublicKeyAsHmacSecret,
    Empty,
    Truncated,
    FlippedByte,
    OtherMessage,
    Padded,
    StandardAlphabet,
    Rs384,
    Ps256,
}

#[derive(Clone, Copy, Default)]
enum Encoding {
    #[default]
    Canonical,
    HeaderPadded,
    PayloadPadded,
    PayloadStandardAlphabet,
    PayloadByteOrderMark,
    PayloadInvalidUtf8,
    TrailingDot,
    ExtraSegment,
    LeadingSpace,
    TwoSegments,
}

impl Encoding {
    fn encode(self, header: &str, payload: &str) -> (String, String) {
        let header_part = match self {
            Self::HeaderPadded => URL_SAFE.encode(header),
            _ => URL_SAFE_NO_PAD.encode(header),
        };
        let payload_part = match self {
            Self::PayloadPadded => URL_SAFE.encode(payload),
            Self::PayloadStandardAlphabet => STANDARD_NO_PAD.encode(payload),
            Self::PayloadByteOrderMark => URL_SAFE_NO_PAD.encode(format!("\u{feff}{payload}")),
            Self::PayloadInvalidUtf8 => {
                let mut bytes = payload.as_bytes().to_vec();
                if let Some(index) = bytes.iter().rposition(|byte| *byte == b'"') {
                    bytes.insert(index, 0xff);
                }
                URL_SAFE_NO_PAD.encode(bytes)
            }
            _ => URL_SAFE_NO_PAD.encode(payload),
        };
        (header_part, payload_part)
    }

    fn frame(self, message: &str, signature: &str) -> String {
        match self {
            Self::TrailingDot => format!("{message}.{signature}."),
            Self::ExtraSegment => format!("{message}.{signature}.e30"),
            Self::LeadingSpace => format!(" {message}.{signature}"),
            Self::TwoSegments => message.to_owned(),
            _ => format!("{message}.{signature}"),
        }
    }
}

/// One claim's JSON text, or `None` when the claim is absent.
type Claim = Option<String>;

#[derive(Clone, Default)]
struct TokenCase {
    alg: Alg,
    kid: usize,
    extra_header: usize,
    iss: usize,
    aud: usize,
    exp: usize,
    nbf: usize,
    sub: usize,
    scope: usize,
    scopes: usize,
    roles: usize,
    duplicate: usize,
    padding: u16,
    encoding: Encoding,
    signing: Signing,
}

const ALGS: [Alg; 11] = [
    Alg::Rs256,
    Alg::Hs256,
    Alg::None,
    Alg::Rs384,
    Alg::Rs512,
    Alg::Ps256,
    Alg::Es256,
    Alg::Lowercase,
    Alg::Empty,
    Alg::Absent,
    Alg::Number,
];
const SIGNINGS: [Signing; 11] = [
    Signing::Valid,
    Signing::UntrustedKey,
    Signing::PublicKeyAsHmacSecret,
    Signing::Empty,
    Signing::Truncated,
    Signing::FlippedByte,
    Signing::OtherMessage,
    Signing::Padded,
    Signing::StandardAlphabet,
    Signing::Rs384,
    Signing::Ps256,
];
const ENCODINGS: [Encoding; 10] = [
    Encoding::Canonical,
    Encoding::HeaderPadded,
    Encoding::PayloadPadded,
    Encoding::PayloadStandardAlphabet,
    Encoding::PayloadByteOrderMark,
    Encoding::PayloadInvalidUtf8,
    Encoding::TrailingDot,
    Encoding::ExtraSegment,
    Encoding::LeadingSpace,
    Encoding::TwoSegments,
];

fn kids() -> Vec<Claim> {
    vec![
        Some(json!(KID).to_string()),
        Some(json!("unknown-key").to_string()),
        Some(json!("").to_string()),
        None,
        Some("7".to_owned()),
        Some(json!("k".repeat(4096)).to_string()),
        Some(json!(format!("{KID} ")).to_string()),
    ]
}

fn extra_headers() -> Vec<&'static str> {
    vec![
        "",
        r#","typ":"JWT""#,
        r#","typ":"at+jwt""#,
        r#","crit":["exp"]"#,
        r#","crit":[]"#,
        r#","b64":false,"crit":["b64"]"#,
        r#","jku":"https://attacker.example.test/jwks.json""#,
        r#","x5u":"https://attacker.example.test/cert.pem""#,
        r#","jwk":{"kty":"oct","k":"c2VjcmV0"}"#,
        r#","zzz":1"#,
    ]
}

fn issuers() -> Vec<Claim> {
    let text = |value: JsonValue| Some(value.to_string());
    vec![
        text(json!(ISSUER)),
        text(json!("https://other.example.test")),
        text(json!(ISSUER.to_uppercase())),
        text(json!(format!("{ISSUER}/"))),
        None,
        text(json!(5)),
        text(json!([ISSUER])),
        text(json!([])),
        text(json!(null)),
        text(json!([ISSUER, "https://other.example.test"])),
    ]
}

fn audiences() -> Vec<Claim> {
    let text = |value: JsonValue| Some(value.to_string());
    vec![
        text(json!(AUDIENCE)),
        text(json!([OTHER_AUDIENCE, AUDIENCE])),
        text(json!([OTHER_AUDIENCE])),
        text(json!([])),
        None,
        text(json!(format!("{AUDIENCE} "))),
        text(json!(5)),
        text(json!([5])),
        text(json!(null)),
        text(json!(AUDIENCE.to_uppercase())),
        text(json!([AUDIENCE, 5])),
    ]
}

fn expiries() -> Vec<Claim> {
    let number = |value: u64| Some(value.to_string());
    vec![
        number(NOW + 3600),
        number(NOW + 1),
        number(NOW),
        number(NOW - 1),
        number(NOW - LEEWAY),
        number(NOW - LEEWAY + 1),
        number(NOW - LEEWAY - 1),
        number(0),
        number(u64::MAX),
        number(1 << 63),
        Some("-1".to_owned()),
        Some(format!("{}.0", NOW + 3600)),
        Some(format!("{}e0", NOW + 3600)),
        Some(format!("\"{}\"", NOW + 3600)),
        None,
        Some("null".to_owned()),
        Some("18446744073709551616".to_owned()),
    ]
}

fn not_befores() -> Vec<Claim> {
    let number = |value: u64| Some(value.to_string());
    vec![
        None,
        number(NOW),
        number(NOW + 1),
        number(NOW + LEEWAY),
        number(NOW + LEEWAY + 1),
        Some("null".to_owned()),
        Some("\"x\"".to_owned()),
        Some("-1".to_owned()),
        number(u64::MAX),
        Some(format!("{NOW}.5")),
    ]
}

fn subjects() -> Vec<Claim> {
    let text = |value: JsonValue| Some(value.to_string());
    vec![
        text(json!("user-1")),
        text(json!("")),
        None,
        text(json!(5)),
        text(json!(null)),
        text(json!(" ")),
        text(json!("u".repeat(10_000))),
        text(json!("user\u{0}")),
        text(json!("用户-é")),
    ]
}

fn scope_claims() -> Vec<Claim> {
    let text = |value: JsonValue| Some(value.to_string());
    vec![
        None,
        text(json!("records.read")),
        text(json!("records.read records.write")),
        text(json!("a  b")),
        text(json!(" a")),
        text(json!("")),
        text(json!("a\tb")),
        text(json!("é")),
        text(json!("a\"b")),
        text(json!(5)),
        text(json!(["a"])),
        text(json!("b a b")),
        text(json!(vec!["s"; 1000].join(" "))),
        text(json!("a\\b")),
    ]
}

fn legacy_scopes() -> Vec<Claim> {
    vec![
        None,
        Some(r#"["records.read"]"#.to_owned()),
        Some(r#""records.read""#.to_owned()),
        Some("[]".to_owned()),
    ]
}

fn roles() -> Vec<Claim> {
    vec![
        None,
        Some("[]".to_owned()),
        Some(r#"["operator"]"#.to_owned()),
        Some("null".to_owned()),
        Some(r#""operator""#.to_owned()),
        Some(r#"[""]"#.to_owned()),
    ]
}

/// A second spelling of one claim, before or after its first spelling.
const DUPLICATES: [Option<(&str, &str, bool)>; 8] = [
    None,
    Some(("iss", "\"https://other.example.test\"", false)),
    Some(("iss", "\"https://other.example.test\"", true)),
    Some(("aud", "\"another-resource\"", true)),
    Some(("exp", "1", false)),
    Some(("exp", "1", true)),
    Some(("sub", "\"intruder\"", true)),
    Some(("scope", "\"admin\"", true)),
];

impl TokenCase {
    fn decode(input: &mut Input<'_>) -> Self {
        Self {
            alg: ALGS[input.choice(ALGS.len())],
            kid: input.choice(kids().len()),
            extra_header: input.choice(extra_headers().len()),
            iss: input.choice(issuers().len()),
            aud: input.choice(audiences().len()),
            exp: input.choice(expiries().len()),
            nbf: input.choice(not_befores().len()),
            sub: input.choice(subjects().len()),
            scope: input.choice(scope_claims().len()),
            scopes: input.choice(legacy_scopes().len()),
            roles: input.choice(roles().len()),
            duplicate: input.choice(DUPLICATES.len()),
            padding: input.u16(),
            encoding: ENCODINGS[input.choice(ENCODINGS.len())],
            signing: SIGNINGS[input.choice(SIGNINGS.len())],
        }
    }

    fn header_json(&self) -> String {
        let alg = match self.alg {
            Alg::Rs256 => Some("\"RS256\""),
            Alg::Hs256 => Some("\"HS256\""),
            Alg::None => Some("\"none\""),
            Alg::Rs384 => Some("\"RS384\""),
            Alg::Rs512 => Some("\"RS512\""),
            Alg::Ps256 => Some("\"PS256\""),
            Alg::Es256 => Some("\"ES256\""),
            Alg::Lowercase => Some("\"rs256\""),
            Alg::Empty => Some("\"\""),
            Alg::Absent => None,
            Alg::Number => Some("1"),
        };
        let mut members = Vec::new();
        if let Some(alg) = alg {
            members.push(format!("\"alg\":{alg}"));
        }
        if let Some(kid) = &kids()[self.kid] {
            members.push(format!("\"kid\":{kid}"));
        }
        format!(
            "{{{}{}}}",
            members.join(","),
            extra_headers()[self.extra_header]
        )
        .replacen("{,", "{", 1)
    }

    fn payload_json(&self) -> String {
        let mut members: Vec<(&str, String)> = Vec::new();
        let claims = [
            ("iss", issuers()[self.iss].clone()),
            ("aud", audiences()[self.aud].clone()),
            ("exp", expiries()[self.exp].clone()),
            ("nbf", not_befores()[self.nbf].clone()),
            ("sub", subjects()[self.sub].clone()),
            ("scope", scope_claims()[self.scope].clone()),
            ("scopes", legacy_scopes()[self.scopes].clone()),
            ("authorization_roles", roles()[self.roles].clone()),
        ];
        for (name, value) in claims {
            if let Some(value) = value {
                members.push((name, value));
            }
        }
        if let Some((name, value, after)) = DUPLICATES[self.duplicate] {
            let duplicate = (name, value.to_owned());
            if after {
                members.push(duplicate);
            } else {
                members.insert(0, duplicate);
            }
        }
        if self.padding % 4 == 1 {
            // Up to 256 KiB of ignored claim text.
            members.push((
                "pad",
                json!("p".repeat(usize::from(self.padding) * 4)).to_string(),
            ));
        }
        let body = members
            .iter()
            .map(|(name, value)| format!("\"{name}\":{value}"))
            .collect::<Vec<_>>()
            .join(",");
        format!("{{{body}}}")
    }
}

// ----- the independent oracle --------------------------------------------------

/// Re-implements bearer acceptance from the specification and the router's
/// documented policy.
struct Oracle<'a> {
    verifying: &'a DecodingKey,
}

impl Oracle<'_> {
    fn expected_principal(&self, token: &str) -> Result<AuthenticatedPrincipal, &'static str> {
        let parts = token.split('.').collect::<Vec<_>>();
        let [header, payload, signature] = parts[..] else {
            return Err("a compact JWS has exactly three parts");
        };
        let header: JsonValue = URL_SAFE_NO_PAD
            .decode(header)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or("the header is not unpadded base64url JSON")?;
        let header = header.as_object().ok_or("the header is not an object")?;
        if header.get("alg") != Some(&json!("RS256")) {
            return Err("only RS256 is accepted");
        }
        if header.get("kid") != Some(&json!(KID)) {
            return Err("the key ID must name a trusted key");
        }
        if header.contains_key("crit") {
            return Err("no critical header extension is understood (RFC 7515 4.1.11)");
        }
        let message = token
            .rsplit_once('.')
            .map(|(message, _)| message)
            .unwrap_or_default();
        if !jsonwebtoken::crypto::verify(
            signature,
            message.as_bytes(),
            self.verifying,
            Algorithm::RS256,
        )
        .unwrap_or(false)
        {
            return Err("the RS256 signature does not verify under the trusted key");
        }
        // RFC 7519 4: a parser may keep only the lexically last duplicate.
        let claims: JsonValue = URL_SAFE_NO_PAD
            .decode(payload)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or("the payload is not unpadded base64url JSON")?;
        let claims = claims.as_object().ok_or("the claims are not an object")?;
        if claims.get("iss") != Some(&json!(ISSUER)) {
            return Err("iss must be exactly the configured issuer string");
        }
        let audience_ok = match claims.get("aud") {
            Some(JsonValue::String(audience)) => audience == AUDIENCE,
            Some(JsonValue::Array(values)) => {
                values.iter().all(JsonValue::is_string)
                    && values.iter().any(|value| value == AUDIENCE)
            }
            _ => false,
        };
        if !audience_ok {
            return Err("aud must name the configured audience");
        }
        let exp = claims
            .get("exp")
            .and_then(JsonValue::as_u64)
            .ok_or("exp must be a non-negative integer")?;
        if exp.saturating_add(LEEWAY) <= NOW {
            return Err("the token is expired");
        }
        let expires_at = UNIX_EPOCH
            .checked_add(Duration::from_secs(exp))
            .ok_or("exp is not a representable time")?;
        match claims.get("nbf") {
            None | Some(JsonValue::Null) => {}
            Some(value) => match value.as_u64() {
                Some(not_before) if not_before <= NOW + LEEWAY => {}
                Some(_) => return Err("the token is not yet valid"),
                None => return Err("nbf must be a non-negative integer"),
            },
        }
        let subject = claims
            .get("sub")
            .and_then(JsonValue::as_str)
            .filter(|subject| !subject.trim().is_empty())
            .ok_or("sub must be a non-empty string")?;
        if claims.contains_key("scopes") {
            return Err("the legacy scopes claim is refused by default");
        }
        let scopes = match claims.get("scope") {
            None => BTreeSet::new(),
            Some(JsonValue::String(scope)) if !scope.is_empty() => {
                let scopes = scope.split(' ').collect::<BTreeSet<_>>();
                if scopes.iter().any(|scope| {
                    scope.is_empty()
                        || !scope
                            .bytes()
                            .all(|byte| matches!(byte, 0x21 | 0x23..=0x5b | 0x5d..=0x7e))
                }) {
                    return Err("scope tokens are RFC 6749 scope-token characters");
                }
                scopes
            }
            Some(_) => return Err("scope must be a non-empty string"),
        };
        if cfg!(feature = "auth-agql") {
            match claims.get("authorization_roles") {
                None => {}
                Some(JsonValue::Array(roles)) if roles.is_empty() => {}
                Some(_) => return Err("role grants need a configured role catalogue"),
            }
        }
        AuthenticatedPrincipal::new(subject, scopes, Some(expires_at))
            .map_err(|_| "the principal itself is invalid")
    }
}

// ----- synthetic key material --------------------------------------------------

/// Synthetic RSA key the harness's JWKS trusts. Test material only; it has
/// never protected anything.
pub const SYNTHETIC_RSA_PRIVATE_KEY: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEAyRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTL
UTv4l4sggh5/CYYi/cvI+SXVT9kPWSKXxJXBXd/4LkvcPuUakBoAkfh+eiFVMh2V
rUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8H
oGfG/AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBI
Mc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi+yUod+j8MtvIj812dkS4QMiRVN/
by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQIDAQABAoIBAHREk0I0O9DvECKd
WUpAmF3mY7oY9PNQiu44Yaf+AoSuyRpRUGTMIgc3u3eivOE8ALX0BmYUO5JtuRNZ
Dpvt4SAwqCnVUinIf6C+eH/wSurCpapSM0BAHp4aOA7igptyOMgMPYBHNA1e9A7j
E0dCxKWMl3DSWNyjQTk4zeRGEAEfbNjHrq6YCtjHSZSLmWiG80hnfnYos9hOr5Jn
LnyS7ZmFE/5P3XVrxLc/tQ5zum0R4cbrgzHiQP5RgfxGJaEi7XcgherCCOgurJSS
bYH29Gz8u5fFbS+Yg8s+OiCss3cs1rSgJ9/eHZuzGEdUZVARH6hVMjSuwvqVTFaE
8AgtleECgYEA+uLMn4kNqHlJS2A5uAnCkj90ZxEtNm3E8hAxUrhssktY5XSOAPBl
xyf5RuRGIImGtUVIr4HuJSa5TX48n3Vdt9MYCprO/iYl6moNRSPt5qowIIOJmIjY
2mqPDfDt/zw+fcDD3lmCJrFlzcnh0uea1CohxEbQnL3cypeLt+WbU6kCgYEAzSp1
9m1ajieFkqgoB0YTpt/OroDx38vvI5unInJlEeOjQ+oIAQdN2wpxBvTrRorMU6P0
7mFUbt1j+Co6CbNiw+X8HcCaqYLR5clbJOOWNR36PuzOpQLkfK8woupBxzW9B8gZ
mY8rB1mbJ+/WTPrEJy6YGmIEBkWylQ2VpW8O4O0CgYEApdbvvfFBlwD9YxbrcGz7
MeNCFbMz+MucqQntIKoKJ91ImPxvtc0y6e/Rhnv0oyNlaUOwJVu0yNgNG117w0g4
t/+Q38mvVC5xV7/cn7x9UMFk6MkqVir3dYGEqIl/OP1grY2Tq9HtB5iyG9L8NIam
QOLMyUqqMUILxdthHyFmiGkCgYEAn9+PjpjGMPHxL0gj8Q8VbzsFtou6b1deIRRA
2CHmSltltR1gYVTMwXxQeUhPMmgkMqUXzs4/WijgpthY44hK1TaZEKIuoxrS70nJ
4WQLf5a9k1065fDsFZD6yGjdGxvwEmlGMZgTwqV7t1I4X0Ilqhav5hcs5apYL7gn
PYPeRz0CgYALHCj/Ji8XSsDoF/MhVhnGdIs2P99NNdmo3R2Pv0CuZbDKMU559LJH
UvrKS8WkuWRDuKrz1W/EQKApFjDGpdqToZqriUFQzwy7mR3ayIiogzNtHcvbDHx8
oFnGY0OFksX/ye0/XGpy2SFxYRwGU98HPYeBvAQQrVjdkzfy7BmXQQ==
-----END RSA PRIVATE KEY-----"#;

/// A second synthetic RSA key the JWKS does not contain.
const UNTRUSTED_RSA_PRIVATE_KEY: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEogIBAAKCAQEApkck+Xp5qsCaCJPIA9ohY6ltCe0Cgk8S5ZDKjGp9ksXYk2R6
bYWdv3FfcC47+h5oSlCipdBHjSxR+BXVmTddoWDtm0XTOQ48CYZ74LK16yptmKN9
d9kKGafsuaFkY9iJX02DOgjyRP7yb/l31hxSlwUCdSXXuNbR9aEBNZqBf1mslakO
N8Bp7xXoqTeUH0D8oYz6SCfxHI6Kz62xGtuK9VPRCC0fRtQNsQmUtvdp2VY2g7ad
Ew/CBk5WuUi2tIPXMllS6nT78+09Bfkj6NUlUPpEoZOgaxjXODqfciYwkjSD+QWA
vcMX67+xiEQTy2e9DG4gXf7IXB9kxLlWV0mcfQIDAQABAoIBABeK1SpZc+aANI7o
VZZuCPIyZ9TS8NsAVR2ZvYPaBAOG7Z4YyNi+5VHYvHmGoklMitmSxySQ88CCOOzW
MpLp6uDRd3LXlQtr/4Ydu2ZaBzXPoQrOgxI38Z0mUHcL6cnVO8X5lI3f/GVWb375
JEV1ZyAovmtVu1BZYgrUC/bJ9HHSRs8FtZWds6A0FI1ZOwtIPiwgwuoaIsEXiwWY
oZv+GkwdBWUHWOvLLrQ21BUUF5bfggqDG3oXEsw7zj3ZCjLUrL6f3fH4DfZDmrYe
ZSFHptBWFluWzBlRF/Jm+cgVQ3XDcPT/KBdDxk7e10gQ6w5LFQNeywkLxKn9uV99
rvbLt+ECgYEA47L7etRhzGyhX1/UUFPnOBGvxIhVK6otriyDGXOF2N4NND5nktRf
13J7r5Bk4XVrfe3xfjHd4iWgvZ45R8tbxO4E4Um+/m9/OEVy7e3u/j8w8+FZxy+G
JaX4QZsVoY7dW/L66RZWTnlaobNt1kP7olcqrBCDzp6p7AqoHWgdzjUCgYEAuvHW
eUeic0z+2Prf/BNu6orDDeL/hl8j+CrjGmi+KvsSJsKiGRCz373H6DAkGjpIOR9a
Z+4slJsb8RHPZcE1/O6mCXSflvyz9TMLRECtl5sn2Crhp5GsbVRg/Eh7iNUTfOA2
pmAAEdnEbvYhqnH7T7HD19lOySJzwEJ3dJGf/ikCgYB1YLaywvkHYJhvif7wyc/P
3440SfNd5wRVXSzX2e2NtiooXGMYFdKyRZntDwpyDw2yhQ6QdzRid/M8zGvKztIn
2ms5l8KVdvSmFWcBsD4bYofhsLGPNLh6f+xSJZgpCHSkCcW+kn07RuOpp81fPvxg
235r5LeNs8z0mSnAvwNeYQKBgB2FRPX/3uJQPUosS6R1uSUmZmjR+MHSUk7FDvnD
oowM+6/NOmW04oKpaGD9ASZ3H6KNlewlizqoR8BIrLSHzyuyz9tcZDsmipBk0Bmm
EQOIx8D1/I5GjbDyCyKrm35fD2W/zkF1lW08My3GhkhxGVtbWe7C7RML8rVgiZaJ
zHcJAoGAZWuruFhIFF3si1UCo3PcDcIj8PyFt4vAH8JvB8eYkOPo9ypLy7ibhQYu
ZKaZZqrbQiRbdCZ56bQdDwq9tMm29Hf0Uz/AAmw7iaSntpdEYDmzW/oXcd/kj6ru
174JuKN3TAY2Q381sT3L3BMzWuKrhNIVW3da8RiS7IxYnTsgJao=
-----END RSA PRIVATE KEY-----"#;

#[cfg(test)]
mod tests;
