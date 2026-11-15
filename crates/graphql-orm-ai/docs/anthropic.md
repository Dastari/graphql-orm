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

`end_turn`, `tool_use`, `refusal`, `max_tokens` and `stop_sequence` complete
the response. A `max_tokens` or `stop_sequence` stop keeps the text and usage
already streamed; the provider-neutral event stream does not yet carry a finish
reason, so a truncated answer looks like a completed one to the caller.

`pause_turn` is refused. Anthropic emits it only while a server tool is
running, and the adapter rejects every request that offers one, so a paused
turn cannot be the answer to a request this adapter sent. Any other stop reason
is refused too.

### Response model identifier

By default the model named in the response must equal the requested string
exactly. Anthropic answers a request for a model alias with the dated snapshot
the alias currently points at, so an alias request is refused after its tokens
were spent. Either request the dated snapshot directly, or declare the one
snapshot an alias may resolve to:

```rust
let provider = AnthropicProvider::new(config, secrets)?
    .with_model_snapshot("example-alias", "example-alias-20250101")?;
```

The mapping accepts exactly one extra response identifier for one requested
identifier. A response that names any other model is still refused, including a
newer snapshot the alias later moves to; update the mapping deliberately when
that happens. Budget reservations, pricing and usage stay keyed by the
requested identifier, so declaring a mapping asserts that the host prices the
alias at the pinned snapshot's rate.

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
