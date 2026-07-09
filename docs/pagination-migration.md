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

## Cursor Model

Generated connections still use offset-style cursors (`encode_cursor(offset)`).
Base64 alone is not a security boundary. A versioned keyset pagination path is
planned; until then:

- clamp limits with `PaginationConfig`
- treat cursors as opaque offsets, not secrets
- do not expose unbounded GraphQL lists

## Field Selection And Counts

Prefer omitting expensive `totalCount` work when the client does not select it.
Repository `fetch_all` remains intentionally unbounded for trusted internal use.
