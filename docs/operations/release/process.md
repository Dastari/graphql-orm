---
title: Workspace release process
kind: runbook
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2026-11-01
supersedes: []
---

# Workspace release process

## Preconditions

- Work from a clean branch based on the intended release baseline.
- Identify every affected package and its direct workspace dependants.
- Keep package versions, changelogs, migration guidance, and examples aligned.
- Use a reviewed full Git revision for consumers; workspace packages are not
  published to crates.io.

## Procedure

1. Classify the public/API/schema effect and update each affected package’s
   `CHANGELOG.md` and `MIGRATION.md` as required by its local `AGENTS.md`.
2. Update package versions in their manifests. Do not hand-edit the generated
   workspace inventory; run:

   ```bash
   python3 scripts/generate-workspace-inventory.py
   ```

3. Run documentation and dependency checks:

   ```bash
   python3 scripts/check-documentation.py
   python3 scripts/generate-workspace-inventory.py --check
   scripts/check-workspace-dependencies.sh
   ```

4. Run `cargo fmt --all -- --check` and every package/backend/provider lane
   required by the root and package-local `AGENTS.md` files. Never use
   workspace `--all-features`; database backends are alternative profiles.
5. Run warnings-denied Clippy and Rustdoc for affected packages, plus SemVer
   checks when a public surface changed.
6. For a router-containing delivery, apply the distribution boundary in
   ADR-0008. Generate CycloneDX inventories from the exact router manifest,
   root lockfile, target, and explicit feature lane; review non-strict license
   metadata, MPL components, native/bundled components, advisories, and the
   actual files in the artifact. Retain the inventory, hashes, channel, and
   designated approval. A source review does not approve a binary or
   container.
7. Review `git diff`, generated manifests, dependency trees, migration text,
   and documentation links before committing.
8. Push the reviewed commit. A tag or publication is a separate explicit owner
   action.

## Failure and rollback

Do not release a partial version/dependency set. If a validation lane fails,
repair it on the branch and rerun affected lanes. If an already-consumed
revision is defective, create a new revision and document the migration; do
not rewrite published Git history.
