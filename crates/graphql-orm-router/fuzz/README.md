---
title: graphql-orm-router fuzz targets
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-11-18
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router fuzz targets

cargo-fuzz targets for the router's bearer-authentication parsing boundary
([#726](https://github.com/Dastari/graphql-orm/issues/726)). Every key and
token is synthetic, the clock is fixed and no target uses the network.

| Target | Input |
| --- | --- |
| `bearer_structured` | Bytes that choose the JOSE header (`alg`, `kid`, `crit`, embedded keys), each claim (`iss`, `aud`, `exp`, `nbf` around the leeway, `sub`, `scope`, legacy `scopes`, roles, duplicates, padding), the encoding, and the signature (valid, untrusted key, HMAC over the public key, empty, truncated, altered, re-encoded, other RSA algorithms). |
| `bearer_raw` | An `Authorization` header value, or byte edits of a valid bearer, through the strict `Bearer` extraction. |
| `jwks_document` | A JWKS document through the refresh path's key selection. |

The harnesses live in `src/fuzzing.rs`, compiled only for tests and `--cfg
graphql_orm_router_fuzzing` builds; the generic `--cfg fuzzing` that `cargo
fuzz` sets for every crate does not expose it. A target fails when the router
accepts a bearer that an independent oracle refuses, or returns a principal
other than the token's claims name. The oracle re-implements RFC 7515/7519 and
the router's policy: RS256 only, a trusted key ID, no `crit`, an RS256
signature checked with `jsonwebtoken::crypto::verify`, `iss` exactly the
configured string, `aud` naming the configured audience, integer `exp` and
`nbf` within the leeway, a non-empty `sub`, and well-formed scopes. Refusing a
valid token is not a target failure.

The external `agql-auth` validator and signed role catalogue are outside
these targets; only the router's adapters around them are covered. The
`AgqlAuthenticationProvider` adapter refuses any `crit` header member before
calling the validator, which does not check it; `src/agql.rs` tests that and
the adapter's syntax check and expiry conversion.

## Ordinary CI

`cargo test -p graphql-orm-router` replays every file in `corpus/<target>/`
and `regressions/<target>/` through the same harness, then runs a short
seeded property test of each target. It takes a few seconds.

## Longer runs

The weekly [fuzz workflow](../../../.github/workflows/fuzz.yml) runs each
target for 20 minutes with a 2 GiB memory limit. To run one locally on the
workspace toolchain:

```sh
crates/graphql-orm-router/fuzz/run.sh bearer_structured 600 2048
```

`run.sh` passes the same coverage flags as `cargo fuzz --sanitizer none`, so
it needs no nightly toolchain and no extra Cargo tool. With cargo-fuzz and a
nightly toolchain, run `RUSTFLAGS="--cfg graphql_orm_router_fuzzing" cargo
fuzz run <target>` from this directory; cargo-fuzz adds its own flags to that
`RUSTFLAGS`. New inputs go to a scratch directory; the retained corpus is read
only.

## Retaining a failure

A failure writes its input to `artifacts/<target>/`. Minimize it with
`cargo fuzz tmin <target> <file>` (or the binary's `-minimize_crash=1`), add it
under `regressions/<target>/` with a descriptive name, and fix the router in
the same change. The regression then runs in ordinary CI. To shrink the seed
corpus after adding inputs, use `cargo fuzz cmin <target>`.
