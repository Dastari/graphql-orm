//! Upgrade admission for indexes that earlier AI schema module versions
//! created and the current module replaces.

use graphql_orm::graphql::orm::{
    ApplyOptions, Entity, EntityMetadata, IndexDef, IntrospectionBackend, MigrationRisk,
    MigrationStep, OrmSchemaModule, PlannedMigration, SchemaModel,
};

use graphql_orm::prelude::Database;

use crate::persistence::{AiApprovalRecord, AiProviderWebhookReceiptRecord, AiSchemaModule};

const LEGACY_APPROVAL_INDEX: IndexDef = IndexDef::new(
    "idx_graphql_orm_ai_approvals_tool_call_id",
    &["tool_call_id"],
);

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
    /// Use [`Self::admit_index_replacements_with_live_schema`] when the plan
    /// replaces the legacy approval lookup index.
    ///
    /// The check matches a dropped index by table and name only, because a
    /// drop step carries no index definition. Earlier module versions created
    /// these names as non-unique indexes, but the check does not verify that
    /// the live index with that name is non-unique: a host that created its
    /// own index under one of these names must review the plan itself. This
    /// checks the plan's steps only: apply the same plan it inspected, and
    /// review host-owned destructive steps separately.
    #[must_use]
    pub fn admit_index_replacements(
        &self,
        plan: &PlannedMigration,
        mut options: ApplyOptions,
    ) -> ApplyOptions {
        options.allow_destructive =
            options.allow_destructive || only_index_replacements(plan, false);
        options
    }

    /// Admit legacy AI index replacements using the source database for verification.
    ///
    /// In addition to the webhook replacements handled by
    /// [`Self::admit_index_replacements`], this admits the pre-0.83 approval
    /// lookup index drop when the same plan creates the current unique
    /// `tool_call_id` index on the approvals table. The plan must still match
    /// the live source schema, and the legacy index must match by table, name,
    /// key columns and non-uniqueness. All destructive steps must be supported
    /// replacements for this method to enable `allow_destructive`.
    ///
    /// Plan to the module's entities and pass the unmodified plan to the
    /// migration runner. Use the same database with a connection that sees all affected
    /// rows, and quiesce writes and other schema changes from planning through
    /// application. SQLite and PostgreSQL hosts can pass the result directly
    /// to managed apply. SQL Server hosts can use it to check a reviewed plan
    /// before executing it transactionally with external migration tooling.
    ///
    /// # Errors
    ///
    /// Returns an error if the source schema cannot be read or if preflight
    /// detects duplicate unique keys or cannot complete. No DDL is executed.
    pub async fn admit_index_replacements_with_live_schema<B: IntrospectionBackend>(
        &self,
        database: &Database<B>,
        plan: &PlannedMigration,
        mut options: ApplyOptions,
    ) -> graphql_orm::Result<ApplyOptions> {
        if !plan
            .steps
            .iter()
            .any(|step| is_approval_lookup_drop(&step.step))
        {
            return Ok(self.admit_index_replacements(plan, options));
        }

        if plan.backend != B::DIALECT.name() || !only_index_replacements(plan, true) {
            return Ok(options);
        }
        let declared_tables = self
            .entities()
            .iter()
            .map(|entity| entity.table_name)
            .collect::<Vec<_>>();
        let live = B::introspect_schema_including(database.pool(), &declared_tables).await?;
        if plan.source_schema_hash.as_ref() != Some(&live.stable_hash())
            || !live.tables.iter().any(|table| {
                table.table_name == AiApprovalRecord::metadata().table_name
                    && table.indexes.contains(&LEGACY_APPROVAL_INDEX)
            })
        {
            return Ok(options);
        }
        database.schema().preflight_unique_indexes(plan).await?;
        options.allow_destructive = true;
        Ok(options)
    }
}

fn is_approval_lookup_drop(step: &MigrationStep) -> bool {
    matches!(
        step,
        MigrationStep::DropIndex { table_name, index_name }
            if table_name == AiApprovalRecord::metadata().table_name
                && index_name == LEGACY_APPROVAL_INDEX.name
    )
}

fn creates_replacement(
    plan: &PlannedMigration,
    entity: &'static EntityMetadata,
    columns: &[&str],
    unique: bool,
) -> bool {
    let schema = SchemaModel::from_entities(&[entity]);
    let Some(replacement) = schema.tables[0]
        .indexes
        .iter()
        .find(|index| index.columns == columns && index.is_unique == unique)
    else {
        return false;
    };
    plan.steps.iter().any(|step| {
        matches!(
            &step.step,
            MigrationStep::CreateIndex { table_name, index }
                if table_name == entity.table_name && index == replacement
        )
    })
}

fn only_index_replacements(plan: &PlannedMigration, allow_approval: bool) -> bool {
    let mut destructive = plan
        .steps
        .iter()
        .filter(|step| step.risk == MigrationRisk::Destructive)
        .peekable();
    if destructive.peek().is_none() {
        return false;
    }

    let webhook = AiProviderWebhookReceiptRecord::metadata();
    let replaces_webhook = creates_replacement(
        plan,
        webhook,
        &WEBHOOK_RECEIPT_COMPOSITE_INDEX_COLUMNS,
        false,
    );
    let replaces_approval = allow_approval
        && creates_replacement(
            plan,
            AiApprovalRecord::metadata(),
            LEGACY_APPROVAL_INDEX.columns,
            true,
        );
    destructive.all(|step| {
        (replaces_webhook
            && matches!(
                &step.step,
                MigrationStep::DropIndex { table_name, index_name }
                    if table_name == webhook.table_name
                        && REPLACED_WEBHOOK_RECEIPT_INDEXES.contains(&index_name.as_str())
            ))
            || (replaces_approval && is_approval_lookup_drop(&step.step))
    })
}
