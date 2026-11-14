---
title: graphql-orm-router operations runbook
kind: runbook
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-11-12
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

Startup is fail closed for JWKS initialization, all source fetches, descriptor
binding, complete composition, authorization-catalog validation, and runtime
construction before the public listener binds. The optional role-scope
catalogue is deliberately not a startup dependency: its first fetch is lazy,
and only authorization-role-bearing requests fail while no verified snapshot
exists. Do not treat liveness as readiness.

The optional admin listener provides authenticated `GET /_router/status`,
`GET /_router/metrics`, `POST /_router/refresh`,
`POST /_router/subgraphs`, and explicit removal of dynamic subgraphs. It
authenticates with its own provider, whose audience public tokens do not carry.
Dynamic registration and removal are process-local, and dynamic services must
re-register after restart. The admin endpoint refuses to remove a static
source; only the embedding host's `RouterHandle::remove_subgraph` can, and the
source returns after restart.

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
| Authenticated WebSocket connections | 1024 | `router_websocket_connections`; close code 1013 when full at `connection_init` |
| Pending (unauthenticated) WebSocket connections | attempts/s × init deadline, capped at the connection limit (384) | `router_pending_websocket_connections`; HTTP 503 |
| WebSocket attempts | 128/s, one-second burst | HTTP 429 and `router_websocket_rejections_total` |
| Operations per WebSocket | 32 | operation limit errors |
| Subscription fan-out / upstream buffers | 32 / 1024 | Hive lagged/dropped counters |
| Client WebSocket message | 64 KiB | close code 4400 below the hard codec ceiling; transport disconnect at the ceiling |
| Connection-init deadline | 3 s | close code 4408 |
| Graceful drain | 10 s | shutdown duration and listener release |
| Byte route response size | 64 MiB per route | 502 `BYTE_ROUTE_RESPONSE_TOO_LARGE` and `router_byte_route_aborted_total` |
| Byte route concurrency / rate | 16 / 32 per second per route | 503 `BYTE_ROUTE_BUSY`, 429 `BYTE_ROUTE_RATE_LIMITED`, `router_byte_route_active_requests` |
| Byte route connect / idle / total | 5 s / 30 s / 15 min | 502, 504, and `router_byte_route_aborted_total` |

The authenticated router snapshot exposes request/error, cumulative downstream
latency, active graph version, WebSocket/subscription gauges, refresh,
composition, rejection, authorization-denial, and health values.
When byte routes are configured, `GET /_router/metrics` also reports six
objects keyed by route prefix: `router_byte_route_requests_total` (every
request a route claims), `router_byte_route_rejected_total` (refused before
contacting the upstream), `router_byte_route_upstream_errors_total` (router
error after contacting it: unreachable, timed out, invalid, or declared too
large), `router_byte_route_aborted_total` (body ended before completion),
`router_byte_route_bytes_total` (body bytes handed to clients), and the gauge
`router_byte_route_active_requests`. A router with no byte routes omits all
six keys rather than reporting empty objects or zeros. The optional
Hive Prometheus exporter adds execution histograms and the pinned subscription
counters
`hive.router.subscriptions.clients.lagged_messages_total` and
`hive.router.subscriptions.subgraphs.dropped_messages_total`.
Embedding hosts can also read `role_scope_stale_serve_total`,
`role_scope_stale_refusal_total`, and `role_scope_refresh_failure_total` from
the JWKS provider. Alert on any increase: stale serves mean the catalogue is
not refreshing, and refusals mean role-bearing requests are failing because a
stale snapshot passed `maximumStalenessSeconds`. Both emit rate-limited warning
logs; refresh failures log a redacted reason without URLs or header values.

## Reload and recovery

Polling uses conditional ETags. A changed source is composed with every other
active last-known-good source and published only after the complete runtime and
authorization catalog validate. A fetch timeout, unavailable source, invalid
descriptor, composition error, or runtime error marks status but does not
remove the active source. Use explicit removal only for an intended topology
change.

Refresh rounds run one at a time, but a round holds no lifecycle lock while it
fetches. A slow or unreachable schema endpoint delays only the next round; it
never blocks GraphQL execution, status, removal, or registration. A source
removed or re-registered while a round fetches keeps that newer decision, and
the round discards its result for that source.

For a rejected candidate, inspect the sanitized admin status, repair the source,
and use the authenticated refresh operation or wait for the next poll. The
same unchanged rejected fingerprint is not recomposed repeatedly.

## WebSocket reconnect behavior

Clients use `graphql-transport-ws` and place one bearer in
`connection_init.payload.authorization` (or its `headers` object). There is no
in-place token replacement. Legacy `subscriptions-transport-ws` (`graphql-ws`)
is not supported; requests without the supported subprotocol close with 4406.

- `4401`: obtain a valid token and open a new connection.
- `4408`: send `connection_init` within the configured deadline and reconnect.
- `4406`: request the `graphql-transport-ws` subprotocol.
- `4409`: do not reuse an active operation ID. Duplicate-ID detection precedes
  payload validation; the connection closes and all its operations are retired.
- HTTP `429`: honor `Retry-After`, add jitter, and increase exponential
  backoff. Do not immediately reconnect.
