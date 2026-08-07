---
title: GraphQL ORM Router 0.1.1 source distribution review
kind: investigation
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2026-11-07
supersedes: []
---

# GraphQL ORM Router 0.1.1 source distribution review

## Question

May the `graphql-orm-router` 0.1.1 source tree be made available from the
public `Dastari/graphql-orm` repository at the full Git revision containing
this record under the distribution boundary in ADR-0008?

This review covers the source-only patch revision and the root `Cargo.lock`
with SHA-256
`06ab9f61424dd933e9f2488520b2411ebb645aeb19c75b4d79642ede407ab1af`.
It does not approve a compiled library, executable, container image, hosted
service, or a later lockfile.

## Patch boundary

The patch changes first-party router authorization and tests so operation-local
GraphQL variables remain available when argument-templated scope requirements
are evaluated across the private WebSocket engine boundary. It also changes
the first-party router package version from 0.1.0 to 0.1.1. It changes no
dependency source, version, feature, license selection, protocol package,
configuration contract, public API, or stored-data format.

The root lockfile differs from the accepted 0.1.0 source review only in the
first-party `graphql-orm-router` package version. Regenerated CycloneDX
inventories differ from their 0.1.0 counterparts only in that component's
version, package URL, and dependency reference.

## Evidence

CycloneDX 1.5 source-crate inventories were regenerated with
`cargo-cyclonedx` 0.5.9, the same fixed `SOURCE_DATE_EPOCH`, the router
manifest, and explicit default and `auth-agql` lanes:

- [default feature inventory](evidence/graphql-orm-router-0.1.1-default.cdx.json)
  contains 636 components including the router and has SHA-256
  `4816b34fe19e3c703c60108990892e5e468cdbd57ba806fc4086d222c6a5526f`;
- [`auth-agql` inventory](evidence/graphql-orm-router-0.1.1-auth-agql.cdx.json)
  contains 668 components including the router and has SHA-256
  `1e0d45514b9501caf498b971461f4bda206ed6fb81fedd7c82b95107a3566281`.

Because the dependency closure is unchanged, the license, native-component,
notice, and vulnerability dispositions in the accepted
[0.1.0 source review](router-source-distribution-review.md) apply without a
dependency delta. In particular, the optional first-party `agql-auth` source
still lacks explicit license metadata, and the upstream-constrained
`quick-xml` and `rsa` findings remain subject to ADR-0008's existing excluded-
capability mitigations. This patch neither enables Hive object storage/XML nor
accepts a private key or private-key operation.

`cargo audit` rechecked all 950 locked packages. The six vulnerability records
are unchanged: the router closure reaches the two `quick-xml` records and the
`rsa` record described above, while the three `rustls-webpki` records are
outside it. The workspace also retains two unmaintained-package warnings
outside the router closure and two router-reachable yanked selections,
`spin` 0.9.8 and `unicode-segmentation` 1.13.1, through exact-pinned Hive
dependencies. The yanked selections have no vulnerability record in this
audit. They are accepted for this dependency-neutral source patch and must be
replaced through a separate, fully reviewed dependency update rather than an
unbounded lockfile change during the live migration fix.

## Conclusion

The designated project owner's continuing authorization to implement and
publish the router migration covers this reviewed source-only patch channel.
Publishing `graphql-orm-router` 0.1.1 from the public repository at the full
Git revision containing this evidence satisfies ADR-0008 for that revision and
channel only.

No binary, container, hosted deployment, or later dependency revision is
approved by this record. GEMA and every other deployment must retain its own
artifact-derived SBOM, notices, linked-component inventory, and designated
approval.

## Follow-up

- Validate the exact variable-backed subscription reproduction against the
  published full revision before destructive consumer cleanup.
- Repeat the complete dependency review when any dependency pin or lockfile
  selection changes.
- Continue the accepted `quick-xml`, `rsa`, and `agql-auth` license-metadata
  follow-ups from the 0.1.0 review.
