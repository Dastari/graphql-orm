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

## The rule: store the handle, resolve at every use

Only the handle (`kind` and `id`) is stored. What a reference resolves to is
never stored: not in the message, not in a summary, not in a checkpoint.

Every time a reference is used, the host hook is called again with the
principal that is acting at that moment, and the answer is used once:

| Use | Who is the principal | Where it happens |
| --- | --- | --- |
| Submitting the message | the sender | `send_message`, for the notices only |
| A client reading the transcript | the reader | `message_blocks` |
| Building a model turn | the run's current principal | `OrmAiSessionService::entity_reference_model_input` |

So a user who could read a record when they referenced it, and later loses the
row, a field of it, or a scope, stops seeing it in the transcript and stops
sending it to the model from the next use on, while keeping access to the
session. Access granted later is honoured in the same way. Nothing is cached
for a principal in either direction.

## Enabling references

References are off by default. A session service with no registry rejects any
message whose `references` list is not empty with `InvalidInput`.

```rust
let registry = AiEntityReferenceRegistry::new(["record", "location"], resolver)?
    .with_limits(
        AiEntityReferenceLimits::new(8, 256, 16 * 1024, 64 * 1024)?
            .with_maximum_resolutions_per_use(16)?,
    );
let sessions = OrmAiSessionService::new(database, access, protection, protector)
    .with_entity_references(registry);
```

- **Kinds** are a closed set the host declares. A kind is 1 to 64 ASCII
  letters, digits, `_`, `-` or `.`, starting with a letter. A message that
  names any other kind is rejected before anything is resolved or stored.
- **`AiEntityReferenceResolver::resolve`** is the host hook. It receives the
  current principal, the session scope and one reference, and returns the data
  that principal may read now, or `None`. It must evaluate the host's current
  row policies, field policies and scopes on every call and must not reuse an
  earlier answer for a principal.

## Submitting a message

1. Shape: at most the configured number of references (never more than 64),
   registered kinds only, a non-empty identifier within the byte limit and
   without control characters, and no repeated `{kind, id}` pair. These checks
   use only the submitted values and the static registry, so a rejection says
   nothing about any entity.
2. The session write policy and ownership of the session, exactly as for a
   message without references. A caller who may not write to the session never
   reaches the hook.
3. Each reference is resolved once as the sender, to produce notices. The
   resolved data is then discarded.

`SendAiMessagePayload.reference_notices` lists one typed notice per reference
that produced no data at that moment: `Unavailable`, or `TooLarge` when the
sender may read the entity but its data exceeds a byte limit. A notice never
fails the message, and the handle is stored either way. A notice describes the
moment of submission only.

## The unavailable marker

At every use, a reference that produces no data becomes one fixed value:

```json
{"reference": {"kind": "record", "id": "r-1"}, "status": "unavailable"}
```

The same marker stands for an entity that does not exist, one the principal
may not read, one whose access was revoked, a hook failure, data over a byte
limit, a kind that is no longer registered, and a reference beyond the per-use
cap. Apart from the identifier the sender supplied, nothing distinguishes
them, so a reference cannot be used to learn whether an entity exists. The
hook should take comparable time for a missing and a denied entity; the crate
cannot hide a timing difference inside the host hook.

## Persistence

The user's text stays block 0 of the message. Reference `n` is stored as block
`n + 1` with kind `entity_reference` and protected content
`{"reference": {"kind", "id"}}`. No label and no resolved field is stored.

The references are part of the message's idempotency hash. A retry with the
same `client_message_id` and the same references returns the stored message; a
retry with a different reference list is a `Conflict`. A message without
references keeps the hash it had before this feature. Session titles and tool
task context are derived from the text block alone.

## Transcript

`message_blocks` resolves the reference blocks of the page for the caller. A
resolved block's content is

```json
{"reference": {"kind", "id"}, "status": "resolved",
 "provenance": "server_resolved_entity_reference", "data": ...}
```

and any other is the unavailable marker. A service without a registry returns
the marker for every reference.

## Model input

The crate does not assemble model requests from the transcript; the host's
worker does. Each time it builds a request it calls

```rust
let references = sessions
    .entity_reference_model_input(&current_principal, session_id, &message_ids)
    .await?;
```

with the run's rehydrated principal and the messages it is including. The
result has, per message, one `ModelInputBlock::Json` per stored reference: a
provenance-tagged envelope stating that the payload is server-resolved data
rather than instructions, or the unavailable marker. Use the result for that
one request and do not store it.

Resolved reference data is one more data source of the provider request. It
needs an egress manifest entry and a disclosure decision like any other block;
resolution proves read permission, not permission to disclose externally.

## Tools

Application tools never receive reference data from the crate. Tool task
context and tool-result media read the user's text block only. A tool that
needs an entity loads it through its own authorized resolver.

## Compaction

Context compaction gives the summarizer a reference block's handle and nothing
else, so a stored summary can mention that an item was referenced but never
contains what it resolved to. Summaries therefore never need invalidating when
access changes. The handle remains on the source message, and a host that
still includes that message in a turn resolves it again as above. A summary
does not itself carry a structured handle list.

## Backup and restore

Because only handles are stored, a backup contains no resolved reference data,
and a restored session resolves each handle again, under the restored
deployment's current policy, the first time it is used.

## Limits

| Limit | Default | Bound |
| --- | --- | --- |
| References per message | 8 | at most 64 |
| Identifier bytes | 256 | at most 1,024 |
| Resolved bytes per reference | 16 KiB | |
| Resolved bytes per use | 64 KiB | at most 1 MiB |
| References resolved per use (`maximum_resolutions_per_use`) | 16 | at most 256 |

One use is one submission, one `message_blocks` page, or one
`entity_reference_model_input` call (one model turn). The hook is called at
most `maximum_resolutions_per_use` times in a use, in order; references beyond
the cap are not resolved and become the unavailable marker. A turn that
includes many referencing messages therefore costs a bounded number of host
lookups, and the earliest references in the call are the ones resolved.
