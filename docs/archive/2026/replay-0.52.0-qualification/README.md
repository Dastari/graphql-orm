---
title: Replay 0.52.0 source-qualified release evidence
kind: reference
status: archived
owner: workspace-maintainers
last_reviewed: 2026-10-19
review_by: none
supersedes: []
---

# Replay 0.52.0 source-qualified release evidence

This immutable evidence supports the explicitly scoped source-release policy
in [ADR-0013](../../../decisions/ADR-0013-explicit-source-only-release-verification-profiles.md).
It is not a full release-matrix result, a new scan or publication authorization.

[provenance.json](provenance.json) binds both archives. `qualification.tar.gz`
contains an exact public subset of **109** original checksummed files and the
original `SHA256SUMS` covering **154** files. Operational checkout/container,
local handoff and PR records are deliberately omitted; their names are listed
in provenance, not represented as published bytes. The complete original packet
remains in private owner custody. Every published original file is checked
against that original checksum record, whose SHA-256 is
`0fdb2d7e484a54a446286a87620bb75cac12fac38e4ae63174a487bf307ce6b0`.
The original producer source, instrumented observations and fixtures remain
committed beside the ORM fixtures; no fixture is changed or resealed here.

Qualification source is `1e49f1c63728865b294795684e38a27e48258b5a`.
The 82-command development matrix spans `de484e6` and its final test-only lint
correction. Affected lint checks, four final positive runs and MSSQL unit-target
compiles record the exact final source. Twelve authentic historical fixtures
run only on qualified 1.97.1; sixteen separate native fixtures run only on
qualified 1.98.1. Both include valid kind-last decoding, both backends/scope
options, full unowned metadata and compound One/Restrict FK facts. The original
successful U+3164 cross-compiler mismatch remains evidence for separate recipes.
Additive SemVer off/on retain 196 pass / 57 skip each; the separate repository
version-policy run retains 0 checks / 253 skips, not compatibility proof.

`audit.tar.gz` retains the original unsuppressed audit JSON, provenance,
reconciliation, exact scanned lock and conditional dependency disposition from
published baseline `62fe2ddde77c49092fb70644af37a92e8fb31f06`.
Cargo-audit 0.22.2 exited **1** at advisory commit
`999ec89d5d3f10f20be14750ec8c2d8b44e0ac9b`, recorded
2026-10-18T20:13:53.859199+00:00. No new audit/refresh is performed. Findings:

- quick-xml 0.39.4: RUSTSEC-2026-0194 and RUSTSEC-2026-0195;
- rsa 0.9.10: RUSTSEC-2023-0071;
- lru 0.16.3: RUSTSEC-2026-0253 (unsound);
- atomic-polyfill 1.0.3: RUSTSEC-2023-0089 (unmaintained);
- unsafe-libyaml 0.2.11: RUSTSEC-2024-0449 (unmaintained).

The [canonical disposition](../../../reference/dependency-security-disposition.md)
retains path/feature conditions and ordinary cargo-audit's path/Git/vendor SMB
coverage limits. Dependency identity equality does not establish a current
advisory set, clean graph, consumer safety or runtime acceptance. Publication
records the snapshot's actual age and these gaps.

Supplementary MSSQL all-integration-target Clippy exited **101** in unchanged
`tests/contract_gaps.rs`, whose SQL-only helpers/CRUD are absent under MSSQL-only.
No runtime tests executed in that failed compile. Required MSSQL library/API/
Clippy/Rustdoc/coexistence and separate unit-target no-run compiles passed.
This distinction is retained, not repaired or reclassified by the policy.

Wrong-recipe/profile/unsupported/malformed/resource refusals, disclosure,
failed-live-check/authority scenarios, negative auth, fault/cancellation,
security concurrency, fuzz/property/penetration and broad mixed dispatchers
remain code-inspection-only and runtime-unverified. Fresh positive release
results are separate assets and cannot turn these gaps into passed tests.
