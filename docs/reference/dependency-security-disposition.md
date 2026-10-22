---
title: Dependency security disposition
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-10-22
review_by: 2026-11-15
supersedes: []
---

# Dependency security disposition

The reviewed dependency graph retains advisories. The
[frozen scan evidence](../archive/2026/dependency-review-2026-10-15/README.md)
is complete **with findings**, including all eight unsuppressed raw audit
outputs. This reference records the technical dispositions and their conditions;
it does not make an affected dependency sound or certify an application's graph.

The snapshot covers the unpublished ORM/macros 0.50.1 candidate and the reviewed
agql-auth 0.21.3 revision. It is not a published release or whole-workspace
runtime acceptance. The generated [package inventory](workspace-packages.md)
and release manifest own package identities. The
[release process](../operations/release/process.md) owns validation and
publication gates; this reference creates no additional approval process.
Application rollout steps belong to the
[security upgrade guide](../how-to/upgrade-security-remediation.md).

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

Storage 0.7.4 carries the reviewed SMB 0.10.2 directory-stream backport as an
optional owner-local dependency. Its [provenance](../../crates/graphql-orm-storage/vendor/smb/PROVENANCE.json)
identifies the original registry archive, exact upstream patch and retained MIT
license. The [supplemental evidence](../archive/2026/dependency-review-2026-10-15/smb-addendum.md)
records actual 5d155ba source/lock mappings and complete-with-findings collection;
the earlier frozen scan remains evidence for its original source.

