---
title: Workspace release process
kind: runbook
status: active
owner: workspace-maintainers
last_reviewed: 2026-10-19
review_by: 2026-11-11
supersedes: []
---

# Workspace release process

The repository uses independent package versions and immutable Git-only
workspace releases under [ADR-0013](../../decisions/ADR-0013-explicit-source-only-release-verification-profiles.md). It is a virtual Cargo workspace, not one Cargo package, so
there is deliberately no single SemVer value for the repository.

## Release identities

Three identities have different purposes:

- A **development snapshot** is one full 40-character commit SHA on `main`.
  It is not a release merely because it is reachable.
- A **package release** uses the package's own SemVer and a qualified tag such
  as `graphql-orm-ai-v0.73.0`. A package tag identifies a package version
  within a workspace release and never moves. Package tags are not pinning
  selectors.
- A **workspace release** is a tested package set named
  `workspace-YYYY.MM.DD.N`, for example `workspace-2026.08.11.1`. Its attached
  manifest binds the source SHA, lockfile, every package source tree and
  version, package tags, external Git revisions, and durable wire/schema
  contract versions.

Consumers pin the workspace tag, for example
`tag = "workspace-2026.08.11.1"`, as one Git reference for every package in
the workspace, and record the commit from that release's attached manifest in
their own reviewed pin record. A published workspace tag is annotated and is
never moved, so the tag and the recorded commit stay one identity; a consumer
lockfile that resolves the tag to a different commit is the detection point.

## Version policy

Packages advance independently because they have different public contracts
and release cadence. `graphql-orm` and `graphql-orm-macros` remain aligned
because generated code and runtime support are one compatibility boundary.

A package version changes when its public Rust API, generated code, Cargo
features, wire contract, runtime behavior, or documented compatibility changes
under that package's release rules. A workspace release does not require a new
version for an unchanged package.

`graphql-orm-ai` additionally versions its persistent schema module. Router,
semantic-catalogue, tool-manifest, and operation-assurance protocols keep their
own contract versions. The generated release manifest records these values
separately from package SemVer. Contract rows are unique and name-sorted so the
same source commit and release ID always produce byte-identical output.

The workspace remains deliberately unpublished on crates.io. Every member
sets `publish = false`, and `scripts/check-release-state.py` enforces that
boundary. Registry publication would be a separate distribution project, not
a side effect of this process.

## Reviewed compiler baseline

CI and workspace release lanes select Rust **1.97.1** explicitly. The independent
router MSRV lane remains Rust **1.90.0**. Floating `stable` previously selected
Rust 1.99.0 on hosted runners while local acceptance used 1.97.1, producing
non-comparable evidence and new Clippy diagnostics in `async-trait`-generated
`must_use` attributes. The selected compiler retains all warnings-denied gates;
no lint is disabled and no test/provider/backend lane is removed.

Changing this compiler baseline requires a separately reviewed build change and
full provider/backend validation before publication. Rust 1.99.0 ordinary ORM
and router test execution has passed, but companion Clippy compatibility remains
an open follow-up. Do not claim the pinned lane demonstrates current floating
stable Clippy support. Workflow configuration at the release source SHA records
the selected compiler; consumers may compile the Rust source themselves within
the documented package support contracts.

## Verification profile selection

`validation_profile=full` is the default. Its existing commands, provider matrix,
all-success/no-skips validator and immutable publisher remain unchanged. Full
profile execution requires authorization for its security, refusal, fault,
concurrency and mixed suites; an inspection-only approval does not permit it.

