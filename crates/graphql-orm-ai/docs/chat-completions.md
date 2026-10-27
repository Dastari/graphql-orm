---
title: "Chat Completions-compatible endpoints"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-10-25
review_by: 2027-02-01
supersedes: []
---

# Chat Completions-compatible endpoints

The `provider-chat-completions` feature (or `provider-chat-completions-core`
with one `provider-tls-*` feature) adds `ChatCompletionsProvider` for servers
that expose only `POST /v1/chat/completions` with streamed
`chat.completion.chunk` events: Ollama's OpenAI endpoint, LM Studio, vLLM,
llama.cpp server, LocalAI and many gateways.

## Profile

A profile uses provider kind `chat_completions` (`AiProviderKindInput::ChatCompletions`)
and the same reviewed contract as an OpenAI-compatible profile: retention
label, `custom_tools`, `parallel_tool_calls` and `structured_output`.
`provider_retained_continuation` must be `false`; the configuration service
refuses anything else. The exact endpoint is set as the profile `base_url` and
must be approved by `AiProviderEndpointPolicy::authorize_endpoint` with kind
`ChatCompletions`. A credential is optional: without one no `Authorization`
header is sent, which suits local servers. Build the adapter with
`ChatCompletionsProviderConfig::from_profile(view, credential)` and
`ChatCompletionsProvider::new(config, endpoint_policy, secrets)`.

Capabilities are declared, never probed. Every egress manifest must bind the
profile ID, the normalized endpoint and the retention label, like the
OpenAI-compatible adapter. Redirects are disabled and `Debug` output redacts
the endpoint and credential.

## Requests and streaming

Every turn is a stateless replay (`ModelContinuationMode::StatelessReplay` for
tool turns): instructions become a `system` message, then the bounded
conversation is replayed as `user`, `assistant` (with `tool_calls`) and `tool`
messages. Requests always set `stream: true`, `stream_options.include_usage:
true` and `max_tokens`; tools are strict functions, with `parallel_tool_calls`
only as declared; structured output uses `response_format: json_schema`.
Attachments, provider built-ins, reasoning summaries and provider-retained
continuation are refused.

Content deltas become `TextDelta`. Tool-call fragments are assembled by their
`index` into `ToolCallStarted`, `ToolArgumentsDelta` and `ToolCallCompleted`,
within the adapter's call-count and argument-size bounds. A second call without
declared parallel tool calls, a rename, malformed arguments, or a
`finish_reason` other than `stop` or `tool_calls` (for example `length`) is a
protocol violation. Call IDs from the server are used when valid; a missing ID
is replaced by a generated opaque one.

## Usage

Settlement requires authoritative usage. A stream that ends without a `usage`
chunk fails the turn with `ProviderError::Rejected` and a warning log, rather
than settling on an estimate. Use an endpoint that honours
`stream_options.include_usage`.

## Errors

HTTP status mapping matches the Responses adapter: 401 and 403 are
`CredentialUnavailable`, 429 is `RateLimited`, 5xx is `Unavailable`, and any
other failure is `Rejected`. Response bodies are never echoed.
