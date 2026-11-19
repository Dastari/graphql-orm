//! Typed representations of text stored by AI lifecycle records.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

macro_rules! persisted_state {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum $name {
            $($variant),+
        }

        impl $name {
            pub(crate) const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }

            pub(crate) fn from_persisted(value: &str) -> Option<Self> {
                match value {
                    $($value => Some(Self::$variant)),+,
                    _ => None,
                }
            }
        }
    };
}

persisted_state!(AttachmentQuarantineState {
    PendingUpload => "pending_upload",
    Uploading => "uploading",
    Rejected => "rejected",
    Failed => "failed",
    Expired => "expired",
    Ready => "ready",
    Released => "released",
    Deleting => "deleting",
    Deleted => "deleted",
});

persisted_state!(AttachmentScanState {
    Pending => "pending",
    Rejected => "rejected",
    Failed => "failed",
    Clean => "clean",
});

persisted_state!(AttachmentProcessingState {
    Pending => "pending",
    Scanning => "scanning",
    Ready => "ready",
    Complete => "complete",
    Deleting => "deleting",
    CleanupRequired => "cleanup_required",
    RetentionCleanupRequired => "retention_cleanup_required",
    CleanupInProgress => "cleanup_in_progress",
    CleanupBackoff => "cleanup_backoff",
});

persisted_state!(AttachmentArtifactCleanupState {
    CleanupRequired => "cleanup_required",
    CleanupInProgress => "cleanup_in_progress",
    CleanupBackoff => "cleanup_backoff",
    CleanupUnconfirmed => "cleanup_unconfirmed",
    Complete => "complete",
});

persisted_state!(ToolCallState {
    WaitingApproval => "waiting_approval",
    ApprovalPrepared => "approval_prepared",
    ControlBlocked => "control_blocked",
    Cancelled => "cancelled",
    Executing => "executing",
    SourceRetained => "source_retained",
    WaitingSubscription => "waiting_subscription",
});
