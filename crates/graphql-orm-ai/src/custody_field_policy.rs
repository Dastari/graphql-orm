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
///
/// Matching by entity name alone is safe because of where this policy can be
/// reached: it is installed only on the crate-internal handle built by
/// [`custody_database`], which services never hand out (`database()` returns
/// the host's handle), and it answers only the repository-surface decisions,
/// which carry no request. A host entity that happens to share a name with one
/// of these records is written through the host's handle and stays under the
/// host's policy.
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

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use crate::persistence::{AiBudgetReservationRecord, AiRunRecord, AiSessionRecord};
    use graphql_orm::graphql::errors::OrmErrorCode;
    use graphql_orm::graphql::orm::Entity;

    /// A host policy that refuses every field decision on every surface.
    struct DenyEverything;
    impl FieldPolicy<DefaultWriteBackend> for DenyEverything {
        fn can_read_field<'a>(
            &'a self,
            _: &'a async_graphql::Context<'_>,
            _: &'a Database<DefaultWriteBackend>,
            _: &'static str,
            _: &'static str,
            _: Option<&'static str>,
            _: Option<&'a (dyn Any + Send + Sync)>,
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(false) })
        }
        fn can_write_field<'a>(
            &'a self,
            _: &'a async_graphql::Context<'_>,
            _: &'a Database<DefaultWriteBackend>,
            _: &'static str,
            _: &'static str,
            _: Option<&'static str>,
            _: Option<&'a (dyn Any + Send + Sync)>,
            _: Option<&'a (dyn Any + Send + Sync)>,
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(false) })
        }
        fn can_read_repository_field<'a>(
            &'a self,
            _: Option<AccessContext<'a>>,
            _: &'a Database<DefaultWriteBackend>,
            _: &'static str,
            _: &'static str,
            _: Option<&'static str>,
            _: Option<&'a (dyn Any + Send + Sync)>,
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(false) })
        }
        fn can_write_repository_field<'a>(
            &'a self,
            _: Option<AccessContext<'a>>,
            _: &'a Database<DefaultWriteBackend>,
            _: &'static str,
            _: &'static str,
            _: Option<&'static str>,
            _: Option<&'a (dyn Any + Send + Sync)>,
            _: Option<&'a (dyn Any + Send + Sync)>,
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(false) })
        }
    }

    async fn host() -> Database<DefaultWriteBackend> {
        let mut database = Database::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        database.set_field_policy(DenyEverything);
        database
    }

    /// Every field of the crate's own entities is granted on the repository
    /// surface, including the progress stamps recovery writes through
    /// ordinary updates, and nothing else is.
    #[tokio::test]
    async fn custody_covers_every_field_of_the_crates_own_entities_and_nothing_else() {
        let host = host().await;
        let custody = custody_database(host.clone());
        let entities = [
            (
                AiRunRecord::entity_name(),
                AiRunRecord::repository_field_policies(),
                Some("recoveryExaminedAt"),
            ),
            (
                AiBudgetReservationRecord::entity_name(),
                AiBudgetReservationRecord::repository_field_policies(),
                Some("settlementExaminedAt"),
            ),
            (
                AiSessionRecord::entity_name(),
                AiSessionRecord::repository_field_policies(),
                None,
            ),
        ];
        for (entity, fields, stamp) in entities {
            assert!(is_custody_entity(entity), "{entity}");
            if let Some(stamp) = stamp {
                assert!(
                    fields.iter().any(|field| field.api_name == stamp),
                    "{entity} has the stamp field {stamp}"
                );
            }
            for field in fields {
                assert_eq!(field.write_policy, None, "{entity}.{}", field.api_name);
                custody
                    .ensure_repository_writable_field(
                        None,
                        entity,
                        field.api_name,
                        field.write_policy,
                        None,
                        None,
                    )
                    .await
                    .unwrap_or_else(|_| panic!("custody writes {entity}.{}", field.api_name));
                custody
                    .ensure_repository_readable_field(
                        None,
                        entity,
                        field.api_name,
                        field.read_policy,
                        None,
                    )
                    .await
                    .unwrap_or_else(|_| panic!("custody reads {entity}.{}", field.api_name));
                // The host's own handle keeps the host's answer.
                let refused = host
                    .ensure_repository_writable_field(
                        None,
                        entity,
                        field.api_name,
                        None,
                        None,
                        None,
                    )
                    .await
                    .expect_err("the host handle is not granted");
                assert_eq!(refused.code, OrmErrorCode::Forbidden);
            }
        }

        // An entity the crate does not own, and a declared key on one it
        // does, stay with the host policy.
        for (entity, key) in [
            ("HostEntity", None),
            (AiRunRecord::entity_name(), Some("host.declared.key")),
        ] {
            let refused = custody
                .ensure_repository_writable_field(None, entity, "anyField", key, None, None)
                .await
                .expect_err("delegated to the host policy");
            assert_eq!(refused.code, OrmErrorCode::Forbidden, "{entity}");
        }
    }

    #[tokio::test]
    async fn a_database_without_a_field_policy_is_used_unchanged() {
        let database = Database::<DefaultWriteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        assert!(custody_database(database).field_policy().is_none());
    }
}