cargo-audit 0.22.2 [skips non-default-registry sources](https://github.com/RustSec/rustsec/blob/cargo-audit/v0.22.2/rustsec/src/database.rs#L133-L143),
including source-less workspace paths and downstream Git dependencies. Ordinary
root/fixture scans therefore do not assess the backported package. The
supplemental original-registry SMB identity/checksum lookup uses the same frozen
advisory database and requires individual applicability review against patched
files and selected features; it does not establish absence of vulnerabilities.
Registry companions retain ordinary audit coverage. Reassess upstream advisories
and the backport whenever its code, features or source changes.

## Retained findings and conditions

Maintenance roles and the **2026-11-15** review date below are recommended
follow-up responsibilities, not assertions that an individual accepted risk.
Re-review immediately when a condition changes, and expose retained findings
in the protected-release review packet.

| Finding | Technical disposition for reviewed paths | Maintenance follow-up |
| --- | --- | --- |
| quick-xml [RUSTSEC-2026-0194](https://rustsec.org/advisories/RUSTSEC-2026-0194.html) / [0195](https://rustsec.org/advisories/RUSTSEC-2026-0195.html) | Affected Hive/object_store reader remains selected. The project router's supported configuration excludes its credential XML initialization; standalone Hive and host extensions are outside this condition. | Router maintainers: seek an owning fixed object_store/quick-xml update; review storage, persisted documents, loaders, plugins and environment/configuration merging. |
| lru [RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253.html) | Unsound non-AWS copies remain. Generated loaders use NoCache; supported router telemetry does not initialize Datadog. Reviewed concrete libdd keys do not supply a panicking destructor. Public generic caches remain conditional host risk. | ORM/router maintainers: seek owning fixed LRU requirements; review custom caches, key destructors, unwind recovery and exporters. |
| rsa [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html) | No fixed release is listed in the reviewed advisory. No private-key-decryption oracle was found in reviewed JWT operations or the native SMB username/password NTLM path. This is not a general constant-time claim. | Auth/storage maintainers: monitor upstream; review key reuse, decryption APIs, crypto providers, credentials, mechanisms and unified features. |

### Router XML initialization

The router constructs [typed internal Hive configuration](../../crates/graphql-orm-router/src/server.rs)
in `build_hive_config`, directly deserializes it, exposes no raw Hive storage or
plugin registry, and supplies no storage map. Its
[file configuration](../../crates/graphql-orm-router/src/file_config.rs) rejects
unknown fields; the CLI uses that project loader rather than Hive's overlay
loader. Persisted-document options remain disabled through initialization and
reload. Hive's empty StorageManager constructs no S3 runtime or credential reader.

Generic Hive S3 WebIdentity/STS and S3 Express credentials have real XML response
paths. object_store's serde reader constructs NsReader, so neither advisory is
excluded merely because the consumer uses deserialization. Ordinary GraphQL input
alone was not shown to reach those readers. Exposing upstream storage, raw engine
configuration, plugins or loader/environment merging invalidates this disposition.

Primary source anchors: [reviewed project translation](https://github.com/Dastari/graphql-orm/blob/84e7d0b0542da7c4425f3875ab9547f26d24dbb8/crates/graphql-orm-router/src/server.rs#L866),
[Hive StorageManager](https://github.com/graphql-hive/router/blob/15c50cf622e9d83bd4a25b3e190a124bcb67ee19/bin/router/src/storage/mod.rs#L18),
[persisted-document initialization](https://github.com/graphql-hive/router/blob/15c50cf622e9d83bd4a25b3e190a124bcb67ee19/bin/router/src/pipeline/persisted_documents/mod.rs#L183),
and [object_store's fixed owning requirement](https://raw.githubusercontent.com/apache/arrow-rs-object-store/v0.14.1/Cargo.toml).
The reviewed fixed owner crosses Hive's existing requirement; use a compatible
owning update rather than an unreviewed semver override.

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

Primary sources: [fix](https://github.com/jeromefroe/lru-rs/pull/238),
[async-graphql cache defaults](https://docs.rs/crate/async-graphql/7.2.1/source/src/dataloader/mod.rs),
[libdd key type](https://docs.rs/crate/libdd-sampling/6.0.0/source/src/glob_matcher.rs),
and [Hive exporter gate](https://github.com/graphql-hive/router/blob/15c50cf622e9d83bd4a25b3e190a124bcb67ee19/bin/router/src/telemetry/traces/mod.rs#L119).

### RSA operations and credentials

Reviewed auth paths parse public keys, verify signatures and sign through the
selected jsonwebtoken randomized/blinded signing implementation. They do not
provide a private-key-decryption endpoint. Reusing that signing key in another
decryption service invalidates this reasoning.

The [native SMB provider](../../crates/graphql-orm-storage/src/smb.rs) passes
username/password credentials. Its selected mechanism is restricted to
`ntlm,!kerberos,!pku2u`; the reviewed NTLM path uses symmetric cryptography.
Other mechanisms, credentials and downstream feature unification need a new
trace. SQLx MySQL is absent from the selected profiles and outside the advertised
ORM backend set; lockfile presence is a separate observation.

Primary sources: [auth signing](https://github.com/Dastari/agql-auth/blob/59c0a38f82e8c75459474238799a81f29d4ffa58/src/service.rs#L1456),
[JWKS public-key extraction](https://github.com/Dastari/agql-auth/blob/59c0a38f82e8c75459474238799a81f29d4ffa58/src/service.rs#L2271),
[selected JWT RSA implementation](https://docs.rs/crate/jsonwebtoken/10.4.0/source/src/crypto/rust_crypto/rsa.rs),
[SMB authenticator](https://docs.rs/crate/smb/0.10.2/source/src/session/authenticator.rs),
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

The scan uses a fixed advisory revision and then-available local registry/index
data. It certifies neither future advisories nor registry freshness. The original
eight reports and the two supplemental changed-lock reports retain nonzero
findings exits; there is no broad ignore list. The supplemental identity-only
lookup exits zero within the scope above; the aggregate remains with findings.

This dependency review records graph/source dispositions separately from
feature, provider and owned-service runtime acceptance. Follow the release
process for final validation; individual scoped passes do not certify the
complete workspace or a consuming application's unified graph. Runner controls,
compilation and selected graphs are distinct from runtime passes. Preserve the
actual tested revision when reusing scoped prior evidence.

Applications must review their own unified feature graph, current configuration,
custom caches, policies and crypto APIs. Dependency or configuration changes
invalidate the corresponding condition immediately, before the scheduled review.
