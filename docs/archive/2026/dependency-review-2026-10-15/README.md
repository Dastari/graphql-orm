---
title: Frozen dependency review evidence, 2026-10-15
kind: investigation
status: archived
owner: workspace-maintainers
last_reviewed: 2026-10-15
review_by: none
supersedes: []
---

# Frozen dependency review evidence, 2026-10-15

> **Archived evidence.** This is a historical scan of an unpublished candidate,
> not current release authorization or a claim about later advisories. Follow the
> active [dependency disposition](../../../reference/dependency-security-disposition.md)
> for the technical conditions and maintenance responsibilities.

## Identity and collection boundary

The reviewed workspace commit is
`84e7d0b0542da7c4425f3875ab9547f26d24dbb8`, tree
`5d90d9f178e05b0c972162c17c8b29c793485e69`, paired ORM/macros 0.50.1.
Its reviewed base/origin snapshot is
`4059d10026b9b8d1e9a2be0f4f556df807f77794`.
The external agql-auth 0.21.3 source is
`59c0a38f82e8c75459474238799a81f29d4ffa58`, tree
`76c3ff964b512e4f477fea1a7a0ec64848a8f065`.
This does not describe a published workspace tag.

The official RustSec clone completed at 2026-10-15T20:55:23.567842Z with revision
`edb58f5974b83eb8302332821cb6d389777aa351`; its commit timestamp is
2026-10-14T22:18:32+02:00. The clone happened to match the earlier packet's
revision; the earlier evidence was preserved. The database was then frozen for
all eight cargo-audit 0.22.2 invocations. Registry yank observations reflect the
scanner's then-available local index data, not an independently certified fresh
registry snapshot. Neither future advisory nor future yank status is covered.

Collection ran at 2026-10-15T21:18:15Z–21:18:58Z with Rust/Cargo 1.97.1 on
x86_64-unknown-linux-gnu. The compiler's full revision is
`8bab26f4f68e0e26f0bb7960be334d5b520ea452`; Cargo's is
`c980f4866141969fab6254a680546a277789d6f0`.
The constrained lane used one Cargo worker, 50% of one CPU, 4 GiB, no swap and
a 45-minute timeout. It created no services. Aggregate wrapper END1 preserves
the collector's findings; an independent cleanup0 was not separately logged.

Eight full locked metadata inputs and eight unsuppressed raw audits completed,
with 67 explicit selected normal/build/dev/feature graphs. Ordinary Cargo
deduplication preserves the complete graph without depth truncation. Backend
alternatives were selected separately, never workspace all-features. Each raw
audit exited1; collector status is complete **with findings**, not clean.

## Compact retained artifacts

[index.json](index.json) records all lock/source identities, exact native command
selectors and exits, per-output hashes, tool identities, source-condition Git
blobs/subtrees, and the complete structured comparison with the earlier packet.
The eight raw audit JSON files below are copied byte-for-byte, with their full
SHA256 values in that index. They were not filtered, reformatted or suppressed.

| Input | Raw output | Vulnerabilities | Warnings |
| --- | --- | --- | --- |
| Root | [JSON](raw-audits/root.json) | quick-xml RUSTSEC-2026-0194/-0195; rsa RUSTSEC-2023-0071 | lru 0.16.3, atomic-polyfill, unsafe-libyaml |
| Federation fixture | [JSON](raw-audits/federation-end-to-end.json) | Same three | lru 0.16.4, atomic-polyfill, unsafe-libyaml |
| Cross-crate fixture | [JSON](raw-audits/cross-crate-relations.json) | RSA only | lru 0.16.4, atomic-polyfill |
| Relationship fixture | [JSON](raw-audits/relationship-capability.json) | RSA only | lru 0.16.3, atomic-polyfill |
| Backend coexistence fixture | [JSON](raw-audits/backend-coexistence.json) | RSA only | lru 0.16.4, atomic-polyfill |
| Migration API fixture | [JSON](raw-audits/migration-api-compatibility.json) | RSA only | lru 0.16.4, atomic-polyfill |
| Repository consumer fixture | [JSON](raw-audits/repository-aggregate-consumer.json) | RSA only | lru 0.16.4, atomic-polyfill |
| Standalone authentication | [JSON](raw-audits/auth.json) | RSA only | None |

