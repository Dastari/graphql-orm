# Public ORM Error-Code Contract

GraphQL clients receive only stable codes and safe messages. SQL text, table names,
constraint names, filesystem paths, and configuration strings stay server-side.

## Codes

| Code | Meaning |
| --- | --- |
| `INVALID_INPUT` | Client input failed validation |
| `UNAUTHENTICATED` | No principal present |
| `FORBIDDEN` | Principal present but not authorized |
| `NOT_FOUND` | Resource missing or not visible (no cross-tenant existence leak) |
| `CONFLICT` | State conflict |
| `CONSTRAINT_VIOLATION` | Database constraint failed without schema leak |
| `CURSOR_INVALID` | Pagination cursor invalid/tampered/version mismatch |
| `PAGE_LIMIT_EXCEEDED` | Requested page size exceeds configured max |
| `SERVICE_UNAVAILABLE` | Temporary dependency failure |
| `INTERNAL_ERROR` | Unexpected failure |
| `AUTHORIZATION_MISCONFIGURED` | Strict mode missing required policy provider |

## GraphQL Extensions

Default extensions:

```json
{
  "code": "FORBIDDEN",
  "correlationId": "optional-host-supplied-id"
}
```

Internal diagnostic strings are never placed in extensions. Set
`GRAPHQL_ORM_LOG_INTERNAL_ERRORS=1` to emit internal detail to stderr for local
debugging.

## Before / After

```rust
// Before (leaks infrastructure)
Err(async_graphql::Error::new(sqlx_error.to_string()))

// After
Err(OrmPublicError::from_sqlx(&sqlx_error).into_graphql_error())
// or
Err(graphql_error_from_sqlx(sqlx_error))
```
