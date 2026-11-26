---
title: Dependency security disposition
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-23
review_by: 2027-02-15
supersedes: []
---

# Dependency security disposition

The reviewed dependency graph retains advisories. The
[frozen scan evidence](../archive/2026/dependency-review-2026-10-15/README.md)
is complete **with findings**, including all eight unsuppressed raw audit
outputs. This reference records the technical dispositions and their conditions;
it does not make an affected dependency sound or certify an application's graph.

The current review covers every tracked root and nested fixture lockfile on
2026-11-14. The October evidence linked above remains historical evidence for
its original source, not the current graph. This is not whole-workspace runtime
acceptance. The generated [package inventory](workspace-packages.md)
owns package paths and the release manifest owns package versions and
external revisions. The
[release process](../operations/release/process.md) owns validation and
publication gates; this reference creates no additional approval process.
Application rollout steps belong to the
[security upgrade guide](../how-to/upgrade-security-remediation.md).


## Dated third-party advisory exceptions

Accepted on **2026-11-15** under the maintainer policy: minor or theoretical
advisories are accepted; never fork or patch a third-party crate; re-check when
upstream ships a fix. These exceptions retain the path conditions below and
the machine-readable review expiry; they do not authorize new private-key or
untrusted-parser uses. A serious, actually reachable vulnerability requires
maintainer review. Existing vendor/fork history is not permission for new patches.
Later exceptions carry their own accepted date.

