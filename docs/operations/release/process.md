---
title: Workspace release process
kind: runbook
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-13
review_by: 2026-11-11
supersedes: []
---

# Workspace release process

The repository uses independent package versions and immutable Git-only
workspace releases. It is a virtual Cargo workspace, not one Cargo package, so
there is deliberately no single SemVer value for the repository.

## Release identities

Three identities have different purposes:

- A **development snapshot** is one full 40-character commit SHA on `main`.
  It is not a release merely because it is reachable.
- A **package release** uses the package's own SemVer and a qualified tag such
  as `graphql-orm-ai-v0.73.0`. A package tag never moves.
- A **workspace release** is a tested package set named
  `workspace-YYYY.MM.DD.N`, for example `workspace-2026.08.11.1`. Its attached
  manifest binds the source SHA, lockfile, every package source tree and
  version, package tags, external Git revisions, and durable wire/schema
  contract versions.

Consumers must use the workspace release's full commit SHA in Cargo `rev`.
Tags improve discovery, comparison, and support but are not authority for a
dependency update.

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
   cargo fmt --all -- --check
   ```

8. Run every package, backend, provider, Clippy, Rustdoc, SemVer, migration,
   restore, and release-policy lane required by the root and package-local
   instructions. `scripts/check-ai-provider-lanes.sh` exercises each provider
   separately, and `scripts/run-owned-database-lanes.sh` supplies the disposable
   database evidence. Never use workspace `--all-features`; database backends
   are alternative profiles. Local command output is the acceptance evidence;
   hosted workflow success alone is insufficient.
9. Review the complete diff, dependency trees, generated schema/manifest
   changes, documentation links, and migration statements.
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
- `prerelease`: whether the workspace release is a candidate;
- `include_router_artifact`: normally false for source-only releases; and
- `router_distribution_approval`: required when a router binary is attached.

The protected `release` environment is the human authorization boundary and
gates the workflow's entry job, so no release lane runs before approval. The
workflow then:

1. proves the requested commit is current `main` and the workspace tag is new;
2. reruns documentation, dependency, package, backend, provider, Clippy, and
   Rustdoc release lanes with the lockfile fixed, as parallel jobs that each
   own their `target/`, one per AI provider lane, all of which must succeed
   before anything is tagged or published;
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

## Router artifact boundary

Source and compiled-router distribution are distinct approvals. A router
binary may be selected only after the exact target, features, lockfile,
linked/native components, advisories, licenses, notices, SBOM, hashes, and
delivery channel have a designated approval under ADR-0008.

The binary lane is therefore opt-in and requires an evidence reference. It
builds the explicit `auth-agql` feature profile for
`x86_64-unknown-linux-gnu`, packages the binary with the workspace license,
CycloneDX inventory, and approval reference, and includes the archive in the
release checksums and provenance attestation. A later target or feature set
requires its own approval.

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
- Consumers roll forward to a newly reviewed full SHA; published Git history
  is never rewritten.