The only additional profile is
[`replay-scoped-positive-v1`](#replay-scoped-positive-v1), explicitly selected
with a publication approval reference. It is source-only and is eligible only
while exact qualified crate/dependency/configuration identities are unchanged.
It replaces the full-matrix acceptance requirement for that specific source
under ADR-0013; it does not report the full matrix as passed. The following
preparation/publication rules apply normally; the scoped section lists its
explicit command and audit-reuse substitutions.

## Prepare the release commit

1. Start from a clean branch based on current `main`.
2. Identify every changed package and direct workspace dependant.
3. Classify public API, GraphQL SDL, persistence, configuration, security,
   backup/restore, provider, and operational effects.
4. Update affected package versions, `CHANGELOG.md`, `MIGRATION.md`, README,
   and examples according to the package-local `AGENTS.md`.
5. Move completed work out of `docs/plans/active/`; an active plan must describe
   genuine remaining implementation rather than release chronology.
6. Regenerate the package inventory after manifest changes:

   ```bash
   python3 scripts/generate-workspace-inventory.py
   ```

7. Run the local release metadata gates:

   ```bash
   python3 scripts/check-documentation.py
   python3 scripts/generate-workspace-inventory.py --check
   python3 scripts/check-release-state.py
   scripts/check-workspace-dependencies.sh
   scripts/check-package-release-policy.sh <merge-base-or-reviewed-base-sha>
   scripts/check-semver.sh <merge-base-or-reviewed-base-sha>
   scripts/check-release-manifest.sh
   python3 scripts/test-router-notices.py
   cargo fmt --all -- --check
   ```

   The SemVer runner explicitly selects private packages and retains native
   per-package check counts. A successful tool exit without those records is
   not accepted. It preserves the tool's inferred release type: an intentionally
   breaking version increment can permit every lint and produce zero checks.
   Such a lane is reported separately as version-policy permission, never as
   API compatibility proof. For an independently approved additive-only gate,
   run an explicit package check with `--release-type minor --verbose` and
   require positive native check counts. Default features remain enabled;
   backend and provider compatibility still require their separate lanes. The
   hosted SemVer steps likewise explicitly select each package, enable verbose
   output and use Rust1.97.1. The pinned action still downloads the latest native
   tool unless one is already installed; retain its actual tool identity and
   native check output when assessing a hosted result.

8. Run every package, backend, provider, Clippy, Rustdoc, SemVer, migration,
   restore, and release-policy lane required by the root and package-local
   instructions. `scripts/check-ai-provider-lanes.sh` exercises each provider
   separately, and `scripts/run-owned-database-lanes.sh` supplies the disposable
   database evidence. Never use workspace `--all-features`; database backends
   are alternative profiles. Local command output is the acceptance evidence;
   hosted workflow success alone is insufficient.
9. Review the complete diff, dependency trees, generated schema/manifest
   changes, documentation links, and migration statements.
   Reconcile unsuppressed dependency scans with the
   [dependency security disposition](../../reference/dependency-security-disposition.md)
   for the exact release source, unified features and advisory snapshot. Preserve
   nonzero findings and changed applicability conditions; an earlier frozen scan
   or excluded call path does not establish a clean graph or runtime acceptance.
10. In the pull request, select exactly one documentation-impact option from
    the repository template. Release changes normally select
    `Documentation updated`; CI rejects missing or ambiguous declarations.
11. Merge and push the reviewed release commit to `main`. Do not tag it yet.

## Preview the release bill of materials

The generator is deterministic for one release ID and commit:

```bash
python3 scripts/generate-release-manifest.py \
  --release-id workspace-2026.08.11.1 \
  --ref 0123456789abcdef0123456789abcdef01234567 \
  --check-clean \
  --verify-tags \
  --output /tmp/workspace-release.json \
  --notes-output /tmp/workspace-release.md
```

`--verify-tags` permits an existing package tag only when that tag's package
source tree is byte-identical to the selected commit. It therefore catches a
package change that reused an already released version.

## Publish through GitHub Actions

Run **Workspace release** manually and supply:

- `release_id`: the new `workspace-YYYY.MM.DD.N` identity;
- `target_ref`: the full commit SHA, which must equal current `main`;
- `validation_profile`: `full` by default, or explicitly `replay-scoped-positive-v1`;
- `release_approval_reference`: required for scoped publication; the owner approval
  must bind full source SHA, release ID, profile and retained limitations;
- `prerelease`: whether the workspace release is a candidate;
- `include_router_artifact`: normally false for source-only releases; and
- `router_distribution_approval`: required when a router binary is attached.

Configure `Dastari` as a required human reviewer on the `release` environment
and restrict it to `main` before dispatch. The guard rejects an environment
without that reviewer; naming an environment in YAML does not create protection.
The protected `release` environment is the human authorization boundary and
gates the workflow's entry job, so no release lane runs before approval. The
workflow then:

1. proves the requested commit is current `main` and the workspace tag is new;
2. for `full`, reruns documentation, dependency, package, backend, provider, Clippy, and
   Rustdoc release lanes with the lockfile fixed, as parallel jobs that each
   own their `target/`, one per AI provider lane, all of which must succeed
   before anything is tagged or published; the explicitly scoped profile instead
   requires its own complete mandatory gates and attaches their evidence;
3. generates the canonical JSON manifest and Markdown release notes;
4. optionally builds the approved Linux router executable and its CycloneDX
   inventory;
5. hashes and attests every release asset;
6. creates package-qualified annotated tags only for versions not already
   tagged, plus the annotated workspace tag, in one atomic push; and
7. publishes the GitHub Release from the existing workspace tag.

Enable GitHub immutable releases for the repository. Prepare every required
asset before publication because neither a release tag nor an attached asset
may be replaced after publication.

## replay-scoped-positive-v1

This profile is the bounded source-only policy for prepared ORM/macros 0.52.0,
qualified at `1e49f1c63728865b294795684e38a27e48258b5a`. Policy changes do not
advance package versions. No ORM algorithm, dependency, fixture, recipe, format
or consumer changes are included. Policy approval, policy-PR merge and actual
release publication are separate decisions. Do not dispatch until publication
is explicitly approved for the final current-main source and release identity.

### Mandatory jobs and literal commands

The protected `guard` binds `GITHUB_REF=refs/heads/main`, workflow `GITHUB_SHA`,
checked-out `target_ref` and current remote main. It checks unused workspace tag,
Dastari required reviewer, explicit scoped selection, nonempty publication
approval reference and `include_router_artifact=false`, then checks source reuse
before any compiler/backend job runs. It never silently selects a recipe.

- `replay-scoped-integrity`: source/evidence reconciliation plus eight positive
  metadata commands; upload the exact records/logs.
- `replay-scoped-positive`: both exact compiler lanes, each running 38 literal
  commands and **16 selected positive tests**. These include ten feature-off/on
  SQLite/PostgreSQL/MSSQL/coexistence builds, six library Clippy and six Rustdoc
  gates with warnings denied, six external API test profiles, six dependency
  trees (three exact off/on comparisons), two live/offline positive replay
  selectors and an MSSQL library unit-target compile without execution.
- `replay-scoped-validate`: require guard/integrity/matrix success, both complete
  compiler records, every expected command/count and all log digests. No absent,
  skipped, cancelled or failed mandatory scoped job can publish.
- `release-scoped`: depends only on successful guard and scoped validator; it
  revalidates the evidence and invokes the checksummed/attested immutable
  publisher. The existing full validator and full publisher remain separate.

The complete, literal command arrays and per-command environment/expected counts
are [replay-positive-commands.json](replay-positive-commands.json). The runner
executes its reviewed Python literals, never commands read from an archive,
caller string or JSON record. The positive conformance checker requires the
reference to match those literals. There are no broad test aliases or workspace
`--all-features`. The exact entry commands are:

```bash
python3 scripts/run-replay-release-positive.py metadata --output target/release-evidence/replay-scoped-integrity
python3 scripts/run-replay-release-positive.py 1.97.1 --output target/release-evidence/replay-scoped-1.97.1
python3 scripts/run-replay-release-positive.py 1.98.1 --output target/release-evidence/replay-scoped-1.98.1
```

Use fresh output directories; never overwrite a previous record. The two
compiler lanes record and require the exact stock release/hash and GNU x86_64
host, set that same build target, and require no wrappers/custom compiler flags.
Historical recipe `graphql-orm-owned-planning-8e1-v1` is only Rust 1.97.1 /
`8bab26f4f68e0e26f0bb7960be334d5b520ea452`; native recipe
`graphql-orm-owned-planning-8e1-rust1981-v1` is only Rust 1.98.1 /
`48a229ceaefd4985c50990b14116b6d856af0985`. Library admission remains authoritative.
Clippy wrapper compilation grants no runtime admission. The global companion
release compiler baseline is unchanged. No cross-compiler historical equivalence
is claimed. Replay stays default-off with explicit capture/custody consent.

The selected positive tests own in-memory SQLite and disposable PostgreSQL,
prove database identity before use and container identity/ownership before
cleanup, close pools and verify retained captures offline afterward. Never set
an application DATABASE_URL or use live infrastructure. Release all test-owned
resources. This profile does not execute MSSQL capture/refusal or live MSSQL.

### Source-qualified evidence reuse and dependency findings

`scripts/replay_release.py reuse` requires the selected full source to be HEAD
and a descendant of the qualification source. It compares the entire `crates`
Git tree, root Cargo manifest/lock, release configuration, existing compiler/
Cargo configuration and original source checker as exact identities. Source
working edits are rejected. Only the enumerated policy scripts, release
workflow, docs and root changelog/migration changes are eligible. Crate trees
include both fixture sets, build admission, manifests and vendor code.

The [archived evidence](../../archive/2026/replay-0.52.0-qualification/README.md)
contains 109 exact public qualification records plus the original 154-file
checksum list. The other 45 private operational/handoff/PR records are listed
as omitted; no claim is made to publish their bytes. Archive/member digests,
original source qualifiers, 82 successful development commands, final replay
runs, additive SemVer **196 pass / 57 skip** off/on and the retained exit-101
MSSQL supplementary compile are checked. The separate **0 / 253** repository
version-policy count remains permission, not compatibility proof. Reuse never
changes a record's source or represents it as fresh execution at a policy SHA.

This profile substitutes reconciliation/reuse of the retained UNSUPPRESSED
cargo-audit 0.22.2 exit-1 snapshot for a new scan. The scanned lock and provenance
are exact; normalizing only the aligned internal ORM/macros versions must leave
all lock records equal. Released feature/dependency tables, companion/vendor
source trees, applicability-condition sources and the conditional disposition
must also match. Original finding rows, advisory commit/time and coverage limits
remain attached. The release record computes actual advisory-snapshot age.
New advisories and current graph safety are unassessed. A condition/graph/source
change blocks this reuse and needs new review; do not run a scan without its
own authorization or suppress findings to satisfy a gate.

### Exclusions and release evidence

The full-profile jobs remain unexecuted under scoped selection. Their names are
listed individually as `unexecuted` in verification metadata; they are not
reported as passed. The full release-manifest/router-notice dispatchers and
security/refusal/malformed/resource-exhaustion/negative-auth/fault/cancellation/
security-concurrency/fuzz/penetration scenarios remain inspection-only and
runtime-unverified. Positive tests do not prove those properties.

The known MSSQL all-integration-target Clippy compile failure remains exit 101
in unchanged `tests/contract_gaps.rs`; passing library/API/unit-target profiles
are separate facts. Retain audit exit 1 and quick-xml, rsa, lru, atomic-polyfill
and unsafe-libyaml findings, applicability conditions and path/Git/vendor SMB
coverage gaps. A scoped release is not full-workspace runtime acceptance.

The publisher attaches `<release-id>-verification.json`, exact qualification
and audit archives, and an archive of all fresh selected logs/records. The
verification record binds source, profile, publication approval reference,
workflow run/attempt, original producer/qualification identities, protected
source trees, actual command arrays/counts/skips/failures, snapshot age and all
unexecuted obligations. The canonical manifest adds an optional `verification`
reference/digest only for this profile; default full manifest/notes remain
unchanged. This is release metadata, not a replay witness/receipt format change.
All assets, including evidence, are covered by SHA256SUMS and build provenance.

### Concrete publication after final review

1. Merge the reviewed policy PR only after owner approval. Fetch current main;
   retain its exact full SHA. Reconcile evidence and rerun the selected positive
   gates there; original evidence remains qualified to its original source.
2. Select an unused `workspace-YYYY.MM.DD.N` and review package tags/tree identity,
   prepared aligned 0.52.0 versions, limitations and source-only delivery.
   Obtain a separate publication approval reference binding that SHA, release
   ID, `replay-scoped-positive-v1`, retained findings and unexecuted obligations.
3. Verify immutable releases and main-only/Dastari protected environment, then
   dispatch **Workspace release** on main with that full `target_ref`, explicit
   scoped profile/reference and `include_router_artifact=false`. Do not dispatch
   `full` under an inspection-only approval. The human release environment
   approval gates entry before any validation lane.
4. Every mandatory scoped gate must pass. The scoped publisher checks current
   main again before creating tags. It generates the canonical manifest/notes,
   checksummed evidence and attestations, then atomically pushes annotated
   package/workspace tags and publishes through the normal GitHub release path.
   A failure stops publication; never change gates or publish manually around it.
5. Download and retain all immutable assets, verify SHA256SUMS, package/workspace
   tag/source/manifest identities and source-pinned attestations. Record exact
   release URL, source and artifact/provenance hashes. No consumer/pin update,
   deployment, historical receipt adoption or recipe relabeling follows.

## Router artifact boundary

Source and compiled-router distribution are distinct approvals. A router
binary may be selected only after the exact target, features, lockfile,
linked/native components, advisories, licenses, notices, SBOM, hashes, and
delivery channel have a designated approval under ADR-0008.

The binary lane is therefore opt-in and requires an evidence reference. It
builds the explicit `auth-agql` feature profile for
`x86_64-unknown-linux-gnu`, packages the binary with the workspace license,
CycloneDX inventory, third-party notice/source evidence, and approval reference,
and includes the archive in the release checksums and provenance attestation.
A later target or feature set requires its own approval.

`scripts/generate-router-notices.py` collects packaged and nested/native notice
files and supplemental upstream notices whose URLs and SHA-256 values were
reviewed at recorded source commits. It also includes exact registry source
archives for MPL components and the explicitly recorded packages whose
standalone notice files were unavailable. The archives must match Cargo.lock
checksums. The generated inventory preserves original license expressions,
source URLs, file hashes, and source-only notice dispositions; it contains no
builder-local source paths. The bundle also retains the matching Rust standard-library
copyright notices shipped with the matching Rust compiler installation.

`config/router-notice-review.v1.json` binds these notice-source exceptions to
the lockfile hash, SBOM component set, target, and features. A dependency/profile
change requires a fresh review of those bindings and supplemental sources;
generation fails if the existing record does not match. This configuration is
technical evidence, not distribution approval. The designated owner must
review source-only notice dispositions, legacy license expressions, applicable
source obligations, and the actual delivery contents before providing the
workflow's `router_distribution_approval` reference. Do not treat a missing
notice as an automatically approved exception.

External `agql-auth` dependencies may select a published `v<version>` release tag.
The manifest keeps its full locked commit in `externalGitDependencies[].revision`
and additionally records `tag`; all other external Git dependencies still require
full revision pins. Generation fails if the lockfile has no unique matching source
or disagrees with an explicit revision. Verify the auth release's attested manifest
and peeled tag before adopting it. Consumers seeking one Cargo source must use the
same tag selector: a `rev` and a tag remain different sources even at the same SHA.

Pure Rust libraries do not receive optimized binary artifacts. Downstream
Cargo builds compile them from the pinned Git source.

## Failure and rollback

- Before tags are pushed, repair the release commit and rerun the workflow.
- If validation fails, do not publish a partial package/dependency set.
- Package and workspace tags are immutable. Never force-push, move, reuse, or
  delete a published release identity.
- If tagged source is defective, make a new commit, advance every affected
  package version, and publish a new workspace release.
- If tag creation succeeds but GitHub Release publication fails, retain the
  immutable tags, inspect the failed run, and attach the already attested
  assets to a release for that exact tag. Do not regenerate from another SHA.
- Consumers roll forward to a newly reviewed workspace tag and its recorded
  commit; published Git history is never rewritten.
