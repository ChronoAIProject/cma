use super::{HomeBackup, HomeBackupDeletionObservation, HomeBackupError, HomeBackupSchedule};

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
