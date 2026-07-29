# Consumer Project Migration Agent Prompt

Copy the prompt below into the main agent session for each project that
consumes one or more `graphql-orm-*` crates.

---

The `graphql-orm` ecosystem has been consolidated into a single monorepo.

## Canonical source

- Repository: `https://github.com/Dastari/graphql-orm.git`
- Reviewed initial consolidation baseline:
  `3b20a947df9e88ff18f4f0aa7d0c1fbab06774a5`

Packages at that baseline:

| Package | Version |
| --- | --- |
| `graphql-orm` | `0.16.0` |
| `graphql-orm-macros` | `0.16.0` |
| `graphql-orm-storage` | `0.6.0` |
| `graphql-orm-backup` | `0.7.0` |
| `graphql-orm-ai` | `0.58.0` |
| `graphql-orm-ai` schema module | `0.51.0` |

`agql-auth` remains an independent external repository. Do not move it into
the monorepo or change its revision unless this project specifically requires
and authorizes that work.

## Objective

Audit and migrate this consumer project to the consolidated monorepo.

Do not assume that this project uses, or should depend on, every
`graphql-orm-*` package. Retain only the packages that its source code, public
API, generated code, configuration, tests, examples, or feature forwarding
actually require.

Complete the migration, validation, documentation, and handoff rather than
stopping after an inventory.

## Read the applicable instructions

Do not assume the monorepo exists at a particular filesystem path.

1. Read this consumer project's local `AGENTS.md` files and follow the nearest
   applicable instructions.
2. Inspect the monorepo documentation at the reviewed revision:
   - `https://github.com/Dastari/graphql-orm/blob/3b20a947df9e88ff18f4f0aa7d0c1fbab06774a5/AGENTS.md`
   - `https://github.com/Dastari/graphql-orm/blob/3b20a947df9e88ff18f4f0aa7d0c1fbab06774a5/docs/monorepo-consolidation.md`
3. For every package this project actually uses, inspect its `AGENTS.md`,
   `README.md`, `MIGRATION.md`, `CHANGELOG.md`, and `Cargo.toml` at the same
   revision.
4. If the monorepo already has a local checkout, discover and validate its
   actual location and revision. Do not assume a path such as
   `/home/toby/dev/graphql-orm`.
5. If remote file inspection is inconvenient and no checkout exists, create a
   temporary reference checkout:

   ```bash
   migration_checkout=$(mktemp -d)
   git clone https://github.com/Dastari/graphql-orm.git \
       "$migration_checkout/graphql-orm"
   git -C "$migration_checkout/graphql-orm" checkout --detach \
       3b20a947df9e88ff18f4f0aa7d0c1fbab06774a5
   ```

   Use this checkout only as migration reference material. Do not edit or
   commit inside it.

If a newer monorepo revision has been explicitly selected for this project,
inspect the instructions and migration documents at that newer revision and
use it consistently instead of silently mixing revisions.

## Audit the consumer before editing

Inspect the complete repository, including:

- root and nested `Cargo.toml` files;
- `Cargo.lock`;
- workspace dependency declarations;
- renamed dependencies;
- optional dependencies and project feature forwarding;
- target-specific dependencies;
- build dependencies and development dependencies;
- `[patch]` and source-replacement sections;
- Rust imports, re-exports, trait implementations, and public signatures;
- generated GraphQL code and schema modules;
- configuration and deployment files;
- examples, tests, scripts, CI workflows, and documentation.

Determine:

1. Which `graphql-orm-*` packages are declared directly.
2. Which packages are genuinely used.
3. Which packages appear in this project's public API.
4. Which packages are used only by tests, examples, optional features, or
   particular deployment targets.
5. Whether the project implements or names any of these contracts:
   - `graphql-orm-storage` storage traits or types, including `BlobStore`;
   - `graphql-orm-backup` repository, restore, schema, or adapter traits;
   - `graphql-orm` entities, schema modules, generated operations, or backend
     types;
   - `graphql-orm-ai` runtime, provider, persistence, tool, approval, backup,
     or restore contracts;
   - direct `agql-auth` types that must share a compatible source universe.
6. Whether old and new Git sources currently resolve duplicate versions of
   logically identical types.

Do not remove a direct dependency merely because another selected crate
depends on it transitively when this project:

