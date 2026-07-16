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

Runtime-schema record/handle errors use the separate lowercase
`RuntimeRecordErrorCode` contract because they are host-side runtime IR errors,
not GraphQL responses. The categories include unknown/cross-collection/stale
handles, unloaded/null/wrong-kind values, missing columns, backend type
mismatches, malformed portable values, unsupported backends, and invalid
serialized records. Safe formatting includes stable IDs only; the standard
error source retains driver detail for trusted logging. See
[Runtime records](runtime-records.md#stable-errors).

Runtime query validation and execution use `RuntimeQueryErrorCode`: `invalid_handle`,
`invalid_request`, `invalid_filter`, `unsupported_operator`, `unsupported_order`,
`unsupported_backend`, `resource_limit`, `cursor_invalid`, `cursor_schema_mismatch`, `decode`, and
`backend_execution`. Safe formatting omits SQL, physical identifiers, bound values, cursor contents,
and backend detail; the standard error source remains available to trusted logs. See
[runtime queries](runtime-queries.md).

## Before / After

```rust
// Before (leaks infrastructure)
Err(async_graphql::Error::new(sqlx_error.to_string()))

// After
Err(OrmPublicError::from_sqlx(&sqlx_error).into_graphql_error())
// or
Err(graphql_error_from_sqlx(sqlx_error))
```
