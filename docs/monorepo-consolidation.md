# GraphQL ORM monorepo consolidation

Status: consolidation implementation in progress

Scope: `graphql-orm`, `graphql-orm-ai`, `graphql-orm-backup`, and
`graphql-orm-storage`

Out of scope: `agql-auth`, which remains an external dependency

This document is both the consolidation plan and the migration guide for
projects that consume these crates. It should be updated with the final
monorepo commit and crate versions before the old repositories are archived.

## Implementation record

The local consolidation candidate uses these immutable source checkpoints:

| Package | Old-repository source commit | Filtered history tip |
| --- | --- | --- |
| `graphql-orm` / macros | `dd68a001f47f04178bf3389dd47ee952faa6ecf0` | Existing monorepo history |
| `graphql-orm-storage` 0.6.0 | `05786fdd7ec20397bcd9f6665cee5b5f5b076703` | `0fa2115c2c2a33170640af1db5b1e9cf63eba19f` |
| `graphql-orm-backup` 0.6.0 | `6a9ccedd76fd140c351c8861de72c4cb7c99feea` | `82c1f26a5b65cf8303322c2cb05116dc9a2bafea` |
| `graphql-orm-ai` 0.57.0 | `d35e3d68d86e77d9aedb62b64842fc9a5f2701f3` | `bd36fd5b5a14d833e3b164da0dc466bdf47ed3e3` |

Every tracked file was byte-compared with its source checkpoint immediately
after import. The candidate then converted internal dependencies to workspace
paths, aligned backup and AI with storage 0.6.0, and generated one root
lockfile.

Local validation passed for:

- workspace formatting and dependency-source identity;
- complete default tests for ORM, storage, and backup with SQLite conformance;
- the AI provider/local-harness matrix and PascalCase contract;
- PostgreSQL and MSSQL compile lanes;
- storage S3/Azure-placeholder and native SMB compile lanes;
- warnings-denied Clippy and Rustdoc for all packages;
- public API SemVer comparison against all three imported source tips;
- AI's test-owned PostgreSQL behavioral parity; and
- the managed Samba matrix, including encrypted/constrained connections,
  backup lifecycle and locking, cancellation, and reconnect.

The first supported consumer baseline remains the reviewed merge commit, not
any intermediate commit on the consolidation branch.

## Decision

Consolidate the four repositories into one Cargo workspace while preserving
their crate boundaries and independent versions.

Do **not** make `graphql-orm-ai`, `graphql-orm-backup`, or
`graphql-orm-storage` features of the `graphql-orm` crate.

Cargo features select capabilities and optional dependencies within one
package. They do not gate workspace members. Making the sibling crates
optional dependencies of `graphql-orm` would also reverse the intended
dependency direction, make feature unification harder to reason about, and
risk dependency cycles as the crates evolve.

Instead:

- Keep every crate as an independently selectable workspace package.
- Keep backend and provider features on the crate that implements them.
- Use `workspace.default-members` to keep ordinary root commands focused on a
  useful, lightweight set of packages.
- Use explicit `-p` and feature selections in CI; do not use
  `--all-features`.
- Let applications depend directly on the crates they use.

If applications later demonstrate a real need for a single suite dependency,
add a small facade crate such as `graphql-orm-suite`. It may offer `ai`,
`backup`, and `storage` features and re-export those crates. The facade must
depend downwards on the existing crates; `graphql-orm` must not depend upwards
on the facade or its siblings.

## Target layout and dependency direction

```text
graphql-orm/
├── Cargo.toml
├── Cargo.lock
├── AGENTS.md
├── crates/
│   ├── graphql-orm/
│   ├── graphql-orm-macros/
│   ├── graphql-orm-storage/
│   ├── graphql-orm-backup/
│   └── graphql-orm-ai/
└── docs/
```

The intended internal dependency graph is:

```text
graphql-orm-ai ────────> graphql-orm
       │
       ├───────────────> graphql-orm-backup ─────> graphql-orm
       │                         │
       └─────────────────────────┴───────────────> graphql-orm-storage

graphql-orm ───────────> graphql-orm-macros

graphql-orm and graphql-orm-ai ──────────────────> agql-auth (external)
```

`graphql-orm-storage` remains usable without the ORM. The ORM dependency in
`graphql-orm-backup` remains optional. Any change that introduces a cycle in
this graph must be redesigned rather than hidden behind a feature.

## Phase 0: stop work and establish cutover inputs

All agents working in the source repositories should stop at a durable
handoff point. A clean worktree alone is not enough: the cutover owner also
needs to know whether each non-`main` branch should be merged, retained for
later, or closed.