- names that package's types directly;
- exposes those types publicly;
- implements one of its traits;
- forwards one of its Cargo features; or
- uses it in tests, examples, generated code, or configuration.

Do not add storage, backup, or AI merely because they are members of the
monorepo. For example, a project that uses only `graphql-orm` should declare
only `graphql-orm`.

## Replace independent repository dependencies

Every selected `graphql-orm-*` Git dependency must use:

- the canonical monorepo URL;
- the same exact full Git revision; and
- the compatible package version.

Cargo selects the requested package by dependency name from the shared Git
repository. No subdirectory argument is required.

The following examples are illustrative. Include only the packages and
features this project needs:

```toml
[dependencies]
graphql-orm = {
    git = "https://github.com/Dastari/graphql-orm.git",
    rev = "3b20a947df9e88ff18f4f0aa7d0c1fbab06774a5",
    version = "0.16.0",
    default-features = false,
    features = ["sqlite"]
}

graphql-orm-storage = {
    git = "https://github.com/Dastari/graphql-orm.git",
    rev = "3b20a947df9e88ff18f4f0aa7d0c1fbab06774a5",
    version = "0.6.0",
    default-features = false,
    features = ["local"]
}

graphql-orm-backup = {
    git = "https://github.com/Dastari/graphql-orm.git",
    rev = "3b20a947df9e88ff18f4f0aa7d0c1fbab06774a5",
    version = "0.7.0",
    default-features = false,
    features = ["local", "orm-sqlite"]
}

graphql-orm-ai = {
    git = "https://github.com/Dastari/graphql-orm.git",
    rev = "3b20a947df9e88ff18f4f0aa7d0c1fbab06774a5",
    version = "0.58.0",
    default-features = false,
    features = ["sqlite"]
}
```

Preserve the project's intended backend, provider, storage, and
optional-dependency feature selections. Do not copy example features blindly.

If this is a Cargo workspace, prefer defining each selected package once under
the consumer's `[workspace.dependencies]` and inheriting it from member
manifests where that matches the project's existing dependency policy.

## Dependency and feature rules

- All selected `graphql-orm-*` Git packages must use the same monorepo `rev`.
- Remove old Git URLs for `graphql-orm-ai`, `graphql-orm-backup`, and
  `graphql-orm-storage`.
- Remove obsolete patches, source overrides, and local path workarounds for
  the old independent repositories.
- Do not turn storage, backup, or AI into features or optional dependencies of
  the core `graphql-orm` crate.
- The consumer may define its own optional product features around direct
  dependencies when appropriate, for example:

  ```toml
  [features]
  ai = ["dep:graphql-orm-ai"]
  backup = ["dep:graphql-orm-backup"]
  ```

- Do not introduce these features if the project's existing product design
  does not need optional compilation.
- Select compatible ORM backends deliberately.
- Do not use workspace `--all-features` when SQLite, PostgreSQL, and MSSQL
  features represent alternative builds.
- Keep `agql-auth` external.
- Do not add `agql-auth` to projects that do not use it.
- If the project directly uses both `agql-auth` and `graphql-orm`, confirm that
  they resolve to the reviewed compatible `agql-auth` source and type
  universe.

## Breaking-change review

Do not treat this as only a Git URL replacement. Look actively for public,
behavioral, feature, persistence, configuration, and source-identity changes.

In particular:

- `graphql-orm-backup` advanced from `0.6.0` to `0.7.0` because its public
  contracts expose `graphql-orm-storage` types from the new source identity.
- `graphql-orm-ai` advanced from `0.57.0` to `0.58.0` because it consumes the
  new backup and storage package identity.
- `graphql-orm-storage` remains `0.6.0`.
- `graphql-orm` and `graphql-orm-macros` remain `0.16.0`.
- The AI schema module remains `0.51.0`.
- The repository move itself does not require a database or AI schema
  migration.
- Rust types compiled from different Cargo Git sources are distinct even when
  their names and source text are identical. Duplicate old and new sources can
  cause trait-bound failures, mismatched argument types, incompatible schema
  modules, and failed adapter implementations.
- Review feature names and defaults rather than assuming they are unchanged.
- Review custom storage, backup, ORM, AI, authentication, and generated
  GraphQL integrations against the selected package's current contracts.
