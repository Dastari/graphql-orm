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
Git worktree, removed on success or failure. At each ref Cargo creates an archive
with `cargo package -p <package> --allow-dirty --no-verify --exclude-lockfile --offline`.
A temporary target directory isolates each archive. No build or dependency
resolution is needed. Archive paths, file contents, and permission modes are
compared, covering additions, removals, include/exclude changes, executable bits,
and published files outside `src/*.rs`.

The archive's `Cargo.toml.orig` preserves authored manifest bytes. The generated
`Cargo.toml` is parsed as TOML for stable semantic comparison, retaining every
field Cargo emits, including inherited lints, package metadata, dependencies,
features, and generated target/build settings. `cargo read-manifest` is used only
for package identity and mapping dependencies to actual workspace members; it is
not the effective-manifest comparison. This avoids its omissions, such as lints.
Pending the maintainer decision in [#435](https://github.com/Dastari/graphql-orm/issues/435),
option B exempts only the generated version requirement (`version`) of a dependency
whose name and path identify an actual workspace member among the nine workspace
packages. Registry/Git dependencies with the same name are not exempt. Sibling
names, features, optional/default-feature flags, dependency kind, target,
and all other published fields remain strict, as do all external requirements.
The exemption maps renamed and target-specific normal, build, and dev dependencies
to the corresponding generated manifest table. Authored path changes remain strict.
Authored manifest bytes remain strict, preserving the existing local manifest
gate; this exemption applies only to the additional effective-manifest comparison.

`WORKSPACE_SIBLING_REQUIREMENT_DRIFT = "exempt"` in `scripts/published_source.py`
is the one-line option B switch. Changing its value to `"strict"` selects option A
and requires reconciling existing inherited sibling requirement drift before
release. Option B treats that requirement drift as release-bundle metadata and
allows current main to release without catch-up package bumps.
Published README, license, build scripts, and data are ordinary archive files and
require version changes, including README/license files copied from outside the
package directory.

Cargo copies the authored manifest to `Cargo.toml.orig` and generates `Cargo.toml`,
`.cargo_vcs_info.json`, and normally `Cargo.lock`. Both manifests are compared;
only generated Git provenance and dependency resolution are excluded from package
source equality. `--exclude-lockfile` prevents lockfile generation.
The workspace release manifest separately binds the full source commit, root
lockfile hash, and external Git dependency revisions. Authored nested lockfiles
are compared if Cargo archives them; excluded fixture lockfiles are not.

Cargo follows source symlinks and archives their destination bytes as regular
files. The snapshot compares those bytes and modes, including destinations
outside the package directory. Retargeting to identical bytes/modes does not
change the archive and is intentionally allowed. Unexpected non-regular archive
entries fail closed. Cargo fixes tar owner/group and timestamps; those transport
headers, gzip metadata, archive ordering, generated manifest comments/formatting,
and the package-version directory prefix are not source identity. Authored
formatting remains covered by `Cargo.toml.orig`. No other generated manifest
fields or archived files are filtered beyond the explicit exceptions above.

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
