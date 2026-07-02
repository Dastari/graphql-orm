use crate::BackupError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestoreMode {
    EmptyDatabase,
    DryRun,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreContext {
    pub mode: RestoreMode,
    pub disable_policies: bool,
    pub disable_change_journal: bool,
}

impl RestoreContext {
    /// Builds the default empty-database restore context.
    #[must_use]
    pub fn empty_database() -> Self {
        Self {
            mode: RestoreMode::EmptyDatabase,
            disable_policies: true,
            disable_change_journal: true,
        }
    }

    /// Builds a dry-run restore context.
    #[must_use]
    pub fn dry_run() -> Self {
        Self {
            mode: RestoreMode::DryRun,
            disable_policies: true,
            disable_change_journal: true,
        }
    }
}

/// Ensures an empty-target restore is only applied to an empty database.
///
/// # Errors
///
/// Returns [`BackupError::RestoreTargetNotEmpty`] when the context requires an
/// empty database and the target is not empty.
pub fn ensure_empty_restore_target(
    target_is_empty: bool,
    context: &RestoreContext,
) -> Result<(), BackupError> {
    match context.mode {
        RestoreMode::EmptyDatabase if target_is_empty => Ok(()),
        RestoreMode::EmptyDatabase => Err(BackupError::RestoreTargetNotEmpty),
        RestoreMode::DryRun => Ok(()),
    }
}