- Check whether this project persists crate or schema version identifiers in
  migrations, backup manifests, restore readiness records, configuration, or
  operational metadata.

If the audit finds a genuine project API, behavior, configuration, feature, or
persistence change:

- make the required consumer changes;
- update its changelog and migration guide;
- update examples and deployment guidance;
- change its version according to its own release policy; and
- state explicitly whether a data migration is required.

Do not perform unrelated API cleanup or broad dependency upgrades as part of
this migration.

## Lockfile and source reconciliation

Regenerate `Cargo.lock` after changing the manifests, but do not perform an
unrelated broad dependency update.

Search the repository for obsolete sources:

```bash
rg -n 'github\.com/Dastari/graphql-orm-(ai|backup|storage)' \
    --glob 'Cargo.toml' \
    --glob 'Cargo.lock' \
    --glob '*.md' \
    --glob '*.sh' \
    .
```

Historical migration notes may retain old URLs when they are clearly marked
as historical. Active manifests, lockfile entries, examples, CI, and current
setup guidance must use the monorepo.

Use Cargo metadata and dependency trees to confirm source identity:

```bash
cargo metadata --locked --format-version 1
cargo tree -d
```

For every package the consumer resolves, inspect the relevant reverse tree,
for example:

```bash
cargo tree -i graphql-orm
cargo tree -i graphql-orm-storage
cargo tree -i graphql-orm-backup
cargo tree -i graphql-orm-ai
```

Do not run a reverse-tree command for a package the project does not resolve
and then treat Cargo's “package not found” result as a migration failure.

Confirm:

- each selected `graphql-orm-*` package resolves once;
- every selected package uses the same monorepo source and revision;
- no old independent Git source remains active;
- no unintended duplicate `agql-auth` source exists; and
- project feature forwarding selects the intended backend and provider
  implementations.

## Validation

Run validation in proportion to the packages and features this project owns.
At minimum:

1. Run formatting checks.
2. Run the relevant unit, integration, and documentation tests.
3. Run warnings-denied Clippy for each supported feature lane.
4. Run warnings-denied Rustdoc for the selected public packages.
5. Compile every backend, provider, storage, backup, and AI feature
   combination supported by this project.
6. Re-run generated GraphQL naming, schema, and operation-contract tests where
   applicable.
7. Re-run backup and restore conformance where applicable.
8. Re-run lock, streaming, object-storage, or provider tests where applicable.
9. Re-run the project's packaging or release-policy checks where applicable.

Never connect tests, migrations, or diagnostics to a live development,
staging, or production database.

- SQLite tests should use temporary or in-memory databases.
- PostgreSQL or MSSQL tests must use disposable, test-owned infrastructure
  already documented by the project.
- Do not infer permission to use credentials or databases merely because they
  are present in the environment.

Do not use `--all-features` when backend features are alternative
configurations. Use explicit package and feature selections.

## Change and commit discipline

- Preserve unrelated worktree changes.
- Do not delete local clones of the old standalone repositories.
- Do not modify or archive the old remote repositories as part of this
  consumer migration.
- Do not make changes in a temporary reference checkout.
- Keep migration changes scoped to this consumer project unless a genuine
  reusable defect must be fixed in the monorepo and that additional work is
  authorized.
- Review the complete diff before committing.
- Commit only after the project is internally consistent and the required
  validation passes.

## Required handoff

At completion, report:

1. Which `graphql-orm-*` packages the project genuinely uses.
2. Which packages were determined to be unused and whether their declarations
   were removed.
3. Which old Git dependency declarations, patches, or source overrides were
   removed.
4. The exact monorepo revision selected.
5. The final package versions and feature selections.
6. Every breaking change or source-identity issue discovered.
7. Any source, public API, configuration, generated code, documentation,
   migration, CI, or deployment changes made.
8. Whether any database or persistent-data migration is required.
9. The validation commands and results.
10. The commit, branch, pull request, and worktree status if publication was
    requested.
11. Any remaining blocker or deliberately deferred follow-up.

The migration is complete only when the project resolves one coherent
monorepo dependency universe, its owned feature lanes pass, current guidance
no longer points at the independent repositories, and the final handoff makes
any compatibility impact explicit.