| Accepted date | Advisory/package/version | One-line reason |
| --- | --- | --- |
| 2026-11-15 | `RUSTSEC-2023-0071:rsa:0.9.10` | JWT verification/signing and optional SMB NTLM paths do not expose a private RSA decryption oracle; no fixed compatible release. Review crypto API/key reuse changes. |
| 2026-11-15 | `RUSTSEC-2023-0089:atomic-polyfill:1.0.3` | Optional heapless/rstar spatial compatibility bridge; obsolete upstream owner. Retained in fixture/root resolution, not a claim of selected runtime reachability. |
| 2026-11-15 | `RUSTSEC-2024-0449:unsafe-libyaml:0.2.11` | Hive telemetry/VRL serde_yaml owner is unmaintained; replacement crosses owning dependency/API boundaries. No universal unreachable claim. |
| 2026-11-15 | `RUSTSEC-2026-0194:quick-xml:0.39.4` | Hive object_store 0.13 reader requires quick-xml 0.39; fixed owner 0.14 crosses its range. Typed router config excludes storage/credential XML initialization; host extensions invalidate this condition. |
| 2026-11-15 | `RUSTSEC-2026-0195:quick-xml:0.39.4` | Hive object_store 0.13 reader requires quick-xml 0.39; fixed owner 0.14 crosses its range. Typed router config excludes storage/credential XML initialization; host extensions invalidate this condition. |
| 2026-11-15 | `RUSTSEC-2026-0253:lru:0.16.3` | async-graphql/libdd retain incompatible old ranges; fixed lru starts at 0.18.2. Generated DataLoaders use NoCache; router does not initialize Datadog. Generic host caches remain exposed to panicking-key destructors. |
| 2026-11-15 | `RUSTSEC-2026-0253:lru:0.16.4` | async-graphql/libdd retain incompatible old ranges; fixed lru starts at 0.18.2. Generated DataLoaders use NoCache; router does not initialize Datadog. Generic host caches remain exposed to panicking-key destructors. |
| 2026-11-15 | `RUSTSEC-2026-0387:crypto-bigint:0.5.5` | Pinned agql-auth/jsonwebtoken RustCrypto elliptic-curve path; no compatible fixed release. Existing owner acceptance excludes private EC operations; auth pin move requires maintainer approval. |
| 2026-11-15 | `RUSTSEC-2026-0441:pin-utils:0.1.0` | aws-smithy-runtime (S3), sspi -> async-dnssd (optional SMB), and Hive reqwest-retry -> wasmtimer retain the unmaintained utility. Requires owning upstream migrations; maintenance status is not treated as a vulnerability exclusion. |
| 2026-11-15 | `RUSTSEC-2026-0459:libsqlite3-sys:0.30.1` | Exploitation requires opening an attacker-written database file together with its -wal; backup restore is logical, not file-level, so the risk is theoretical. |
| 2026-11-17 | `RUSTSEC-2026-0543:chrono:0.4.45` | Informational 'unmaintained' notice (chronotope/chrono#1768), no vulnerability. Re-check when upstream adopts a maintained successor or a security advisory appears. |
| 2026-11-17 | `RUSTSEC-2026-0555:chrono-tz:0.10.4` | Informational 'unmaintained' notice (chronotope/chrono#1768), no vulnerability. Re-check when upstream adopts a maintained successor or a security advisory appears. |
| 2026-11-17 | `RUSTSEC-2026-0542:humantime:2.4.0` | Informational 'unmaintained' notice (chronotope/chrono#1768), no vulnerability; the workspace reaches humantime through the Hive Router dependency tree. Re-check when upstream adopts a maintained successor or a security advisory appears. |
| 2026-11-19 | `RUSTSEC-2026-0574:heapless:{0.6.1,0.7.17,0.8.0}` | Unsound `FindMutView::pop`, not called: rstar uses only `binary_heap::BinaryHeap`; 0.6.1/0.7.17 are lockfile-only (geo-types weak `serde` features). No geo/geo-types release accepts heapless 0.9. |
| 2026-11-19 | `RUSTSEC-2026-0575:heapless:{0.6.1,0.7.17,0.8.0}` | Unsound spsc queue `peek` ordering, not called: rstar uses only `binary_heap::BinaryHeap`; 0.6.1/0.7.17 are lockfile-only (geo-types weak `serde` features). No geo/geo-types release accepts heapless 0.9. |
| 2026-11-19 | `RUSTSEC-2026-0576:heapless:{0.6.1,0.7.17,0.8.0}` | Unsound `IndexMap::truncate`, not called: rstar uses only `binary_heap::BinaryHeap`; 0.6.1/0.7.17 are lockfile-only (geo-types weak `serde` features). No geo/geo-types release accepts heapless 0.9. |
| 2026-11-19 | `RUSTSEC-2026-0577:heapless:{0.6.1,0.7.17,0.8.0}` | Unsound panicking element `Drop` (including `vec::IntoIter::drop`), not called: rstar uses only `binary_heap::BinaryHeap`; 0.6.1/0.7.17 are lockfile-only (geo-types weak `serde` features). No geo/geo-types release accepts heapless 0.9. |
| 2026-11-19 | `RUSTSEC-2026-0578:heapless:{0.6.1,0.7.17,0.8.0}` | Unsound `SortedLinkedList::zeroize`, not called: rstar uses only `binary_heap::BinaryHeap`; 0.6.1/0.7.17 are lockfile-only (geo-types weak `serde` features). No geo/geo-types release accepts heapless 0.9. |
| 2026-11-19 | `RUSTSEC-2026-0579:heapless:{0.6.1,0.7.17,0.8.0}` | Unsound `Deque` `TryFrom` (affected only from 0.9.2), not called: rstar uses only `binary_heap::BinaryHeap`; 0.6.1/0.7.17 are lockfile-only (geo-types weak `serde` features). No geo/geo-types release accepts heapless 0.9. |
| 2026-11-19 | `RUSTSEC-2026-0580:heapless:{0.6.1,0.7.17,0.8.0}` | Unsound treiber-stack pools, not called: rstar uses only `binary_heap::BinaryHeap`; 0.6.1/0.7.17 are lockfile-only (geo-types weak `serde` features). No geo/geo-types release accepts heapless 0.9. |
| 2026-11-19 | `RUSTSEC-2026-0581:heapless:{0.6.1,0.7.17,0.8.0}` | Unsound `Deque::make_contiguous`, not called: rstar uses only `binary_heap::BinaryHeap`; 0.6.1/0.7.17 are lockfile-only (geo-types weak `serde` features). No geo/geo-types release accepts heapless 0.9. |
| 2026-11-19 | `RUSTSEC-2026-0582:heapless:{0.6.1,0.7.17,0.8.0}` | Unsound `IndexMap::insert` at `u16::MAX`, not called: rstar uses only `binary_heap::BinaryHeap`; 0.6.1/0.7.17 are lockfile-only (geo-types weak `serde` features). No geo/geo-types release accepts heapless 0.9. |

### Dispositions rechecked on 2026-11-23

The review covered the tracked locks and current registry metadata. The fixed
releases fit the Rust 1.97.1 toolchain and MSRV lane, but the current owning
requirements prevent a complete lockfile-only fix.

| Identity | Review by | Evidence |
| --- | --- | --- |
| [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), `rsa 0.9.10` | 2026-12-23 | Rechecked 2026-11-24. Auth 0.27 removes its direct RSA dependency. JWT 11 RustCrypto, the ORM JWT 10 test dependency, the SMB SSPI stack and lock-only SQLx MySQL 0.8.6 still require 0.9.x. The fix is in 0.10.0. The deployed router and ORM paths use no RSA private key. The RSA operations section below distinguishes public-key verification, fixture signing, and external agql-auth signing paths. |
| [RUSTSEC-2023-0089](https://rustsec.org/advisories/RUSTSEC-2023-0089.html), `atomic-polyfill 1.0.3` | 2027-02-23 | The archive notice has no patched release. Weak geo-types `serde` features retain rstar 0.9/0.10/0.11 and heapless 0.7.17 in the locks. The selected spatial path uses rstar 0.12 and heapless 0.8, which replaced atomic-polyfill with portable-atomic. Current geo-types retains the older rstar requirements. |
| [RUSTSEC-2024-0449](https://rustsec.org/advisories/RUSTSEC-2024-0449.html), `unsafe-libyaml 0.2.11` | 2027-02-23 | The unmaintained notice has no patched release. Datadog/libdd and the VRL build use serde_yaml 0.9.34. Hive/VRL uses serde_yaml_ng 0.10.0 at runtime. Both require unsafe-libyaml. Newer VRL still uses serde_yaml_ng, and replacing either parser with serde-saphyr requires an owning migration. The typed router uses its own JSON configuration and supplies no Datadog exporter. |
| [RUSTSEC-2026-0194](https://rustsec.org/advisories/RUSTSEC-2026-0194.html), `quick-xml 0.39.4` | 2027-02-23 | The duplicate-attribute CPU fix starts at 0.41.0. object_store 0.14.1 and later accept it, but both pinned Hive 0.2.19 and Hive 0.3.19 require object_store `^0.13.2`. Router initialization supplies no Hive storage map and uses only local JSON when persisted documents are enabled. |
| [RUSTSEC-2026-0195](https://rustsec.org/advisories/RUSTSEC-2026-0195.html), `quick-xml 0.39.4` | 2027-02-23 | The `NsReader` namespace-allocation fix also starts at 0.41.0 and hits the same object_store requirement. The router's local JSON manifest path bypasses object_store and its S3 credential XML readers. |
| [RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253.html), `lru 0.16.3` | 2027-02-23 | The root and four nested locks reach the affected copy via async-graphql (`^0.16.2`) or Datadog and libdd-sampling (`^0.16.3`). Current owner releases still exclude fixed 0.18.2. Generated loaders use `NoCache` and the typed router supplies no Datadog exporter. The AWS-owned 0.18.5 copy is already fixed. |
| [RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253.html), `lru 0.16.4` | 2027-02-23 | Fixtures resolve through async-graphql 7.2.1 (`^0.16.2`) and, in federation-end-to-end, through Hive 0.2.19's Datadog and libdd sampling dependencies (`^0.16.3`). The fix starts at 0.18.2. Generated loaders use `DataLoader::new` with `NoCache`. The cross-crate fixture's custom cache uses `HashMapCache`. The typed Hive translation configures optional Prometheus metrics and supplies no Datadog exporter. The exception excludes host LRU caches with potentially panicking key destructors. |
| [RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html), `crypto-bigint 0.5.5` | 2026-12-23 | Rechecked 2026-11-24. Auth 0.27 and the router share JWT 11 in the root lock. JWT 11 and the ORM JWT 10 test dependency still use elliptic-curve 0.13.8 (`crypto-bigint ^0.5`). JWT 12 adopts the newer curves but crosses the current requirements. Router JWT verification uses public keys and workspace signing fixtures use RSA or HS256. The owner acceptance retains the conditions below. |
| [RUSTSEC-2026-0441](https://rustsec.org/advisories/RUSTSEC-2026-0441.html), `pin-utils 0.1.0` | 2027-02-23 | The unmaintained notice includes 0.1.1. aws-smithy-runtime 1.16.1 and wasmtimer 0.4.4 drop the dependency. The optional SMB path still resolves it because sspi 0.16.1 requires async-dnssd 0.5 on macOS and iOS, and that line retains pin-utils. |
| [RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html), `libsqlite3-sys 0.30.1` | 2027-02-23 | SQLx SQLite 0.8.6 requires `^0.30.1`, and the cross-crate fixture pins `=0.30.1`. SQLx 0.9.1 can resolve the fixed SQLite library starting at 0.39.0. It crosses the ORM's public SQLx contract. Backup restores logical table and change records into a host-selected connection. This disposition assumes the host controls writes to the database and its sibling WAL. |

## Owner-accepted crypto-bigint 0.5.5 disposition

**RUSTSEC-2026-0387; scope: `crypto-bigint` version `0.5.5` only.**
Accepted by the workspace owner on **2026-11-04**, with an initial review due
**2026-12-04**. The table above records the next review date.
This is a timing side channel on secret arithmetic. This workspace does not
do ECDSA private-key operations; the exposure belongs to agql-auth's signing
API; no compatible fixed release exists in the current owning dependency ranges.
Patched crypto-bigint releases exist on the incompatible 0.7 line; this
disposition does not describe those releases as unavailable.

The retained root path is `agql-auth 0.27.0 → jsonwebtoken 11.0.0 → p256/p384
0.13 → elliptic-curve 0.13.8 → crypto-bigint 0.5.5`. The router shares that JWT
version, and the ORM's JWT 10.4.0 test dependency also reaches those curves.
Search of workspace source, examples, fixtures and tests found no EC
`EncodingKey` construction, ES256/ES384 signing, or calls to Auth's EC client
signing API. JWT signing fixtures construct RSA keys and use RS256. The router
runtime verifies signatures with public keys; public-input verification does
not perform the secret arithmetic described by this advisory. SMB's selected
credential path uses NTLM and symmetric signing. S3's SigV4a owner is updated
to aws-sigv4 1.6.4, using the patched crypto-bigint 0.7.10 line; it does not rely
on this disposition.

This acceptance does not cover Auth signing services, consumer applications, other crate
versions, or a future workspace EC signer. Introducing private EC operations invalidates
it immediately. Keep the raw unsuppressed audit finding; the exact-version CI
disposition below does not suppress raw reports. Follow up upstream on a JWT aws_lc_rs
backend or a release adopting the corrected RustCrypto owners.

## TLS certificate distrust patches

The maintained workspace and affected fixture locks select published
`rustls-webpki` 0.103.16 and `webpki-roots` / `webpki-root-certs` 1.0.10.
These address [RUSTSEC-2026-0382](https://rustsec.org/advisories/RUSTSEC-2026-0382.html),
[RUSTSEC-2026-0381](https://rustsec.org/advisories/RUSTSEC-2026-0381.html) and
[RUSTSEC-2026-0383](https://rustsec.org/advisories/RUSTSEC-2026-0383.html): distrusted
emSign intermediate public keys and the affected bundled trust anchor.
The 0.26.11 compatibility root crates do not carry an independent safe bundle;
their resolved 1.x parent must also be patched. Downstream applications must
refresh their own locks and rebuild; updating this repository does not update
an application's resolved graph or installed binaries.

The October 15 frozen scan below predates these advisories and is not evidence
for the repaired lockfiles. Qualify the exact release source, selected graphs
and current advisory snapshot under the release process; retain raw findings
and nonzero scan exits. The previous quick-xml, LRU, RSA and maintenance
findings below remain conditional and are not erased by the TLS updates.
Router binary distribution additionally requires refreshed lock-bound notices
and its separate approval; source-only delivery does not supply that evidence.

## Owning dependency fixes

The frozen locks and selected graphs confirm the modern S3 transport and
owning SDK update, compatible QUIC/Rustls/WebPKI patches, same-revision Tiberius
native TLS, removal of the old Decimal/rkyv bridge and legacy PEM owner, and the
reviewed yanked patch replacements. The AWS-owned LRU copy is fixed; unrelated
old LRU copies remain. Exact versions, advisory identities, selectors and
comparison against the earlier snapshot belong to the archived evidence.

Transport migrations also change toolchain/platform requirements. Follow the
[storage migration guide](../../crates/graphql-orm-storage/MIGRATION.md) for the
S3 SDK's Rust minimum, and the [SQL Server guide](graphql-orm/mssql.md) for native
TLS dependencies, trust and full-session encryption configuration. A lockfile
update alone does not validate a deployment image.

## Owner-local SMB dependency

Storage 0.7.4 carries the reviewed SMB 0.10.2 directory-stream backport as an optional
owner-local dependency. Its
[provenance](../../crates/graphql-orm-storage/vendor/smb/PROVENANCE.json) identifies the
original registry archive, exact upstream patch and retained MIT license. The
[supplemental evidence](../archive/2026/dependency-review-2026-10-15/smb-addendum.md)
records actual 5d155ba source/lock mappings and complete-with-findings collection; the
earlier frozen scan remains evidence for its original source.

cargo-audit 0.22.2 [skips non-default-registry
sources](https://github.com/RustSec/rustsec/blob/cargo-audit/v0.22.2/rustsec/src/database.rs#L133-L143),
including source-less workspace paths and downstream Git dependencies. Ordinary
root/fixture scans therefore do not assess the backported package. The supplemental
original-registry SMB identity/checksum lookup uses the same frozen advisory database
and requires individual applicability review against patched files and selected
features; it does not establish absence of vulnerabilities. Registry companions retain
ordinary audit coverage. Reassess upstream advisories and the backport whenever its
code, features or source changes.

Storage 0.8.1 adds an owner-local security patch for
[GHSA-439h-2cx8-xwgx](https://github.com/Dastari/graphql-orm/security/advisories/GHSA-439h-2cx8-xwgx):
the client verifies signing or encryption before acting on a response status. Upstream
smb-rs has the same ordering, so a registry update would not fix it; provenance records
the patch, and the [native SMB
reference](../../crates/graphql-orm-storage/docs/native-smb.md#response-verification)
states the corrected behaviour. Re-review it with any vendor update.

## Retained findings and conditions

Maintenance roles and the **2026-12-04** review date below are recommended
follow-up responsibilities, not assertions that an individual accepted risk.
Re-review immediately when a condition changes, and expose retained findings
in the protected-release review packet.

| Finding | Technical disposition for reviewed paths | Maintenance follow-up |
| --- | --- | --- |
| quick-xml [RUSTSEC-2026-0194](https://rustsec.org/advisories/RUSTSEC-2026-0194.html) / [0195](https://rustsec.org/advisories/RUSTSEC-2026-0195.html) | Affected Hive/object_store reader remains selected. The project router's supported configuration excludes its credential XML initialization; standalone Hive and host extensions are outside this condition. | Router maintainers: seek an owning fixed object_store/quick-xml update; review storage, persisted documents, loaders, plugins and environment/configuration merging. |
| lru [RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253.html) | Unsound non-AWS copies remain. Generated loaders use NoCache; supported router telemetry does not initialize Datadog. Reviewed concrete libdd keys do not supply a panicking destructor. Public generic caches remain conditional host risk. | ORM/router maintainers: seek owning fixed LRU requirements; review custom caches, key destructors, unwind recovery and exporters. |
| rsa [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html) | RustSec lists a fix in 0.10.0. The reviewed runtime paths either perform public verification or use symmetric credentials. The pinned 0.9 owners are listed in the current review above. | Auth/storage maintainers should review newer owners, key reuse, decryption APIs, providers, and authentication mechanisms. |

### Router XML initialization

The router constructs [typed internal Hive
configuration](../../crates/graphql-orm-router/src/server.rs) in `build_hive_config`,
directly deserializes it, exposes no raw Hive storage or plugin registry, and supplies
no storage map. Its [file
configuration](../../crates/graphql-orm-router/src/file_config.rs) rejects unknown
fields; the CLI uses that project loader rather than Hive's overlay loader.
Persisted documents are disabled by default. When enabled, the router sends Hive
a validated private local JSON snapshot as a `file` source, including on reload.
Hive resolves remote storage only for its separate `storage` source. The empty
StorageManager constructs no S3 runtime or credential reader.

Generic Hive S3 WebIdentity/STS and S3 Express credentials have real XML response
paths. object_store's serde reader constructs NsReader, so neither advisory is
excluded merely because the consumer uses deserialization. Ordinary GraphQL input
alone was not shown to reach those readers. Exposing upstream storage, raw engine
configuration, plugins or loader/environment merging invalidates this disposition.

Primary source anchors: [reviewed project
translation](https://github.com/Dastari/graphql-orm/blob/84e7d0b0542da7c4425f3875ab9547f26d24dbb8/crates/graphql-orm-router/src/server.rs#L866),
[Hive
StorageManager](https://github.com/graphql-hive/router/blob/15c50cf622e9d83bd4a25b3e190a124bcb67ee19/bin/router/src/storage/mod.rs#L18),
[persisted-document
initialization](https://github.com/graphql-hive/router/blob/15c50cf622e9d83bd4a25b3e190a124bcb67ee19/bin/router/src/pipeline/persisted_documents/mod.rs#L183),
and [object_store's fixed owning
requirement](https://raw.githubusercontent.com/apache/arrow-rs-object-store/v0.14.1/Cargo.toml).
The reviewed fixed owner crosses Hive's existing requirement; use a compatible owning
update rather than an unreviewed semver override.

### LRU defaults and extensions

The reported unsoundness involves a panicking key destructor, recovery from
unwinding and subsequent cache use. Generated
[DataLoaders](../../crates/graphql-orm-macros/src/schema_roots.rs) use
`DataLoader::new`, whose reviewed default is NoCache. The router configures
logging and optional Prometheus metrics, leaves tracing exporters empty and
does not initialize Datadog. Environment variables do not create an absent
exporter. Separately reviewed libdd glob-cache keys are `Vec<u8>`, whose element
destruction does not supply the reported trigger.

These conditions do not make lru sound or cover async-graphql's public generic
LruCache or independently initialized host exporters. LRU 0.16.4 is still
affected; the upstream fix starts at 0.18.2. The selected AWS-owned 0.18.5 copy
does not repair unrelated 0.16.x owners.

Primary sources: [fix](https://github.com/jeromefroe/lru-rs/pull/238), [async-graphql
cache defaults](https://docs.rs/crate/async-graphql/7.2.1/source/src/dataloader/mod.rs),
[libdd key type](https://docs.rs/crate/libdd-sampling/6.0.0/source/src/glob_matcher.rs),
and [Hive exporter
gate](https://github.com/graphql-hive/router/blob/15c50cf622e9d83bd4a25b3e190a124bcb67ee19/bin/router/src/telemetry/traces/mod.rs#L119).

### RSA operations and credentials

The deployed router verifies bearer tokens using public JWKS keys, either
directly or through agql-auth's `AccessTokenValidator`. The ORM consumes
already-validated principals. Neither runtime signs or decrypts with an RSA
private key in the reviewed paths. Router tests and the dedicated fuzz build
sign JWTs with synthetic RSA keys. The ORM's AuthService fixtures use HS256.

The external agql-auth pin can use RSA private keys in two configured paths.
`AuthService` signs an RS256 probe when validating a key pair and signs RS256
tokens on issuance. `ClientJwtSigner` signs a probe at construction and uses
RS256 or PS256 for OAuth/OIDC client assertions. These paths use jsonwebtoken's
randomized, blinded RSA signing and are outside the router/ORM runtime paths
reviewed here. Private-key decryption, reuse of a signing key for decryption,
or adding host private-key operations requires a new review.

The [native SMB provider](../../crates/graphql-orm-storage/src/smb.rs) passes
username/password credentials. Its selected mechanism is restricted to
`ntlm,!kerberos,!pku2u`; the reviewed NTLM path uses symmetric cryptography.
Other mechanisms, credentials and downstream feature unification need a new
trace. SQLx MySQL is absent from the selected profiles and outside the advertised
ORM backend set; lockfile presence is a separate observation.

Primary sources: [auth
signing](https://github.com/Dastari/agql-auth/blob/71a3acee5942bd5b6aac57dcaccbb6391b383880/src/service.rs#L2033),
[client assertion signing](https://github.com/Dastari/agql-auth/blob/71a3acee5942bd5b6aac57dcaccbb6391b383880/src/client_auth.rs#L386),
[JWKS public-key
extraction](https://github.com/Dastari/agql-auth/blob/71a3acee5942bd5b6aac57dcaccbb6391b383880/src/service.rs#L3198),
[selected JWT RSA
implementation](https://docs.rs/crate/jsonwebtoken/11.0.0/source/src/crypto/rust_crypto/rsa.rs),
[SMB
authenticator](https://docs.rs/crate/smb/0.10.2/source/src/session/authenticator.rs),
and [RSA upstream tracking](https://github.com/RustCrypto/RSA/issues/626).

## Maintenance warnings and acceptance limits

The [atomic-polyfill warning](https://rustsec.org/advisories/RUSTSEC-2023-0089.html)
remains in locks through older optional spatial compatibility bridges but is
absent from the 67 reviewed selected profiles. Spatial maintainers should seek
owning cleanup and re-review added compatibility features.
The [unsafe-libyaml warning](https://rustsec.org/advisories/RUSTSEC-2024-0449.html)
remains selected through router/federation Datadog/VRL dependencies. Router
maintainers should pursue an owning parser migration with YAML compatibility
review; no blanket exclusion for all VRL/configuration paths is established.
Unmaintained status alone does not establish an exploit.

The scan uses a fixed advisory revision and then-available local registry/index data. It
certifies neither future advisories nor registry freshness. The original eight reports
and the two supplemental changed-lock reports retain nonzero findings exits; raw reports
have no ignore list; the exact CI dispositions above are separate. The supplemental
identity-only lookup exits zero within the scope above; the aggregate remains with
findings.

This dependency review records graph/source dispositions separately from
feature, provider and owned-service runtime acceptance. Follow the release
process for final validation; individual scoped passes do not certify the
complete workspace or a consuming application's unified graph. Runner controls,
compilation and selected graphs are distinct from runtime passes. Preserve the
actual tested revision when reusing scoped prior evidence.

Applications must review their own unified feature graph, current configuration,
custom caches, policies and crypto APIs. Dependency or configuration changes
invalidate the corresponding condition immediately, before the scheduled review.

## Automated review on 2026-11-14

CI and full release validation install cargo-audit 0.22.2 with its locked build
graph and scan all tracked lockfiles against the current RustSec database.
Raw JSON reports, including retained findings and the database revision, are
uploaded even on scan failure. New advisory IDs, affected package versions,
maintenance warnings and yanks fail the gate. Scanner execution/parse failures
also fail. No severity threshold or blanket ignore applies.

The single machine-readable [disposition file](../../dependency-audit-dispositions.json)
contains exact advisory/package/version identities, reasons and expiry dates. Changing
or renewing it requires ordinary repository review. The dated exceptions above record the maintainer acceptance policy. The existing crypto-bigint owner acceptance
above remains limited to its original conditions. CI warns during the fourteen days
before expiry, fails after expiry, and fails on identities absent from every tracked
lock. Remove obsolete entries during review; follow the [release review
procedure](../operations/release/process.md#dependency-disposition-review). Raw
cargo-audit output is never filtered. Advisory/index connectivity failures fail closed;
a fresh database clone and index preflight precede the JSON scans.

Compatible lockfile updates repair Quinn protocol/UDP findings
RUSTSEC-2026-0523, RUSTSEC-2026-0524 and RUSTSEC-2026-0538, rustix
RUSTSEC-2026-0486 and the yanked clipboard-win 5.4.1 release. All affected nested
locks receive the same compatible rustix patch where present.

| Retained identity | Owning path and disposition |
| --- | --- |
| crypto-bigint 0.5.5 / RUSTSEC-2026-0387 | agql-auth, router, and ORM tests → jsonwebtoken 11 → p256/p384 → elliptic-curve. Existing conditional acceptance above. |
| libsqlite3-sys 0.30.1 / RUSTSEC-2026-0459 | Exploitation requires opening an attacker-written database file together with its -wal; backup restore is logical, not file-level, so the risk is theoretical. |
| quick-xml 0.39.4 / RUSTSEC-2026-0194, RUSTSEC-2026-0195 | hive-router → object_store 0.13. Fixed object_store owner crosses Hive's range. Router XML initialization conditions above apply. |
| rsa 0.9.10 / RUSTSEC-2023-0071 | JWT RustCrypto and optional SMB crypto owners; no compatible fixed release. RSA operations and credentials conditions above apply. |
| lru 0.16.3 and fixture 0.16.4 / RUSTSEC-2026-0253 | async-graphql DataLoader and Hive/libdd owners; neither range accepts fixed 0.18.2. NoCache/exporter/key conditions above apply; generic host caches remain exposed. |
| heapless 0.6.1, 0.7.17, 0.8.0 / RUSTSEC-2026-0574 to RUSTSEC-2026-0582 | geo 0.33.1 → rstar 0.12.2 → heapless 0.8.0 is the only built path; geo-types 0.7.19 weak `rstar_0_8?/serde`-style features lock rstar 0.8.4–0.11.0 (heapless 0.6.1/0.7.17) without building them. rstar calls only `BinaryHeap` new/push/pop/len/capacity/into_vec in its nearest-neighbour heap, whose spilled `vec::IntoIter` holds elements without drop glue; the ORM calls only geo `Relate`. Update when geo/geo-types accept rstar 0.14 (heapless 0.9). |
| atomic-polyfill 1.0.3 / RUSTSEC-2023-0089 | heapless/rstar spatial compatibility resolution. Unmaintained; optional graph presence does not establish runtime use. Seek an owning cleanup. |
| pin-utils 0.1.0 / RUSTSEC-2026-0441 | S3 aws-smithy-runtime, optional SMB sspi → async-dnssd, and Hive reqwest-retry → wasmtimer. Unmaintained; replacing these owners needs upstream/API review. No exploit or unreachability inference. |
| unsafe-libyaml 0.2.11 / RUSTSEC-2024-0449 | Hive/VRL → serde_yaml and serde_yaml_ng. Unmaintained parser; migrate owning dependencies with YAML compatibility review. |

### Duplicate crypto and HTTP stacks (#498)

As of 2026-11-26, the router, pinned agql-auth v0.27.0 and ORM tests resolve
jsonwebtoken 11. The full-stack fixture follows the ORM backend feature
selection. Its SQLite build excludes the PostgreSQL driver and its TLS stack.
CI checks both fixture backends and each ORM backend with `auth-agql`.

| Remaining stack | Resolved owners and constraint |
| --- | --- |
| reqwest 0.12 and 0.13 | AI, router and the federation fixture use 0.13 with default features disabled. Hive directly requires 0.12 and also brings it in through its HTTP middleware and object_store. Hive telemetry uses 0.13. Cargo also locks SSPI's optional 0.12 dependency for SMB. |
| rustls with ring and AWS-LC | Hive enables both via its direct rustls/hyper-rustls dependencies and reqwest 0.12. The router's own reqwest 0.13 `rustls` feature also selects AWS-LC. S3 and the standard AI HTTP features select AWS-LC. PostgreSQL uses SQLx 0.8's ring provider with WebPKI roots. SQLx 0.8's `verify-ca` verifier relies on an unambiguous or host-installed process-wide provider. Switching PostgreSQL to AWS-LC would introduce a panic risk for hosts that explicitly pair PostgreSQL with AI's ring provider. AI's explicit ring option uses the platform verifier. AI tests also build ring through rcgen for the loopback ring test. |
| Native TLS/OpenSSL on Linux | The pinned Tiberius driver selects native TLS for SQL Server. Its rustls alternative uses tokio-rustls 0.24, adding a second rustls version. Switching transports requires the driver and certificate coverage tracked in #516. AI also offers native TLS as an explicit host choice. |

PostgreSQL and AI HTTP can share ring as their only rustls provider when the
host selects AI's `provider-*-core` adapters and `provider-tls-ring`. Hive or
S3 in the same application can still add AWS-LC through Cargo feature unification.

### Remaining repository/test hygiene (#545)

PR base SHAs now enter shells through environment variables. All tracked fixture
locks are scanned. The fixed qualified generator is still executed for the
historical serialization comparison: it is repository-owned code loaded from a
full immutable commit, not issue text or a moving ref. Only the current generator
performs package-tag verification, preserving that gate while avoiding obsolete
historical source-selection rules (#616).

Sleep-based tests, missing-docs policy expansion, committed agent tooling and the
synchronous public migration-file API require separate scoped decisions. No tests,
lints or tool directories are removed here. The synchronous API documentation
and timing-test refactors should be addressed with their owning runtime/API work.
