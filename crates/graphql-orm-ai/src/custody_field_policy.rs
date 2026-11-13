//! Field-policy custody for the crate's own persistence.
//!
//! Generated repository reads and writes ask the host's `FieldPolicy` about
//! every field, with the field's policy key or `None`, and they pass no
//! access context. The records this crate persists are custody records it
//! owns: a host policy that denies fields it does not know, or every
//! repository write without an access context, must not stop the crate from
//! reading or writing its own rows. Every internal database handle therefore
//! carries the host policy behind [`CustodyFieldPolicy`], which answers
//! repository-surface field decisions for this crate's entities itself and
//! delegates everything else unchanged.
//!
//! The wrapped handle never leaves the crate: public `database()` accessors
//! return the handle the host supplied.

use std::any::Any;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use graphql_orm::db::Database;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::auth::AccessContext;
use graphql_orm::graphql::orm::{DefaultWriteBackend, FieldPolicy, OrmSchemaModule};

use crate::persistence::AiSchemaModule;

/// Whether `entity_name` is one of this crate's own persisted entities.
fn is_custody_entity(entity_name: &str) -> bool {
    static ENTITIES: OnceLock<HashSet<&'static str>> = OnceLock::new();
    ENTITIES
        .get_or_init(|| {
            AiSchemaModule
                .entities()
                .iter()
                .map(|entity| entity.entity_name)
                .collect()
        })
        .contains(entity_name)
}

/// The host field policy, with repository decisions for this crate's own
/// key-less fields answered by the crate.
struct CustodyFieldPolicy {
    host: Arc<dyn FieldPolicy<DefaultWriteBackend>>,
}

impl FieldPolicy<DefaultWriteBackend> for CustodyFieldPolicy {
    fn ignore_denied_field_write(
        &self,
        entity_name: &'static str,
        field_name: &'static str,
    ) -> bool {
        self.host.ignore_denied_field_write(entity_name, field_name)
    }

    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        db: &'a Database<DefaultWriteBackend>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        // GraphQL surfaces always belong to the host policy.
        self.host
            .can_read_field(ctx, db, entity_name, field_name, policy_key, record)
    }

    fn can_write_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        db: &'a Database<DefaultWriteBackend>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
        value: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        self.host
            .can_write_field(ctx, db, entity_name, field_name, policy_key, record, value)
    }

    fn can_read_repository_field<'a>(
        &'a self,
        access: Option<AccessContext<'a>>,
        db: &'a Database<DefaultWriteBackend>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        // A declared key is the host's to decide; this crate declares none.
        if policy_key.is_none() && is_custody_entity(entity_name) {
            return Box::pin(async { Ok(true) });
        }
        self.host
            .can_read_repository_field(access, db, entity_name, field_name, policy_key, record)
    }

    fn can_write_repository_field<'a>(
        &'a self,
        access: Option<AccessContext<'a>>,
        db: &'a Database<DefaultWriteBackend>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
        value: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        if policy_key.is_none() && is_custody_entity(entity_name) {
            return Box::pin(async { Ok(true) });
        }
        self.host.can_write_repository_field(
            access,
            db,
            entity_name,
            field_name,
            policy_key,
            record,
            value,
        )
    }
}

/// Returns the crate's internal handle for a host database.
///
/// Without a host field policy the handle is returned unchanged: this
/// crate's entities declare no field policy key, and key-less fields are
/// readable and writable without a provider in every authorization mode.
pub(crate) fn custody_database(
    mut database: Database<DefaultWriteBackend>,
) -> Database<DefaultWriteBackend> {
    if let Some(host) = database.field_policy().cloned() {
        database.set_field_policy(CustodyFieldPolicy { host });
    }
    database
}