- `SUBSCRIPTION_CONTINUITY_LOST`: frames were dropped or skipped. The sanitized
  `error` is terminal: resubscribe and refetch authoritative state; no replay or
  trailing `complete` is provided. The connection remains usable.
- `SUBSCRIPTION_SCHEMA_RELOAD`: the selected graph retired; the `error` frame is
  terminal. Open a fresh connection and resubscribe without waiting for `complete`.
- `SERVICE_UNAVAILABLE` or `1011`: back off, query authoritative state, and
  reconnect. Missed events are not replayed.

Use jittered exponential backoff and refetch state after any gap. A subscription
is a notification channel, not the system of record.

An operation-scoped subgraph subscription failure produces terminal `error` (or a
`next` frame carrying GraphQL errors followed by `complete`) for that operation ID.
After protocol `error`, no further frames, including `complete`, follow for that
operation. Its state is released and the client may reuse the ID. It does not close
the public socket or unrelated sibling operations. Loss of the private graph
bridge is connection-scoped and closes the public socket with 1011. The first
terminal cause wins, so a client frame/protocol failure is not replaced by a
secondary bridge close.

Router-owned loss is local. The bounded 32-frame bridge queues subscription
frames only: an overflow retires the operation identified by that frame.
One-shot query/mutation results and connection controls bypass this queue.
Public socket write backpressure retires only that connection's subscriptions.
Neither path publishes a runtime loss signal or terminates sibling one-shot work.
Operation classification uses the executable AST and `operationName`; requests
without an explicit, unambiguous operation fail before execution.

Hive 0.2.19's synchronous dropped/lagged metric calls are observed directly,
independently of exporters, sampling, or polling intervals. Exported metrics are
preserved. The pinned counters expose only transport, with no operation, field,
subgraph, or upstream-buffer identity. HTTP-client broadcast lag is excluded
because it cannot skip frames for WebSocket receivers. Otherwise an unattributed
Hive loss conservatively invalidates active **subscriptions** on the runtime.
Queries and mutations are never terminated by continuity loss.

The fallback coalesces loss reports until the counters have been quiet for one
second. During this interval new subscriptions receive terminal
`SUBSCRIPTION_CONTINUITY_LOST` without starting upstream work; queries and
mutations continue normally. Closing admission prevents suppressed notifications
from hiding losses in newly started streams. Clients should use jittered backoff,
then resubscribe and refetch. Quiet active streams are notified without waiting
for a later source frame. Operation generations suppress all frames after error.

Residual denial-of-service consideration: one subscriber's lag or one upstream
burst can still end other subscribers' streams through the unattributed Hive
fallback. A sustained source can keep subscription admission paused. Debouncing
limits termination waves but cannot remove this shared availability risk.
Enforce connection/operation limits, rate-limit subscription starts at ingress,
use jittered reconnect backoff, alert on dropped/lagged counters, and isolate
untrusted subscription workloads in separate router instances where necessary.
An upstream operation/subscriber-scoped loss hook is required to eliminate this
cross-subscriber effect; see [#548](https://github.com/Dastari/graphql-orm/issues/548).

Hive's executor and fan-out buffers remain bounded and configurable. Raising
capacities can absorb bursts but is not a continuity guarantee. Keep alerting on
both counters and tune budgets from observed traffic. The adapter is tested
against the pinned counters; revalidate those hooks before upgrading Hive.
This terminal-error contract covers the public `graphql-transport-ws` gateway,
which is the only subscription transport.

### HTTP subscription requests

Since 0.10.0 the public HTTP path refuses subscriptions. Hive can negotiate
`text/event-stream`, `multipart/mixed` and Apollo multipart streams, including
over GET. Those streams would skip the gateway's mandatory bearer, expiry
teardown, attempt limiter and continuity signalling, as well as execute-time
authorization (GHSA-3qf7-j9r6-6qv3). A subscription that does not arrive
through the gateway's authenticated private endpoint gets HTTP 400 with a
plain JSON GraphQL error:

```json
{"errors":[{"message":"subscriptions are not supported over HTTP; use the graphql-transport-ws WebSocket endpoint","extensions":{"code":"SUBSCRIPTIONS_NOT_SUPPORTED"}}]}
```

A router without `subscriptions` configured has no gateway. It sends the same
status and code with the message `subscriptions are not enabled on this router`.

The refusal comes after parsing and before validation, planning or upstream
work. It does not depend on `Accept`, method, credentials or a sub-path of the
GraphQL path. A document is
refused when any operation it could select is a subscription, so use
`operationName` to run a query from a mixed document. Queries and mutations over
HTTP are unaffected. Clients that see this code must switch to
`graphql-transport-ws` rather than retry. The HTTP continuity exposure noted in
[#437](https://github.com/Dastari/graphql-orm/issues/437) no longer applies.
Its upstream request for an operation-scoped Hive loss hook is now
[#548](https://github.com/Dastari/graphql-orm/issues/548).

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
- **Byte route download fails:** a `graphql-orm-router-error` response header
  means the router refused or failed the request; its absence means the status
  and body came from the upstream. Compare `router_byte_route_upstream_errors_total`
  (unreachable, timed out, or declared too large) with
  `router_byte_route_aborted_total` (ended mid-body by the size limit, a
  timeout, or a client that stopped reading). Raise a limit only from observed
  file sizes and transfer times.
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