Each agent should:

1. Stop accepting new scope.
2. Finish the smallest coherent unit already in progress.
3. Run the repository's documented formatting and test checks.
4. Commit and push coherent work to its existing branch.
5. Leave no modified, staged, or untracked files.
6. Do not use a stash as the handoff mechanism.
7. Do not discard uncertain or user-owned changes to make the tree clean.
   Report them instead.
8. Record the branch, full commit SHA, upstream, version, checks, and the
   required branch disposition using the template below.

Useful evidence commands:

```bash
git status --short
git branch --show-current
git rev-parse HEAD
git rev-parse --abbrev-ref '@{upstream}'
git rev-list --left-right --count main...HEAD
git log -1 --format='%H %cI %s'
```

The first command must produce no output for a ready repository. Agents should
use the checks documented by their own repository rather than assuming one
test command is valid for every backend and provider.

### Agent handoff template

```text
Repository:
Working directory:
Branch:
HEAD (full SHA):
Upstream:
Ahead/behind main:
git status --short: empty
Package version:
Cargo.lock committed and current: yes/no
Checks run and results:
Open PR or review:
Unresolved work or risks:
Branch disposition: merge to main / retain for later / close
Recommended cutover commit:
```

The cutover owner must reject a handoff that has an unexplained dirty tree,
unpushed commits, a missing branch disposition, or a failing required check.

### Observed pre-freeze state

This was observed on 2026-07-29 and is diagnostic only. Re-run the evidence
commands after all agents have stopped.

