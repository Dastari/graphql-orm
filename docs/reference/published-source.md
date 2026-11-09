---
title: Published package source comparison
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-09
review_by: 2027-02-01
supersedes: []
---

# Published package source comparison

`scripts/published_source.py` owns the comparison used by
`generate-release-manifest.py --verify-tags` and
`check-package-release-policy.sh`. Each ref is checked out in a temporary detached
Git worktree, removed on success or failure. Cargo selects files with
`cargo package --list -p <package> --allow-dirty --no-verify`. Selection and file
contents are compared at both refs, so additions, removals, include/exclude
changes, and changes outside `src/*.rs` cannot escape the gate.

Authored `Cargo.toml` is compared verbatim. `cargo read-manifest` additionally
compares effective workspace inheritance, including dependency requirements;
checkout paths are normalized and targets absent from the package are omitted.
Pending the maintainer decision in [#435](https://github.com/Dastari/graphql-orm/issues/435),
option B exempts only the effective version requirement (`req`) of a dependency
whose name and path identify an actual workspace member among the nine workspace
packages. Registry/Git dependencies with the same name are not exempt. Sibling
names, features, optional/default-feature flags, dependency kind, target, paths,
and all other fields remain strict, as do all external dependency requirements.
Authored manifest bytes remain strict, preserving the existing local manifest
gate; this exemption applies only to the additional effective-manifest comparison.

`WORKSPACE_SIBLING_REQUIREMENT_DRIFT = "exempt"` in `scripts/published_source.py`
is the one-line option B switch. Changing its value to `"strict"` selects option A
and requires reconciling existing inherited sibling requirement drift before
release. Option B treats that requirement drift as release-bundle metadata and
allows current main to release without catch-up package bumps.
This does not resolve or build dependencies. Published README, license, build
scripts, and data are ordinary source files and require version changes.

Cargo synthesizes `Cargo.toml.orig`, `.cargo_vcs_info.json`, and `Cargo.lock`.
The original manifest is already compared as `Cargo.toml`; generated Git
provenance and dependency resolution are excluded from package source equality.
The workspace release manifest separately binds the full source commit, root
lockfile hash, and external Git dependency revisions. Authored nested lockfiles
are compared if Cargo lists them; excluded fixture lockfiles are not.

Current workspace manifests do not exclude all tests, examples, or benches.
Cargo includes ordinary test files while excluding nested fixture packages.
We deliberately retain strict comparison for included tests rather than silently
filtering published content. A general test-only exemption requires a separate
review of package include/exclude rules; this script change does not alter
package manifests. The regression suite checks both excluded and included tests.

The PR policy additionally retains the storage-owned nested SMB source and
manifest gate because that Git-distributed dependency is excluded by Cargo's
nested-package rule. Owner documentation and SemVer alignment requirements
continue to apply. The AI schema and router-specific checks remain additional
gates, not replacements for published-source equality.

Run `python3 scripts/test-published-source.py`,
`python3 scripts/test-package-release-vendor.py`, and
`python3 scripts/test-release-manifest.py` for script regression coverage.
See the [release process](../operations/release/process.md) for publication gates.
