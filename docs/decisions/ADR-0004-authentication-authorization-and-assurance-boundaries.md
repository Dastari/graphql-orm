---
title: ADR-0004 Authentication authorization and assurance boundaries
kind: decision
status: accepted
owner: graphql-orm-maintainers
last_reviewed: 2026-08-01
review_by: 2027-08-01
supersedes: []
---

# ADR-0004: Authentication, authorization, and assurance boundaries

## Context

The ORM needs a project-neutral current subject and policy hooks. Some
operations also need evidence of a stronger or more recent authentication
ceremony. The external `agql-auth` package can supply principals and assurance
decisions, but provider workflows, tokens, cookies, WebSocket transport, and
product policy belong to applications.

## Decision

Authentication establishes a principal; authorization decides whether that
principal may perform the exact operation/resource action; operation assurance
adds declared actor/recency/step-up requirements. These are separate checks and
all applicable checks must pass.

`graphql-orm` owns project-neutral `AuthSubject`, database auth context, policy
hooks, assurance declarations, completeness audits, and a generic enforcement
hook. Its optional `auth-agql` bridge is one-way and exact-revision pinned. It
does not own login, token/session persistence, cookie attributes, WebSocket
credential refresh, MFA policy, identity-provider evidence, or route security.

Generated resolvers enforce the installed server hook. Custom resolvers use the
same declared guard. Directive metadata and manifests are advisory. Absent,
stale, malformed, or insufficient evidence fails according to the installed
policy; it is never synthesized from unrelated claims.

## Consequences

- Applications can use HTTP cookies, bearer credentials, WebSocket session
  protocols, or another transport without embedding one in the ORM.
- Passing assurance does not grant authorization, and passing authorization
  does not satisfy a declared step-up requirement.
- AI and backup packages do not bypass host authorization merely because they
  hold durable records or operation descriptors.

## Supersession

A change that moves authentication transport or product policy into a reusable
package requires a new ADR and cross-package security review.
