# Workspace Contribution Workflow

`graphql-orm-ai`, `graphql-orm-backup`, `graphql-orm-storage`, `graphql-orm`,
and `graphql-orm-macros` now share one repository. A reusable internal contract
and all affected consumers should be changed, reviewed, and tested on one
workspace branch.

`agql-auth` remains external. Changes to it still require a separately reviewed
handoff and an exact final revision.

## Ownership model

Use one integration owner for a cross-crate change. Package-focused agents may
work in separate Git worktrees, but must not edit the same files or merge
independently into the integration branch. The integration owner resolves
dependency order, runs the combined matrix, and publishes one candidate
monorepo revision.

Keep implementation in the package that owns the reusable contract:

1. storage primitives belong in `graphql-orm-storage`;
2. backup/restore orchestration belongs in `graphql-orm-backup`;
3. database, schema, and generated GraphQL contracts belong in `graphql-orm`
   or `graphql-orm-macros`;
4. AI runtime and provider behavior belongs in `graphql-orm-ai`; and
5. authentication and principal lifecycle remain external in `agql-auth`.

Internal packages use path dependencies and one root lockfile. Do not create
internal Git pins or wait for separate repository SHAs.

## Dependency sequence

Implement and validate from the bottom of the affected dependency graph:

1. storage;
2. backup;
3. ORM/runtime macros where applicable; and
4. AI.

This order is a testing sequence, not a requirement for separate commits or
pull requests. The final candidate must resolve one source for every internal
package.

For an external `agql-auth` change, merge and record the reviewed auth revision
first, update the root workspace dependency, then run the affected ORM and AI
matrix.

## Shared-machine safety

- Never run write commands in a worktree owned by another agent.
- Never stage all files in a dirty worktree without proving ownership of every
  change.
- Never resolve conflicts by discarding, stashing, or resetting another
  agent's files.
- Prefer a separate `git worktree` and branch for every concurrent task in the
  same repository.
- Record the owning agent and branch in the PR or issue before parallel work
  starts.

## Handoff prompt contents

Every upstream prompt should state:

- the exact external repository, base branch, existing PR/branch, and owning
  boundary;
- the reusable problem and required public contract, without consumer-domain
  entities or policies;
- compatibility and security invariants;
- expected version, README, changelog, migration, and Rustdoc updates;
- required tests, backend compile checks, and database isolation;
- whether the owner should merge; and
- the final information the workspace needs: merge strategy, version, final
  SHA, and any migration or feature changes.

The downstream agent resumes only after receiving that final handoff.
