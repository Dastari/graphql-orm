---
title: "Native Anthropic Provider"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-11-15
review_by: 2027-02-01
supersedes: []
---

# Native Anthropic Provider

Enable `provider-anthropic` for a native adapter to Anthropic's
[Messages API](https://platform.claude.com/docs/en/api/messages/create) and
[streaming event contract](https://platform.claude.com/docs/en/build-with-claude/streaming).
It is not a configurable OpenAI-compatible proxy: `AnthropicProvider::new`
uses the official HTTPS Messages endpoint, a fixed supported API-version
header, disabled redirects, and a bounded timeout.

## Construction and credentials

Create `AnthropicProviderConfig` from a `SecretRef`, then pass it and an
`Arc<dyn AiSecretStore>` to `AnthropicProvider::new`. The public configuration
retains only the secret reference. Plaintext is resolved immediately before
the HTTP request, validated as a header value, and neither retained nor placed
in errors. Provider profiles, GraphQL values, models, and tool arguments cannot
set a URL, header, or plaintext credential.

### Policy-checked endpoint

`AnthropicProvider::with_endpoint(config, endpoint_policy, secrets, endpoint)`
targets an exact Messages endpoint instead of the official one, for example an
approved gateway, a regional endpoint or a local fake server in tests. The
endpoint must be `http` or `https` with a host, no user information, query or
fragment, and a non-root path, and `AiProviderEndpointPolicy::authorize_endpoint`
must approve its normalized form for `AiProviderKindInput::Anthropic`. The
request shape, version header and disabled redirects are unchanged. Every
egress manifest for a request through this provider must name the normalized
endpoint as its destination, so data approved for the official destination
cannot reach a gateway. `Debug` output shows `<deployment-authorized>` rather
than the endpoint. `AnthropicProvider::new` is unchanged.

Only the deployment containing this adapter should receive Anthropic network
egress and credential-store access. DNS, proxy, firewall, residency, and
deployment secret isolation remain host responsibilities.

## Supported request contract

The adapter advertises:

- bounded SSE streaming;
- text and JSON input;
- strict server-authored custom tools and parallel tool calls;
- protected `StatelessConversation` continuation; and
- JSON-schema structured output through `output_config.format`.

Every request must include `maximum_output_tokens`. Application tools retain
the exact server-offered provider name, local tool ID, descriptor fingerprint,
arguments, and result identity. Contiguous tool results are mapped to one
Anthropic user turn, but every result still requires its own fresh disclosure
manifest in the provider-neutral request context. The adapter never accepts a
model-authored tool definition or arbitrary GraphQL document.

### Stop reasons

`end_turn`, `tool_use`, `refusal` and `stop_sequence` complete the response as
the provider ended it. Tool requests are held until the stop reason arrives and
are released, in order, only with `tool_use`; a request whose arguments are not
a whole JSON object is refused with any of these stop reasons.

`max_tokens`, `model_context_window_exceeded` and `pause_turn` mean the
provider spent the tokens and ended the turn early. The turn still completes
with its usage settled exactly, so the run ends `Completed` rather than failing
after the spend. The adapter appends one visible notice to the answer: the
response stopped at the output limit, stopped because the context window was
full, or was paused, and so may be incomplete. Any tool request in such a turn
is dropped and never dispatched, because its arguments may be cut off; the
notice then says that no tool was run. Anthropic normally pauses only while a
server tool runs. This adapter offers none, so it cannot resume a paused turn
as-is and ends it the same way; the user can send a follow-up to continue.

Any other stop reason is refused.

### Response model identifier

Anthropic answers a request for a model alias with the dated snapshot the
alias currently points at. The response model is accepted when it is the
requested identifier, the requested identifier followed by a date
(`-YYYYMMDD` or `-YYYY-MM-DD`), or, for a request ending in `-latest`, the part
before `-latest` followed by such a date. A response that names any other
model, including another model that shares a prefix, is refused.

To pin an alias to one snapshot, declare it:

```rust
let provider = AnthropicProvider::new(config, secrets)?
    .with_model_snapshot("example-alias", "example-alias-20250101")?;
```

A pin is exact and replaces the dated-snapshot rule for that requested
identifier: the response must name the requested identifier or the pinned
snapshot. A newer snapshot the alias later moves to is refused until the pin
is updated. Without a pin, a request is not tied to one snapshot. Budget
reservations, pricing and usage stay keyed by the requested identifier, so a
host that requests an alias prices it at the rate of the snapshot it resolves
to; pin the alias when that rate must not change silently.

The following remain deliberately unsupported and fail closed:

- attachments and provider-persistent files;
- Anthropic server tools;
- provider-retained continuation;
- hidden or extended-thinking blocks;
- prompt-cache creation; and
- custom endpoints or arbitrary beta headers.

Unknown top-level SSE event types are preserved as bounded
`ProviderEvent::Unknown` values only during a valid active response. Unknown
content blocks/deltas, unoffered tool names, malformed partial JSON, excessive
events/arguments/tool calls, incomplete streams, and unsupported stop reasons
are rejected.

## Usage and pricing

Anthropic defines total input as uncached input plus cache-created and
cache-read input. The crate's authoritative ledger currently represents total
input and a cached-read subset, but not Anthropic's separately priced cache
creation class. The adapter therefore emits no cache-control directive and
rejects any response reporting a nonzero cache write. A cache read is included
in total input and reported as the cached subset so an exact route-specific
cached-input price can settle it.

This restriction is an accounting security boundary, not merely a missing
optimization. Prompt caching can be added only with a distinct immutable
billable-unit/rate contract, reservation ceiling, usage observation, migration,
and restore validation.

## Testing

Automated tests use a bounded IPv4-loopback mock that verifies request headers,
body mapping, SSE state normalization, usage, and safe HTTP/stream failure
classification. They use only a synthetic key and make no Anthropic request.
No database is required beyond the repository's ordinary in-memory SQLite
suite.
