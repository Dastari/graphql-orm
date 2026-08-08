---
title: graphql-orm-router operations runbook
kind: runbook
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-08
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router operations runbook

## Start and verify

1. Supply schema credentials through the named environment variables.
2. Run `graphql-orm-router --config <path> --check` in the deployment network.
3. Start the same configuration without `--check`.
4. Probe `GET /health` for process liveness and `GET /readiness` for a complete
   active executable graph.
5. Execute a representative authenticated query and subscription before
   accepting traffic.

Startup is fail closed. JWKS initialization, all source fetches, descriptor
binding, complete composition, authorization-catalog validation, and runtime
construction finish before the public listener binds. Do not treat liveness as
readiness.

The optional admin listener provides authenticated `GET /_router/status`,
`GET /_router/metrics`, `POST /_router/refresh`,
`POST /_router/subgraphs`, and explicit removal. Dynamic registration and
removal are process-local. Static sources return after restart and dynamic
services must re-register.

## Resource budgets

Production readiness requires choosing budgets from observed traffic rather
than merely raising defaults.

| Resource | Default budget | Operator signal |
| --- | ---: | --- |
| Public body / headers | 1 MiB / 64 KiB | GraphQL request errors and 4xx limit responses |
| Parser tokens / depth / fields | 10000 / 20 / 500 | GraphQL validation or limit errors |
| Public / subgraph deadline | 60 s / 30 s | request and subgraph error totals/latency |
| Connections per downstream host | 100 | subgraph latency and saturation |
| SDL or descriptor body | 2 MiB | unhealthy/rejected source status |
| Refresh attempts | 2 | refresh and composition totals |
| WebSocket connections | 1024 | `router_websocket_connections` |
| WebSocket attempts | 128/s, one-second burst | HTTP 429 and `router_websocket_rejections_total` |
| Operations per WebSocket | 32 | operation limit errors |
| Subscription fan-out / upstream buffers | 32 / 1024 | Hive lagged/dropped counters |
| Client WebSocket message | 64 KiB | close code 4400 below the hard codec ceiling; transport disconnect at the ceiling |
| Connection-init deadline | 5 s | close code 4408 |
| Graceful drain | 10 s | shutdown duration and listener release |

The authenticated router snapshot exposes request/error, cumulative downstream
latency, active graph version, WebSocket/subscription gauges, refresh,
composition, rejection, authorization-denial, and health values. The optional
Hive Prometheus exporter adds execution histograms and the pinned subscription
counters
`hive.router.subscriptions.clients.lagged_messages_total` and
`hive.router.subscriptions.subgraphs.dropped_messages_total`.

## Reload and recovery

Polling uses conditional ETags. A changed source is composed with every other
active last-known-good source and published only after the complete runtime and
authorization catalog validate. A fetch timeout, unavailable source, invalid
descriptor, composition error, or runtime error marks status but does not
remove the active source. Use explicit removal only for an intended topology
change.

For a rejected candidate, inspect the sanitized admin status, repair the source,
and use the authenticated refresh operation or wait for the next poll. The
same unchanged rejected fingerprint is not recomposed repeatedly.

## WebSocket reconnect behavior

Clients use `graphql-transport-ws` and place one bearer in
`connection_init.payload.authorization` (or its `headers` object). There is no
in-place token replacement.

- `4401`: obtain a valid token and open a new connection.
- `4408`: send `connection_init` within the configured deadline and reconnect.
- `4406`: request the `graphql-transport-ws` subprotocol.
- HTTP `429`: honor `Retry-After`, add jitter, and increase exponential
  backoff. Do not immediately reconnect.
- `SUBSCRIPTION_SCHEMA_RELOAD`: the selected graph retired; wait for operation
  completion, open a fresh connection, and resubscribe.
- `SERVICE_UNAVAILABLE` or `1011`: back off, query authoritative state, and
  reconnect. Missed events are not replayed.

Use jittered exponential backoff and refetch state after any gap. A subscription
is a notification channel, not the system of record.

An operation-scoped subgraph subscription failure produces `error` (or a
GraphQL error followed by `complete`) for that operation ID; it does not close
the public socket or unrelated sibling operations. Loss of the private graph
bridge is connection-scoped and closes the public socket with 1011. The first
terminal cause wins, so a client frame/protocol failure is not replaced by a
secondary bridge close.

A successful one-shot query or mutation over WebSocket emits `next` and then
`complete` with the original ID. Retire that ID on `complete`. If the socket is
lost after a mutation was sent but before `complete`, treat its outcome as
uncertain: do not replay it automatically unless the application has an
idempotency contract. An oversized serialized message is a connection-level
failure; move bulk data to bounded HTTP or chunk it below 64 KiB rather than
retrying the same frame.

## Shutdown

The executable catches `SIGTERM`, `SIGINT`, and `SIGQUIT`, stops accepting new
work, drains listeners for the configured whole-second deadline, stops
background refresh tasks, flushes telemetry, invokes engine shutdown hooks,
and releases listeners. Send a second hard termination only after the
deployment grace period exceeds the router drain deadline.

## Troubleshooting

- **Does not become ready:** run `--check`; verify JWKS reachability, every SDL
  and descriptor, descriptor fingerprints/operations, and full composition.
- **Active graph did not change:** compare observed and active fingerprints.
  Unchanged candidates are skipped; rejected candidates retain last-known-good.
- **Dynamic registration rejected:** verify service subject and exact trusted
  ID/name/origins, IP-literal execution origin, host/port/CIDR policy, and all
  DNS answers.
- **HTTP allowed by router but denied downstream:** expected defence in depth.
  Align protocol metadata and the authoritative subgraph guard; do not weaken
  the guard.
- **Subscription gaps:** inspect lag/drop metrics, reduce event rate or fan-out,
  increase bounded buffers only with memory evidence, and refetch state.
- **Metrics route unavailable:** admin JSON needs its exact bearer scope;
  Prometheus needs an explicitly configured distinct port and network access.
- **SIGTERM exceeds budget:** inspect long-running requests and exporter
  shutdown, then keep the orchestration grace period above the configured
  drain timeout.

The generic release artifact remains subject to the distribution/license and
SBOM approval in ADR-0008; a passing runtime test is not that approval.

## Single-instance event limitation

Static query traffic may be replicated when every instance receives the same
configuration, but dynamic registry state and activation are not coordinated.
Generated process-local subgraph events do not fan out across multiple
write-capable subgraph instances. The initial event profile therefore requires
one active instance of each write/event-owning subgraph. There is no durable
queue, replay, exactly-once delivery, or shared router registry. Ordinary
queries against the authoritative store recover missed state.
