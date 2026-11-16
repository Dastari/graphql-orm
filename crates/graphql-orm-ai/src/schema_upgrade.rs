//! Upgrade admission for indexes that earlier AI schema module versions
//! created and the current module replaces.

use graphql_orm::graphql::orm::{
    ApplyOptions, Entity, MigrationRisk, MigrationStep, PlannedMigration, SchemaModel,
};

use crate::persistence::{AiProviderWebhookReceiptRecord, AiSchemaModule};

/// Webhook-receipt indexes that earlier AI schema module versions created and
/// the current module replaces with its bounded composite index on
/// `(provider_kind, provider_profile_id, provider_response_id, state)`.
const REPLACED_WEBHOOK_RECEIPT_INDEXES: [&str; 3] = [
    // The single-column filter index from before `provider_kind` joined the
    // primary key. The composite index leads with the same column.
    "idx_graphql_orm_ai_provider_webhook_receipts_provider_kind",
    // Module 0.72.0's composite index under its full name (SQLite, SQL Server)
    // and as PostgreSQL stored it, truncated to 63 bytes.
    "idx_graphql_orm_ai_provider_webhook_receipts_provider_kind_provider_profile_id_provider_response_id_state",
    "idx_graphql_orm_ai_provider_webhook_receipts_provider_kind_prov",
];

const WEBHOOK_RECEIPT_COMPOSITE_INDEX_COLUMNS: [&str; 4] = [
    "provider_kind",
    "provider_profile_id",
    "provider_response_id",
    "state",
];

impl AiSchemaModule {
    /// Admit the destructive steps of `plan` that only replace an index an
    /// earlier AI schema module version created.
    ///
    /// Upgrading from an older module drops an AI-owned webhook-receipt index
    /// and creates the current bounded composite index in its place. Dropping
    /// an index is [`MigrationRisk::Destructive`], so
    /// [`ApplyOptions::default`] refuses the upgrade. This returns `options`
    /// with `allow_destructive` set only when the plan has at least one
    /// destructive step, every destructive step drops one of those known
    /// legacy webhook-receipt indexes, and the same plan creates the current
    /// composite index on that table. In every other case `options` is
    /// returned unchanged, so the migration gate refuses any other
    /// destructive step, including one that a host's own schema contributes.
    ///
    /// The legacy indexes are non-unique, so dropping them removes no
    /// constraint. This checks the plan's steps only: apply the same plan it
    /// inspected, and review host-owned destructive steps separately.
    #[must_use]
    pub fn admit_index_replacements(
        &self,
        plan: &PlannedMigration,
        options: ApplyOptions,
    ) -> ApplyOptions {
        let metadata = AiProviderWebhookReceiptRecord::metadata();
        let schema = SchemaModel::from_entities(&[metadata]);
        let Some(replacement) = schema
            .tables
            .iter()
            .filter(|table| table.table_name == metadata.table_name)
            .flat_map(|table| table.indexes.iter())
            .find(|index| index.columns == WEBHOOK_RECEIPT_COMPOSITE_INDEX_COLUMNS)
        else {
            return options;
        };
        let creates_replacement = plan.steps.iter().any(|step| {
            matches!(
                &step.step,
                MigrationStep::CreateIndex { table_name, index }
                    if table_name == metadata.table_name
                        && index.name == replacement.name
                        && index.columns == replacement.columns
                        && !index.is_unique
            )
        });
        let mut destructive = plan
            .steps
            .iter()
            .filter(|step| step.risk == MigrationRisk::Destructive)
            .peekable();
        if !creates_replacement || destructive.peek().is_none() {
            return options;
        }
        let only_replacements = destructive.all(|step| {
            matches!(
                &step.step,
                MigrationStep::DropIndex { table_name, index_name }
                    if table_name == metadata.table_name
                        && REPLACED_WEBHOOK_RECEIPT_INDEXES.contains(&index_name.as_str())
            )
        });
        if only_replacements {
            ApplyOptions {
                allow_destructive: true,
                ..options
            }
        } else {
            options
        }
    }
}
