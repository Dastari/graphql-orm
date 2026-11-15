---
title: "Per-Message Entity References"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-11-15
review_by: 2027-02-01
supersedes: []
---

# Per-Message Entity References

A user message can carry a short list of structured references to host
entities, such as "this record" or "that location". Each reference is a
`{kind, id}` pair on `SendAiMessageInput.references`. References are bound to
the one message that carries them, unlike per-session context.

A reference is a request to look something up. It never grants access, and it
is never treated as an instruction.

## Enabling references

References are off by default. A session service with no registry rejects any
message whose `references` list is not empty with `InvalidInput`.

```rust
let registry = AiEntityReferenceRegistry::new(["record", "location"], resolver)?
    .with_limits(AiEntityReferenceLimits::new(8, 256, 16 * 1024, 64 * 1024)?);
let sessions = OrmAiSessionService::new(database, access, protection, protector)
    .with_entity_references(registry);
```

- **Kinds** are a closed set the host declares. A kind is 1 to 64 ASCII
  letters, digits, `_`, `-` or `.`, starting with a letter. A message that
  names any other kind is rejected before anything is resolved or stored.
- **`AiEntityReferenceResolver::resolve`** is the host hook. It receives the
  sending principal, the session scope and one reference, and returns the data
  that principal may read, or `None`.

## What the service checks, in order

1. Shape: at most the configured number of references (never more than 64),
   registered kinds only, a non-empty identifier within the byte limit and
   without control characters, and no repeated `{kind, id}` pair. These checks
   use only the submitted values and the static registry, so a rejection says
   nothing about any entity.
2. The session write policy and ownership of the session, exactly as for a
   message without references. A caller who may not write to the session never
   reaches the resolver.
3. Resolution, one reference at a time, as the sending principal.

## Outcomes

| Resolver result | Stored block kind | Notice |
| --- | --- | --- |
| `Ok(Some(data))` within the limits | `entity_reference` | none |
| `Ok(Some(data))` over the per-reference or per-message byte limit | `entity_reference_too_large` | `TooLarge` |
| `Ok(None)` | `entity_reference_unavailable` | `Unavailable` |
| `Err(_)` | `entity_reference_unavailable` | `Unavailable` |

A dropped reference never fails the message. `SendAiMessagePayload.reference_notices`
lists one typed notice per dropped reference, in submission order.

`Unavailable` covers an entity that does not exist, one the sender may not
read, and a resolver failure. The stored block and the notice are identical in
all three cases apart from the identifier the sender supplied, so the sender
cannot use a reference to learn whether an entity exists. `TooLarge` is only
reported for data the sender was allowed to read. The resolver should take
comparable time for a missing and a denied entity; the crate cannot hide a
timing difference inside the host hook.

## Persistence

The user's text stays block 0 of the message. Reference `n` is stored as block
`n + 1`, protected like any other block:

- resolved: `{"reference": {"kind", "id"}, "status": "resolved",
  "provenance": "server_resolved_entity_reference", "data": ...}`;
- dropped: `{"reference": {"kind", "id"}, "status": "unavailable"}` or
  `"too_large"`, with no data.

The transcript therefore shows what was referenced, including references that
produced nothing. `AiMessageBlockView.kind` exposes the three block kinds to
clients. Session titles and tool task context are still derived from the text
block alone.

The references are part of the message's idempotency hash. A retry with the
same `client_message_id` and the same references returns the stored message
and the notices recorded when it was accepted; the resolver's current answer
does not change them. A retry with a different reference list is a `Conflict`.
A message without references keeps the hash it had before this feature.

## Model input

The crate does not assemble model requests from the transcript; the host's
worker does. For each stored block, `entity_reference_model_input(kind,
content)` returns a `ModelInputBlock::Json` for a resolved reference and `None`
for everything else. The envelope names the reference, carries the provenance
tag, and states that the payload is server-resolved data rather than
instructions. Dropped references produce no model input.

Resolved reference data is one more data source of the provider request. It
needs an egress manifest entry and a disclosure decision like any other block;
resolution proves read permission, not permission to disclose externally.

## Limits

`AiEntityReferenceLimits` defaults to 8 references per message, 256 bytes per
identifier, 16 KiB of serialized data per reference and 64 KiB per message.
The count cannot exceed 64 and the per-message total cannot exceed 1 MiB.
