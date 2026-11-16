use super::{HomeBackup, HomeBackupDeletionObservation, HomeBackupError, HomeBackupSchedule};

/// Both documents are acquired and published in one durable transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeBackupClaim {
    pub schedule: HomeBackupSchedule,
    pub backup: Option<HomeBackup>,
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
    /// Recovery-only claim after new expiry admission closes. Never transitions
    /// a ready or failed record into expiring.
    async fn claim_expiring(
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