| Repository | Observed branch and state | Cutover action |
| --- | --- | --- |
| `graphql-orm` | `main` at `dd68a001f47f04178bf3389dd47ee952faa6ecf0`, clean | Revalidate and use as the monorepo base |
| `graphql-orm-backup` | `main` at `6a9ccedd76fd140c351c8861de72c4cb7c99feea`, clean | Revalidate after the storage decision |
| `graphql-orm-storage` | `main` at `05786fdd7ec20397bcd9f6665cee5b5f5b076703`, clean and synchronized with `origin/main`; version `0.6.0` | Branch disposition resolved by [PR #1](https://github.com/Dastari/graphql-orm-storage/pull/1); imported as the storage source baseline |
| `graphql-orm-ai` | `main` at `d35e3d68d86e77d9aedb62b64842fc9a5f2701f3`, clean and synchronized with `origin/main`; crate/schema versions `0.57.0`/`0.51.0` | Tested pause checkpoint fast-forwarded to old-repository `main` and imported as the AI source baseline |

Storage PR #1 rebased the hardening work onto `main`; the resulting tree
exactly matches hardening commit
`10b63083467f1877a88b7266af564c88c596105e`. All observed source branch
dispositions are resolved.

The AI checkpoint reports passing tests, Clippy, Rustdoc, PascalCase, SemVer,
package review, alternate-backend checks, and owned PostgreSQL parity. It pins
`graphql-orm-backup` 0.6.0 at
`6a9ccedd76fd140c351c8861de72c4cb7c99feea`. These results make the commit a
credible cutover candidate; it was promoted to old-repository `main` before
history import.

## Phase 1: choose and record final source commits

For every repository:

1. Resolve the Phase 0 branch disposition.
2. Merge accepted work to that repository's `main` using its normal policy.
3. Update crate version and repository documentation if accepted work requires
   it.
4. Regenerate and commit its lockfile before import.
5. Run its required checks from the final `main` commit.
6. Record the full final SHA in a cutover manifest or issue.
7. Push `main` and verify the remote SHA.

These final SHAs are immutable migration inputs. Do not continue feature work
in the old repositories after they have been recorded.

## Phase 2: import history into the monorepo

Perform the migration on a dedicated branch from the final `graphql-orm`
`main`. Use temporary clones so the source repositories remain untouched.

For each of AI, backup, and storage:

1. Clone the repository into a temporary directory.
2. Check out the recorded final source commit.
3. Rewrite the temporary clone with `git filter-repo
   --to-subdirectory-filter crates/<crate-name>`.
4. Fetch the rewritten branch into the monorepo.
5. Merge it with `--allow-unrelated-histories`.
6. Verify that its history is visible under its new path with
   `git log --follow`.

Example shape, with paths and remote names adjusted for each crate:

```bash
git clone https://github.com/Dastari/graphql-orm-storage.git <temporary-path>
git -C <temporary-path> checkout <recorded-full-sha>
git -C <temporary-path> filter-repo \
  --to-subdirectory-filter crates/graphql-orm-storage

git remote add import-storage <temporary-path>
git fetch import-storage
git merge --no-ff --allow-unrelated-histories import-storage/main
git remote remove import-storage
```

The rewritten branch name may differ if the recorded commit was placed on a
temporary import branch. Inspect it rather than assuming `main`.

Import source history, not the old repository's GitHub configuration. Existing
issues, pull requests, releases, and branch protections do not move with Git
history. Record links to material issues and PRs in the relevant monorepo issue
or documentation. Leave old tags available in the archived repositories;
future monorepo tags use unambiguous crate-prefixed names.

## Phase 3: convert the imports to workspace members

Add the three packages to `workspace.members`. Choose
`workspace.default-members` deliberately; default membership is a developer
ergonomics and build-cost decision, not a dependency boundary.

Define internal crates once in `[workspace.dependencies]` with both a path and
the compatible package version. Member manifests then inherit those
dependencies:

```toml
[workspace.dependencies]
graphql-orm = {
  path = "crates/graphql-orm",
  version = "0.16.0",
  default-features = false,
}
graphql-orm-storage = {
  path = "crates/graphql-orm-storage",
  version = "0.6.0",
  default-features = false,
}
graphql-orm-backup = {
  path = "crates/graphql-orm-backup",
  version = "0.7.0",
  default-features = false,
}
```

These are the selected consolidation-candidate versions. Storage keeps 0.6.0;
backup advances to 0.7.0 because its public API now resolves storage types
through the monorepo source; AI advances to 0.58.0 because its backup
dependency crosses the same source boundary.

Then:

- Replace internal Git dependencies with `{ workspace = true }`, preserving
  `optional = true` and feature forwarding where needed.
- Keep `agql-auth` as one exact, full-revision external workspace dependency.
- Keep `resolver = "3"`.
- Remove member `Cargo.lock` files and generate one root `Cargo.lock`.
- Update `repository`, `homepage`, README links, badges, and source links to
  `https://github.com/Dastari/graphql-orm`.
- Preserve crate-local `AGENTS.md` instructions that describe security,
  storage, backup, or provider invariants.
- Add root instructions describing the dependency graph, shared commands, and
  rules for cross-crate changes.
- Do not combine crate APIs, rename packages, or change public behavior during
  the repository move.

Keeping the import mechanical makes regressions attributable. API cleanup and
crate reorganization should happen in later changes.

## Phase 4: workspace validation and CI

Do not use `cargo test --all-features`. The ORM database backends are
alternative configurations, and some provider integrations require distinct
environments.

The migration branch should have explicit lanes for:

- formatting for the workspace;
- `graphql-orm` and macros with the default SQLite configuration;
- ORM PostgreSQL and MSSQL configurations in separate lanes;
- storage local tests;
- storage S3 and SMB compile or integration checks, with credentials and
  managed infrastructure isolated as appropriate;
- backup local tests and separate SQLite/PostgreSQL ORM conformance checks;
- AI SQLite, PostgreSQL, and MSSQL configurations in separate lanes;
- AI provider feature checks in the combinations the AI crate documents;
- at least one integrated default SQLite lane spanning ORM, storage, backup,
  and AI;
- Clippy and rustdoc using deliberate package/feature selections;
- dependency-source checks that reject old internal Git URLs.

At minimum, validate dependency identity with:

```bash
cargo metadata --format-version 1
cargo tree -d
cargo tree -i graphql-orm
cargo tree -i graphql-orm-storage
```

There should be one workspace source for each internal package and no
Git-sourced second copy of an internal crate. Review every duplicate reported
by `cargo tree -d`; not every third-party duplicate is an error.

The migration is not ready to merge until all required lanes pass from one
candidate commit and the root lockfile is unchanged after the checks.

## Phase 5: merge, tag, and archive

Merge the migration branch without rewriting the imported history. Record the
full resulting monorepo SHA as the first supported consolidation baseline.

Keep independent crate versions and use crate-prefixed release tags, for
example:

```text
graphql-orm-v0.16.0
graphql-orm-ai-v0.58.0
graphql-orm-backup-v0.7.0
graphql-orm-storage-v0.6.0
```

Only tag crates whose source and declared version are actually represented by
the tagged commit. Independent versions avoid unnecessary major or minor
bumps, while a single monorepo SHA still identifies a mutually compatible
source set.

After the monorepo baseline and downstream migration have been verified:

1. Put a prominent migration notice in each old repository README.
2. Link to the monorepo, this guide, the final old SHA, and the first monorepo
   baseline.
3. Disable new feature work in the old repositories.
4. Archive the old repositories rather than deleting them.

Keeping the old repositories readable preserves existing full-SHA Git
dependencies and historical links while consumers migrate.

## Consumer migration guide

This section is intended to be linked from applications and other projects
that use any of the consolidated crates.

### What changes

- The Rust package names and public crate names remain unchanged.
- Each package keeps its own version and features.
- Internal source moves to one Git repository.
- Git consumers update the URL and pin all selected packages to the same
  reviewed monorepo commit.
- Consumers may still select only one package; workspace membership does not
  make the other packages runtime or build dependencies.
- `agql-auth` remains external.

### Manifest changes

Use the final baseline and package versions published in the cutover notice.
For a project using all four crates, the dependency shape is:

```toml
[dependencies]
graphql-orm = {
  git = "https://github.com/Dastari/graphql-orm.git",
  rev = "<full-monorepo-sha>",
  version = "0.16.0",
  default-features = false,
  features = ["sqlite"],
}
graphql-orm-storage = {
  git = "https://github.com/Dastari/graphql-orm.git",
  rev = "<same-full-monorepo-sha>",
  version = "0.6.0",
  default-features = false,
  features = ["local"],
}
graphql-orm-backup = {
  git = "https://github.com/Dastari/graphql-orm.git",
  rev = "<same-full-monorepo-sha>",
  version = "0.7.0",
  default-features = false,
  features = ["local", "orm-sqlite"],
}
graphql-orm-ai = {
  git = "https://github.com/Dastari/graphql-orm.git",
  rev = "<same-full-monorepo-sha>",
  version = "0.58.0",
  default-features = false,
  features = ["sqlite"],
}
```

These are the consolidation-candidate versions; a later release may advance
them further. A consumer should declare only the crates it uses. Cargo selects
the package by dependency name from the shared Git repository.

Select one compatible ORM backend throughout an application. Avoid combining
the alternative `sqlite`, `postgres`, and `mssql` paths in a single feature
set unless the relevant crate explicitly documents that combination as
supported.

### Migration checklist

1. Choose the full monorepo baseline SHA from the cutover notice.
2. Change old AI, backup, and storage Git URLs to the monorepo URL.
3. Give every consolidated dependency the same `rev`.
4. Retain the package-specific version and deliberate feature selection.
5. Remove obsolete `[patch]` entries or source overrides for the old
   repositories.
6. Regenerate or update `Cargo.lock`.
7. Search the manifest and lockfile for old internal repository URLs.
8. Check the resolved graph for duplicate ORM and storage package identities.
9. Run the consuming project's backend-specific formatting, build, and test
   checks.
10. Commit the manifest and lockfile together, recording the baseline SHA in
    the migration change.

Useful checks:

```bash
rg 'graphql-orm-(ai|backup|storage)\\.git' Cargo.toml Cargo.lock
cargo tree -d
cargo tree -i graphql-orm
cargo tree -i graphql-orm-storage
```

The `rg` command should produce no old repository URLs after migration. Adapt
its file paths for workspaces whose manifests live below the root.

Projects that clone sibling repositories for CI scripts, documentation links,
release automation, or agent instructions must update those non-Cargo
references as well. Search the whole project for the old repository names,
but review matches rather than mechanically replacing historical prose.

### Migration acceptance report

Consumers can return this compact report to the cutover owner:

```text
Project:
Migration commit:
Monorepo baseline SHA:
Consolidated packages and versions:
Backend/provider features:
Old internal Git URLs remaining: none
Duplicate internal package sources: none
Checks run and results:
Known follow-up work:
```

## Rollback

Before merge, abandon or fix the migration branch; the source repositories are
unchanged and remain authoritative.

After merge but before old repositories are archived, consumers can restore
their previous manifest and lockfile pins to the recorded final old-repository
SHAs. Do not rewrite the monorepo's published history to perform a rollback.
Fix forward or revert the migration commit normally.

## Definition of done

The consolidation is complete when:

- every source repository has a clean, pushed, tested final commit;
- every outstanding source branch has an explicit disposition;
- AI, backup, and storage history is available under `crates/`;
- all five packages are workspace members with one root lockfile;
- internal dependencies resolve by workspace path and have no internal Git
  duplicates;
- `agql-auth` remains an exact external dependency;
- the explicit backend/provider CI matrix passes on one baseline commit;
- repository metadata and developer instructions point to the monorepo;
- at least one downstream project has completed the consumer checklist;
- the baseline SHA and independent crate tags are recorded; and
- old repositories contain migration notices and are archived only after the
  baseline is proven.
