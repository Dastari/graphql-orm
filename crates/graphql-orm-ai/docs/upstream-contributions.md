---
title: "Workspace and External-Contribution Workflow"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# Workspace and External-Contribution Workflow

`graphql-orm-ai`, `graphql-orm-backup`, `graphql-orm-storage`,
`graphql-orm`, and `graphql-orm-macros` are packages in one monorepo. Make a
reusable internal contract in the package that owns it, update every affected
consumer on the same integration branch, and verify the affected dependency
lanes together. Internal packages use workspace paths and the one root lockfile;
do not create internal Git pins, separate-package handoffs, or local overrides.

`agql-auth` is the only external dependency in this boundary. Treat it as
read-only here unless the task explicitly includes that repository. Record a
needed external change as a reviewed handoff and update this workspace only
after its final merged revision is available.

## Ownership and coordination

Use one integration owner for a cross-package change and keep each concurrent
edit scoped to distinct files. Temporary coordination belongs in ignored root
`.handoff/` (singular), never in a package, release artifact, dependency
manifest, or committed documentation tree. The integration owner resolves
cross-package ordering, runs the combined matrix, and publishes one monorepo
candidate revision.

Keep implementation in the package that owns the reusable contract:

1. storage primitives belong in `graphql-orm-storage`;
2. backup and restore orchestration belongs in `graphql-orm-backup`;
3. database, schema, and generated GraphQL contracts belong in `graphql-orm`
   or `graphql-orm-macros`;
4. AI runtime and provider behavior belongs in `graphql-orm-ai`; and
5. authentication and principal lifecycle remain external in `agql-auth`.

Never discard, reset, stash, or stage another contributor's changes merely to
make a branch appear clean. Confirm file ownership before editing in a dirty
workspace and review the complete combined diff before handoff.

## Dependency and validation sequence

Implement and validate from the bottom of the affected dependency graph:

1. storage;
2. backup;
3. ORM/runtime macros where applicable; and
4. AI.

This is a test sequence, not a requirement for separate commits or pull
requests. The resulting candidate must resolve one source for every internal
package. For `agql-auth`, obtain and record the reviewed external revision,
update the root workspace dependency, then run the affected ORM and AI matrix.

## External handoff contents

An `agql-auth` handoff should state:

- the exact external repository, base branch, and owning boundary;
- the reusable problem and required public contract, without consumer-domain
  entities or policies;
- compatibility and security invariants;
- expected version, README, changelog, migration, and Rustdoc updates;
- required tests, backend compile checks, and database isolation; and
- the final information this workspace needs: merge strategy, version, final
  SHA, and migration or feature changes.

Resume the dependent workspace change only after that final revision is known.
