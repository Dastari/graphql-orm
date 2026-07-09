# Cross-Backend Tenant Isolation Guide

PostgreSQL RLS is defense in depth. Tenant safety must also hold on SQLite,
MSSQL, repository APIs, and DataLoader batches.

## Structural Predicates

Use the runtime helpers in `graphql_orm::graphql::structural_auth`:

```rust
use graphql_orm::prelude::*;

let metadata = StructuralAuthMetadata::new(
    Some("tenant_id"),
    Some("owner_id"),
    StructuralAuthorization::Required,
);
let values = StructuralAuthValues::from_subject(&subject);
match resolve_structural_auth(metadata, &values) {
    StructuralAuthResolution::Filter(filter) => {
        // AND into list/count/update/delete queries before pagination
    }
    StructuralAuthResolution::DeniedMissingContext => {
        return Err(OrmPublicError::forbidden().into_graphql_error());
    }
    StructuralAuthResolution::None => {}
}
```

Predicates are parameterized (`tenant_id = ?`). Never interpolate tenant values
into SQL text.

## Requirements

- Apply constraints inside SQL before pagination.
- Never fetch all rows and filter authorization only in memory.
- Missing tenant under `StructuralAuthorization::Required` denies before query execution.
- Relation loaders should carry `DbAuthContext` so cache partitions cannot cross tenants.

## PostgreSQL Parity

Attach both `AuthSubject` and `DbAuthContext`:

```rust
let request = request
    .data(subject.clone())
    .data(DbAuthContext::from_subject(&subject));
```

RLS policies and structural predicates should encode the same tenant/owner
rules. Prefer parity tests on every backend your service enables.
