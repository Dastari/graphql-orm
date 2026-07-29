# GraphQL ORM workspace agent guide

These rules apply to the entire repository. More specific `AGENTS.md` files
under a crate add package-local invariants.

## Workspace boundaries

- The workspace contains `graphql-orm`, `graphql-orm-macros`,
  `graphql-orm-storage`, `graphql-orm-backup`, and `graphql-orm-ai`.
- `agql-auth` remains an external exact-revision dependency. Do not modify its
  repository unless the task explicitly includes it.
- Keep the packages independently consumable. Do not turn AI, backup, or
  storage into features or optional dependencies of the core ORM crate.
- Preserve this acyclic dependency direction:
  `graphql-orm-ai -> graphql-orm-backup -> graphql-orm-storage`,
  `graphql-orm-ai -> graphql-orm`, optional
  `graphql-orm-backup -> graphql-orm`, and
  `graphql-orm -> graphql-orm-macros`.
- Internal packages use workspace path dependencies and the root `Cargo.lock`.
  Never add Git dependencies between packages in this workspace.

## Cross-crate changes

- Make reusable changes in the package that owns the contract and update all
  affected dependants in the same branch.
- Preserve crate-local security, restore, provider, schema, and locking
  invariants. Read the nearest crate `AGENTS.md` before editing below it.
- Keep public API cleanup separate from mechanical repository or dependency
  changes unless compatibility requires them together.
- Update package changelogs, migration notes, versions, and examples according
  to each crate's local release rules.

## Verification

- Never use workspace `--all-features`; ORM database backends are alternative
  configurations.
- Use explicit `-p` and feature selections for backend and provider checks.
- Default database tests may use temporary SQLite. PostgreSQL or MSSQL tests
  may run only through test-owned disposable infrastructure documented by the
  affected crate; never use a live application database.
- Run `cargo fmt --all -- --check`, relevant package tests, warnings-denied
  Clippy and Rustdoc, dependency-tree checks, and all affected backend compile
  lanes before handoff.
