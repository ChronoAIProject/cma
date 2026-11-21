use super::{HomeBackup, HomeBackupError, HomeBackupSchedule};

/// Both documents are acquired and published in one durable transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeBackupClaim {
    pub schedule: HomeBackupSchedule,
    pub backup: Option<HomeBackup>,
}

/// Independent from capture so lifecycle cleanup can remain active when new
/// capture admission is disabled. The caller must wait for the retained lease
/// deadline before destroying the source; a fenced holder cannot publish.
#[async_trait::async_trait]
pub trait HomeBackupRetirementStore: Send + Sync {
    /// `seed` has the same pristine shape accepted by `ensure_schedule`.
    /// If discovery has not inserted it yet, insert a retiring sentinel so a
    /// late set-on-insert cannot reactivate capture after teardown begins.
    async fn retire_schedule(
        &self,
        seed: &HomeBackupSchedule,
    ) -> Result<HomeBackupSchedule, HomeBackupError>;
}

/// Catalog coverage is evidence about durable records, not proof that an
/// independent physical copy remains readable. All retained homes contribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeBackupCoverage {
    pub sandbox_id: String,
    pub schedule_present: bool,
    pub latest_ready_at: Option<String>,
}

#[async_trait::async_trait]
pub trait HomeBackupDiscoveryStore: Send + Sync {
    /// A bounded keyset page over Sandbox authority. An error returns no
    /// continuation, so callers retry the same page before publishing coverage.
    async fn backfill_schedules(
        &self,
        page: &crate::sandbox::store::PageRequest,
        now: &str,
    ) -> Result<crate::sandbox::store::Page<HomeBackupCoverage>, HomeBackupError>;
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
