//! Transaction-bound dependent delete verification for external effect policies.
use std::collections::BTreeMap;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

/// One externally admitted dependent write, identified by public entity and field.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum DependentDeleteEffect {
    /// Delete rows of this entity.
    Delete(String),
    /// Clear this public field on rows of this entity.
    SetNull(String, String),
}

/// Request-local, one-delete effect guard. Attach a clone to GraphQL request
/// data and verify [`Self::was_committed`] after execution. Only the ORM's
/// introspected, transaction-bound planner can mark it committed. Ordinary
/// physical deletes and bulk deletes refuse this guard (unknown-effect fallback).
/// It grants no GraphQL authority and never replaces dependent authorization.
#[derive(Clone, Debug)]
#[cfg_attr(not(any(feature = "sqlite", feature = "postgres")), allow(dead_code))]
pub struct DependentDeleteGuard {
    pub(super) root: String,
    pub(super) effects: BTreeMap<DependentDeleteEffect, Option<u32>>,
    committed: Arc<AtomicBool>,
}
#[cfg_attr(not(any(feature = "sqlite", feature = "postgres")), allow(dead_code))]
impl DependentDeleteGuard {
    /// Admit exactly this root and complete transitive effect set. Bounds are
    /// counted on the locked execution plan before any write; excess rolls back.
    pub fn new(root: String, effects: BTreeMap<DependentDeleteEffect, Option<u32>>) -> Self {
        Self { root, effects, committed: Arc::new(AtomicBool::new(false)) }
    }
    /// Whether the ORM verified this request's effect plan and committed it.
    pub fn was_committed(&self) -> bool { self.committed.load(Ordering::Acquire) }
    pub(super) fn check(&self, root: &str, counts: &BTreeMap<DependentDeleteEffect, usize>) -> async_graphql::Result<()> {
        if self.was_committed() || root != self.root || counts.len() != self.effects.len()
            || counts.iter().any(|(effect, count)| self.effects.get(effect).is_none_or(|bound| bound.is_some_and(|maximum| *count > maximum as usize))) {
            return Err(async_graphql::Error::new("Dependent delete effect refused"));
        }
        Ok(())
    }
    pub(super) fn committed(&self) { self.committed.store(true, Ordering::Release); }
}
