# Schema Modules and Fenced Leases

Version 0.7 adds project-agnostic primitives for dependencies that own private
persistence and durable workers. They do not expose GraphQL roots, apply
migrations automatically, or grant database authority.

## Dependency-Owned Schema Modules

Implement `OrmSchemaModule` with a stable descriptor and the entity metadata
owned by the dependency:

```rust
use graphql_orm::graphql::orm::{
    EntityMetadata, OrmSchemaModule, SchemaModuleDescriptor,
    SchemaModuleRestoreHook, SchemaModuleRestorePhase,
};

static DESCRIPTOR: SchemaModuleDescriptor =
    SchemaModuleDescriptor::new("com.example.activity", "1.0.0", "activity_");

static RESTORE_HOOKS: [SchemaModuleRestoreHook; 1] = [SchemaModuleRestoreHook {
    hook_id: "reconcile-runtime",
    phase: SchemaModuleRestorePhase::Reconcile,
}];

struct ActivityModule;

impl OrmSchemaModule for ActivityModule {
    fn descriptor(&self) -> &SchemaModuleDescriptor {
        &DESCRIPTOR
    }

    fn entities(&self) -> &[&'static EntityMetadata] {
        activity_entity_metadata()
    }

    fn restore_hooks(&self) -> &[SchemaModuleRestoreHook] {
        &RESTORE_HOOKS
    }
}
```

`SchemaModuleCatalog::compose` rejects duplicate module/table ownership,
overlapping namespaces, out-of-namespace tables, invalid restore-hook IDs, and
fingerprint drift. Hosts use its schema model with their normal reviewed
migration workflow and include `SchemaModulesSnapshot` in backup metadata.

The fingerprint detects structural or declaration drift; it is not a
cryptographic signature. The host remains responsible for authenticating and
authorizing migration and restore operations.

## Restore Phases

Modules may declare preflight, reconcile, validate, and readiness hooks. The
declaration is metadata only: the owning dependency supplies the actual hook
implementation. A restored runtime should remain closed until reconciliation,
validation, and readiness checks succeed.

## Fenced Lease Transitions

`FencedLeaseState` models the state transition and produces a `LeaseProof` that
binds the resource, worker owner, attempt, and monotonic fencing token. Every
persistent claim, heartbeat, child append, terminal write, or release must also
compare the unexpired deadline and expected CAS row version atomically.

Never implement a claim as a read followed by an unconditional update. A stale
worker can retain an old in-memory `LeaseProof`; only the complete atomic
database predicate prevents it from committing after another worker reclaims
the resource.

Transition errors do not partially mutate the in-memory state. Database
implementations must provide the same all-or-nothing behavior through their
transaction and compare-and-swap facilities.
