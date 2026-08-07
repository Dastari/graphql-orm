---
title: GraphQL ORM Router 0.1.0 source distribution review
kind: investigation
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2026-11-07
supersedes: []
---

# GraphQL ORM Router 0.1.0 source distribution review

## Question

May the `graphql-orm-router` 0.1.0 source tree be made available from the
public `Dastari/graphql-orm` repository at a full Git revision under the
distribution boundary in ADR-0008?

This review covers the source revision that contains this record and the root
`Cargo.lock` with SHA-256
`b7094bfbdd28b6957f9cafee492dad26775a46d0d323ca5a32b454d7bae798c2`.
It does not approve a compiled library, executable, container image, hosted
service, or a later lockfile.

## Evidence

CycloneDX 1.5 source-crate inventories were generated with
`cargo-cyclonedx 0.5.9`, a fixed `SOURCE_DATE_EPOCH`, the router manifest, and
explicit default and `auth-agql` feature lanes. These inventories deliberately
include build and development components, making them broader than the
normal-and-build runtime closures:

- [default feature inventory](evidence/graphql-orm-router-0.1.0-default.cdx.json)
  contains 636 components including the router and has SHA-256
  `cfe67b0edaa4d5f7405702397fe04e89591132a0ffd902fe795d1a8928871419`;
- [`auth-agql` inventory](evidence/graphql-orm-router-0.1.0-auth-agql.cdx.json)
  contains 668 components including the router and has SHA-256
  `00b5b57b00a9dae439ba0e91888dfe2ccd66bbad6830742590e77a1861c7a42a`.

The corresponding normal-and-build dependency trees contain 611 and 642
unique package/version nodes. The default inventory has declared license
metadata for every component. The optional inventory has one missing
declaration: first-party `agql-auth` 0.14.0 at exact revision
`413fda3435f060604cd653c11e2cc18a668aace1`. That repository is referenced but
not vendored in this source tree. Its missing declaration is accepted by the
project owner only for this first-party source channel; it must be corrected or
separately approved before third-party redistribution of an artifact containing
its code.

The inventory contains deprecated Cargo slash-form expressions such as
`MIT/Apache-2.0`, `Apache-2.0/MIT`, `BSD-3-Clause/MIT`, `Unlicense/MIT`, and
`Apache-2.0 / MIT`. They are retained verbatim in the SBOM as named-license
metadata and reviewed as the packages' historical dual-license choices. No
dependency declares GPL, LGPL, AGPL, EUPL, EPL, CDDL, SSPL, BUSL, or another
network-copyleft expression. This is a project distribution disposition, not
a legal interpretation of those licenses.

The default closure's MPL-2.0 notice inventory is:

- `cynic-parser` 0.11.2;
- `cynic-parser-deser` 0.11.2;
- `cynic-parser-deser-macros` 0.11.2;
- `graphql-composition` 0.12.2;
- `graphql-wrapping-types` 0.4.0; and
- `vrl` 0.33.1.

The optional `auth-agql` lane additionally contains MPL-2.0 `ascii_utils`
0.9.3 and `fast_chemail` 0.9.6. The public source revision does not vendor or
modify those packages. A binary or container review must determine the files
actually delivered and satisfy applicable notice, source-availability, and
modification obligations for that artifact.

Native or bundled-component candidates reviewed in the default source
inventory are `aws-lc-sys` 0.43.0, `inotify-sys` 0.1.8,
`libmimalloc-sys` 0.1.44, `linux-raw-sys` 0.12.1,
`ntex-io-uring` 0.7.120, `ring` 0.17.14, `zstd` 0.13.3, and
`zstd-sys` 2.0.16+zstd.1.5.7. Each declares a permissive license expression.
The source review does not assume which native components a later target or
container will compile or bundle.

`cargo audit` scanned all 950 packages in the workspace lockfile. The router's
normal-and-build closure reaches two accepted upstream-constrained findings:

- `quick-xml` 0.39.4 through Hive's unconditional `object_store` 0.13.2 edge,
  covering RUSTSEC-2026-0194 and RUSTSEC-2026-0195; and
- `rsa` 0.9.10 through Hive's unconditional `jsonwebtoken` RustCrypto edge,
  covering RUSTSEC-2023-0071.

ADR-0008 records the execution-boundary mitigations: the router exposes no
Hive storage/XML path and accepts no private keys or private-key operations.
The remaining three `rustls-webpki` findings and maintenance/yank warnings in
the workspace audit are not reachable from the router closure. These findings
remain monitored and do not authorize either excluded router capability.

## Conclusion

The designated project owner approved publishing the reviewed
`graphql-orm-router` 0.1.0 and `graphql-orm-router-protocol` 0.1.0 source from
the public repository at the full Git revision containing this evidence. The
source distribution gate in ADR-0008 is satisfied for that revision and
channel only.

No binary, container, hosted deployment, or subsequent dependency revision is
approved by this record. Those channels require an artifact-derived SBOM and
notice review, including confirmation of the components actually linked or
copied into the delivered artifact.

## Follow-up

- Keep the Hive, composition, and parser versions exact and repeat this review
  whenever their pins or the root lockfile change.
- Upgrade or remove the `quick-xml` and `rsa` paths when compatible upstream
  releases permit it; do not enable Hive storage or router private-key use in
  the meantime.
- Add explicit license metadata to `agql-auth` before approving redistribution
  of an `auth-agql` binary or container to a third party.
- Use the workspace release runbook and retain channel-specific evidence for
  every later router artifact.
