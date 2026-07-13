# Pagination Migration Guide

## What Changed In 0.3.0

| Setting | Before (≤0.2.x) | After (0.3.0 default) | Legacy restore |
| --- | --- | --- | --- |
| Default limit | 1000 | 50 | `PaginationConfig::legacy()` |
| Max limit | 1000 | 100 | `PaginationConfig::legacy()` |

```rust
// Secure defaults (crate default)
let database = Database::new(pool);

// Restore previous limits during migration
let database = Database::new(pool)
    .with_pagination_config(PaginationConfig::legacy());

// Trusted internal jobs only
let database = Database::builder(pool)
    .unbounded_pagination()
    .build();
```

## Cursor Models

Existing generated connections continue to use offset-style cursors
(`encode_cursor(offset)`). Base64 alone is not a security boundary. Entities
with an explicit keyset order also have versioned, order-fingerprint-bound
opaque keyset cursors.

Version 0.7 adds the repository-only `keyset_connection_page` helper for
bounded bidirectional windows:

```rust
let tail = Event::keyset_connection_page(
    &database,
    EventWhereInput::default(),
    KeysetConnectionInput {
        last: Some(50),
        ..Default::default()
    },
).await?;

let older = Event::keyset_connection_page(
    &database,
    EventWhereInput::default(),
    KeysetConnectionInput {
        before: tail.page_info.start_cursor,
        last: Some(50),
        ..Default::default()
    },
).await?;
```

Backward database reads are returned in the entity's canonical order. Page
sizes are clamped by `PaginationConfig`, and total counts remain explicit
opt-in work.

- treat all cursors as opaque positions, not secrets
- restart traversal when an order fingerprint changes
- prefer bounded keyset windows for deep or append-heavy timelines
- do not expose unbounded GraphQL lists

## Field Selection And Counts

Prefer omitting expensive `totalCount` work when the client does not select it.
Repository `fetch_all` remains intentionally unbounded for trusted internal use.
