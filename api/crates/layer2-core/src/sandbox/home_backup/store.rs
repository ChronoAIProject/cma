use super::{HomeBackup, HomeBackupError, HomeBackupSchedule};

/// Both documents are acquired and published in one durable transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeBackupClaim {
    pub schedule: HomeBackupSchedule,
    pub backup: Option<HomeBackup>,
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
