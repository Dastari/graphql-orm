---
title: GraphQL contract development workflow
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2027-02-01
supersedes: []
---

# GraphQL contract development workflow

Use this sequence when a change affects generated or runtime GraphQL behavior.

1. Identify the package that owns the contract. Macro syntax/code generation
   belongs to `graphql-orm-macros`; runtime execution and metadata belong to
   `graphql-orm`; project-neutral router wire declarations belong to
   `graphql-orm-router-protocol`; federation composition, execution, and early
   denial belong to `graphql-orm-router`; AI tool admission belongs to
   `graphql-orm-ai`.
2. State the intended GraphQL operation, backend profile, authorization and
   assurance behavior, schema/data compatibility, and failure semantics.
3. Change the owning contract and all affected dependants in one branch. Keep
   internal dependencies as workspace paths.
4. Test exact SDL/root names, arguments, result shapes, exposure policy,
   fingerprints, authorization, assurance, and backend behavior. Naming
   profiles and alternative database backends require separate lanes.
5. If persistence changes, update migration and restore compatibility and
   prove the package’s backup/readiness invariants.
6. Update current mechanics in reference docs. Add an ADR only for a durable
   choice with meaningful alternatives; do not use an ADR as a change log.
7. Run the relevant checks in [testing](testing.md) and declare documentation
   impact in the pull request.

Generated operation descriptors and client manifests help discover and detect
surface drift. They do not grant execution authority or replace the finished
schema, host disclosure policy, resolver authorization, row policy, assurance,
or current principal checks.