Warning identities are lru RUSTSEC-2026-0253 (unsound), atomic-polyfill
RUSTSEC-2023-0089 and unsafe-libyaml RUSTSEC-2024-0449 (unmaintained).
The active reference explains their distinct conditions; nonzero scan exits
remain visible regardless of a source disposition.

Large metadata/tree outputs are retained outside this repository in the exact
maintainer evidence packet `84e7d0b0542d-20261015T211815Z-fww78snq`.
Request that packet ID for independent review. The JSON index retains complete
selectors, exits and hashes rather than bundling those large outputs here.
Native historical argv/cwd strings in JSON are provenance, not portable local
navigation links. Packet summary/provenance/artifact-manifest SHA256 values are:

```text
summary.json          7c841a016620b959e7d2cc290d9eef895ed3b5d5cee7c552d3b2630ddf870ce8
provenance.json       49326564522cbeca0a2a7b6371a1aa92c3fc4a02dd3d4d2d07a8b1f93071bf14
artifact-sha256.json  0e6f7494dc3a49531ded6b85814d5922e5a2f43ec0633a868047b2b80562a5d7
```

## Remediation and comparison

All locked H2 copies are 0.4.16/0.4.19, quinn-proto 0.11.18, Rustls 0.23.46,
and rustls-webpki 0.103.15. Legacy H2 0.3, old Rustls/WebPKI, rustls-pemfile and
the rkyv 0.7 bridge are absent. Only rkyv 0.8.18 remains locked; no selected
profile activates it. The selected AWS S3 SDK 1.144.0 uses fixed lru 0.18.5.
No corresponding old H2/QUIC/Rustls/WebPKI/rkyv/PEM finding remains. Reviewed
chacha20/spin/unicode-segmentation patch replacements also remain in place.

The repaired advisory groups are H2 RUSTSEC-2026-0258; QUIC
RUSTSEC-2026-0338/-0339/-0340/-0341/-0342; Rustls
RUSTSEC-2026-0346/-0347/-0348/-0349; WebPKI
RUSTSEC-2026-0098/-0099/-0104; rkyv RUSTSEC-2026-0235; and the
rustls-pemfile maintenance warning RUSTSEC-2025-0134. The JSON index links the
primary advisory/fix references alongside the retained source identities.

Compared with the earlier 4059 packet, the seven workspace/fixture locks change
only local ORM/macros 0.48.0 to 0.50.1 and yoke-derive 0.8.3 to 0.8.4 in the
federation, migration API and repository consumer locks. Auth's lock is unchanged.
Exactly those three yank warnings disappear; no advisory or maintenance-warning
identity is added. Absence of a yank warning is limited to the scanner's registry
data boundary above.

The router/storage subtrees and generated DataLoader source are identical to
the reviewed 4059 source. The separate actual selected graphs establish feature
applicability: no sqlx-mysql, atomic-polyfill, rkyv or rustls-pemfile node is
selected; unsafe-libyaml remains in router/default-auth and federation profiles.
Source equality alone would not establish these graph facts or runtime passes.

## Acceptance limits

This evidence resolves the dependency collection/reconciliation task, not all
release acceptance. Fresh final-candidate S3/Samba fixture execution and remaining
local AI/package gates were pending at this checkpoint. The first AI provider
attempt stopped during compilation with a confirmed 4 GiB OOM before any test
completion. That is incomplete resource evidence, not a native test failure,
timeout or pass. Runner mocks are not runtime acceptance. Historical MSSQL
TLS/write/notification evidence keeps its actual tested source and separately
reviewed source/lock/selected-graph applicability proof.

User holds on Actions, remote merges and publication remain. No approval URL or
published tag is established by this archive. Follow the release runbook and
the current acceptance record before any future authorized release.
