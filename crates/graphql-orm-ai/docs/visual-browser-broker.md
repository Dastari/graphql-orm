---
title: "Future Capability-Scoped Visual Browser Broker"
kind: architecture
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-01
supersedes: []
---

# Future Capability-Scoped Visual Browser Broker

This document records a back-burner architecture boundary. It does not enable
browser automation, register a tool, define a database migration, or make a
browser process part of the current provider runtime. Hosted web search is a
separate provider built-in and must not be implemented by silently broadening
this broker.

## Intended authority flow

A visual-browser action starts as an exact, host-registered application tool.
The normal coordinator must first:

1. rehydrate the current principal;
2. re-evaluate current session, scope, tool, rule, disclosure, egress, budget,
   and approval policy;
3. claim the exact current run attempt and fencing generation;
4. issue a short-lived, single-purpose browser capability; and
5. call a separately deployed host broker through a fixed logical target.

The broker validates that capability and performs only its typed action. It
returns a bounded observation to the coordinator, which applies the registered
projection and classification before any provider continuation. Browser state
never grants GraphQL, provider, shell, filesystem, or approval authority.

```text
model request
    -> registered tool descriptor
    -> coordinator + current-principal checks
    -> one-shot browser capability
    -> host broker
    -> protected bounded observation
    -> disclosure-approved provider continuation
```

## Reusable and host-owned responsibilities

`graphql-orm-ai` may eventually own project-neutral value types for:

- a closed browser action vocabulary;
- exact context, origin, target, run, attempt, fence, expiry, and maximum-use
  bindings;
- capability issuance and consumption traits;
- protected screenshot/DOM-summary artifact metadata;
- bounded lifecycle facts and cancellation; and
- coordinator integration with ordinary tool, disclosure, retention, audit,
  and budget policy.

The host owns process/container isolation, browser installation and patching,
network controls, DNS/private-network protection, context allocation, cookie
policy, download quarantine, credential mediation, and the fixed transport to
the broker. A browser-specific adapter may translate the closed contract to a
reviewed engine. It must not expose a generic DevTools, Playwright, WebDriver,
MCP, JSON-RPC, or arbitrary JavaScript bridge.

## Closed capability model

The first useful release should be observation-first. Candidate actions are
bounded navigation to a server-authorized origin, viewport capture, and a
bounded accessibility/DOM summary. Any click, form entry, upload, download,
clipboard action, credential use, or consequential submission remains absent
until it has its own reviewed type and policy.

Each capability must bind at least:

- owner principal reference, tenant, AI session, and application scope;
- exact run, attempt, run-lease generation, tool call, and broker context;
- allowed action and normalized origin/target chosen by host policy;
- descriptor, broker-registration, network-policy, and disclosure
  fingerprints;
- issue/expiry times and a one-shot or small immutable use ceiling; and
- output byte, pixel, node, depth, redirect, and duration ceilings.

Capabilities are non-transferable, non-refreshable bearer material protected
in transit and never persisted in browser-visible state. The broker rejects
unknown fields, action widening, redirects outside the fixed policy, stale
fences, replays beyond the use ceiling, and any model-authored destination not
admitted by the host.

## Hard boundaries

The broker must have no ambient application bearer token, user browser
profile, personal cookies, cloud/SSH agent, repository mount, local filesystem,
shell, arbitrary GraphQL client, arbitrary header injection, or unrestricted
private-network access. Browser contexts are isolated by owner and AI session,
have idle and absolute lifetimes, and are destroyed on cancellation, session
deletion, policy change, ownership loss, restore, or uncertain fencing.

Web pages and visual observations are untrusted input. Instructions found in a
page cannot change the registered tool, capability, origin, target, approval,
principal, disclosure projection, or application-tool authority. A visual
gesture is never evidence of authorization. Consequential product changes
should normally be performed by the exact authenticated GraphQL mutation after
its established approval flow, not by clicking an equivalent web control.

## Artifacts, events, and recovery

Screenshots and extracted observations are protected content artifacts, not
ordinary lifecycle-event payloads. Events carry only bounded identifiers,
classification, dimensions/counts, state, and redacted reason codes. Content
must not enter URLs, logs, analytics, provider metadata, or browser storage.
Every read rechecks current owner, scope, field/row, retention, and disclosure
policy.

Cancellation fences new actions and output persistence, then destroys the
context. An ambiguous broker result is cleanup-required, never success.
Portable restore must not revive a browser context or capability. Restore and
retention reconcile or purge protected local artifacts while treating every
external context as absent or cleanup-required according to an authoritative
broker proof.

## Future phases

1. Specify observation-only contracts and a fake broker conformance suite.
2. Implement one host-isolated broker with navigation and capture only.
3. Add protected artifact persistence, replay metadata, cancellation, cleanup,
   and restore reconciliation.
4. Add narrowly typed interaction actions only when each has an explicit
   approval and rollback story.
5. Consider authenticated browser contexts only through a separate credential
   broker and threat review; never inherit the operator's ordinary browser.

Acceptance must prove cross-owner/context isolation, exact fence and one-shot
consumption, origin/redirect enforcement, private-network denial, bounded
artifacts, immediate revocation, cancellation races, no capability leakage,
and that hostile page content cannot enable another application tool or a
disabled broker action.
