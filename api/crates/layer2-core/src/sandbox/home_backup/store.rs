use super::{HomeBackup, HomeBackupDeletionObservation, HomeBackupError, HomeBackupSchedule};

/// Both documents are acquired and published in one durable transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeBackupClaim {
    pub schedule: HomeBackupSchedule,
    pub backup: Option<HomeBackup>,
}

/// Receipt returned to the account-erasure coordinator for one physically
/// removed backup. The receipt is returned rather than persisted here because
/// account erasure owns its durable deletion ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeBackupErasureReceipt {
    pub sandbox_id: String,
    pub backup_id: String,
    pub erased_at: String,
    pub receipt: String,
}

/// Deletion admission is separate from capture; consumers must keep its writer
/// disabled until provider deletion and independent absence checks are proved.
#[async_trait::async_trait]
pub trait HomeBackupDeletionStore: Send + Sync {
    /// Atomically persist `expiring` and acquire its document lease before effects.
    async fn claim_expiry(
        &self,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Option<HomeBackup>, HomeBackupError>;
    /// Requires `erasing` on the parent schedule in the same transaction. Never
    /// preempts a live capture/deletion lease or a restore pin.
    async fn claim_erasure(
        &self,
        backup_id: &str,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<HomeBackup, HomeBackupError>;
    /// Publish the exact absence receipt with token, fence, revision and lease
    /// checks after the external call. An ambiguous result must be read back.
    async fn save_deletion(
        &self,
        claimed: &HomeBackup,
        observation: &HomeBackupDeletionObservation,
        now: &str,
    ) -> Result<HomeBackup, HomeBackupError>;
}

#[async_trait::async_trait]
pub trait HomeBackupStore: Send + Sync {
    async fn get_schedule(&self, id: &str) -> Result<Option<HomeBackupSchedule>, HomeBackupError>;
    async fn get_backup(&self, id: &str) -> Result<Option<HomeBackup>, HomeBackupError>;
    async fn ensure_schedule(&self, schedule: &HomeBackupSchedule) -> Result<(), HomeBackupError>;
    async fn claim_due(
        &self,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Option<HomeBackupClaim>, HomeBackupError>;
    async fn plan_capture(
        &self,
        claim: &HomeBackupClaim,
        backup: &HomeBackup,
        now: &str,
    ) -> Result<HomeBackupClaim, HomeBackupError>;
    async fn save_capture(
        &self,
        claim: &HomeBackupClaim,
        schedule: &HomeBackupSchedule,
        backup: &HomeBackup,
        now: &str,
    ) -> Result<HomeBackupClaim, HomeBackupError>;
}

/// Provider-neutral storage boundary for fencing one already owner-qualified
/// Sandbox and claiming all of its backups. It does not enumerate owners or
/// perform provider deletion effects.
#[async_trait::async_trait]
pub trait HomeBackupErasureStore: HomeBackupDeletionStore {
    async fn claim_sandbox_erasure(
        &self,
        sandbox_id: &str,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Vec<HomeBackup>, HomeBackupError>;
}

/// The irreversible provider effect is a separate port. Implementations must
/// delete both the named backup and its independent retained copy, then return
/// an observation suitable for `save_deletion`.
#[async_trait::async_trait]
pub trait HomeBackupDeletionPort: Send + Sync {
    async fn delete_backup(
        &self,
        claimed: &HomeBackup,
    ) -> Result<HomeBackupDeletionObservation, HomeBackupError>;
}
