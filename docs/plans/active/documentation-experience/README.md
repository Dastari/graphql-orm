---
title: Documentation experience plan
kind: plan
status: active
owner: workspace-maintainers
last_reviewed: 2026-09-02
review_by: 2026-10-01
supersedes: []
---

# Documentation experience

## Outcome

Give a new application developer a project-neutral, version-pinned path from
the repository README to a working GraphQL ORM service, then to complete and
discoverable package, configuration, security, and operations documentation.

## Non-goals

- Change public APIs, package versions, or database semantics solely to make
  documentation examples shorter.
- Represent local public-demo authentication settings as production security
  guidance.
- Delete accepted decisions, completed plans, investigations, or archival
  evidence while simplifying newcomer navigation.

## Dependencies

- The checked-in `graphql-orm` example and its explicit SQLite migration API.
- Component maintainers owning each package-local README and reference.
- The generated workspace inventory and documentation-governance checks.

## Acceptance gates

- The root README explains the purpose, fit, non-fit, package choices,
  backends, source installation, maturity, security boundaries, and a
  five-minute route without directing newcomers to completed-plan history.
- A project-neutral SQLite + async-graphql example has source, migration,
  seed data, generated roots, local HTTP transport, sample request, and a
  passing smoke test.
- The central index supports Learn, How-to, Reference, and Concepts journeys
  while retaining maintainers' links to architecture, decisions, operations,
  plans, investigations, and archive.
- Each independently consumable package has an installation and usage route;
  public configuration options and defaults are discoverable from a canonical
  reference path.
- Documentation validation, generated inventory validation, formatting, and
  relevant example/package checks pass.

## Current checkpoint

The first three documentation passes are complete:

- The public front door now gives a 60-second project description, fit and
  non-fit, current exact Git package-release installation, capability limits,
  all nine package choices, security boundaries, and a source-backed SQLite
  quickstart with its own reproducible repository snapshot pin.
- The core ORM reference now has a discoverable entry point and canonical macro
  and attribute reference; Learn, How-to, Reference, and Concepts navigation
  link into it without routing newcomers through completed-plan history.
- Companion package documentation now has consistent onboarding and
  configuration routes. The Learn index inventories the checked-in quickstart
  and focused ORM, router, and router-protocol examples.

The milestone remains active. The remaining acceptance gaps are:

- a hosted, searchable, versioned documentation site with matching hosted
  rustdoc for all packages;
- a broader set of tested end-to-end demos beyond the SQLite quickstart,
  especially PostgreSQL, SQL Server integration, router federation, storage /
  backup, and AI paths; and
- a project-neutral public AI host-bootstrap/demo API. The public AI schema
  module and mock provider exist, but a runnable host still requires
  application-owned schema application, authenticated executor/context,
  current-principal resolution, access/tool/egress/secret/content-protection
  policies, and explicit runtime readiness wiring.

Do not mark this plan complete until those follow-up deliverables land and are
verified with the documentation surface.
