use serde::{Deserialize, Serialize};

const MAX_RETENTION_SECONDS: u64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupSchedule {
    pub id: String,
    pub home: HomeBackupHome,
    pub state: HomeBackupScheduleState,
    pub next_due_at: String,
    pub last_ready_at: Option<String>,
    pub last_ready_backup_id: Option<String>,
    pub lease: Option<HomeBackupLease>,
    pub next_fence: u64,
    pub in_flight: Option<HomeBackupInFlight>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupHome {
    pub mode: String,
    pub home_source_id: String,
    pub home_identity: String,
    pub source_reference: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeBackupScheduleState {
    Active,
    Retiring,
    Erasing,
    Restoring,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupLease {
    pub holder: String,
    pub token: String,
    pub fence: u64,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupInFlight {
    pub backup_id: String,
    pub phase: HomeBackupPhase,
    pub started_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeBackupPhase {
    Requested,
    Creating,
    Observing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackup {
    pub id: String,
    pub sandbox_id: String,
    pub epoch: u64,
    pub source: HomeBackupSource,
    pub cut: HomeBackupCut,
    pub storage: HomeBackupStorage,
    pub custody: HomeBackupCustody,
    pub retention: HomeBackupRetention,
    pub state: HomeBackupState,
    pub restore_pins: u32,
    pub lease: Option<HomeBackupLease>,
    pub next_fence: u64,
    pub revision: u64,
    pub failure: Option<HomeBackupFailure>,
    pub erasure: Option<HomeBackupErasure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupSource {
    pub home_source_id: String,
    pub home_identity: String,
    pub runtime_generation: u64,
    pub runtime_version: String,
    pub boot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupCut {
    pub requested_at: String,
    pub source_creation_time: Option<String>,
    pub ready_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupStorage {
    pub snapshot_name: String,
    pub snapshot_class: String,
    pub source_reference: Option<String>,
    pub restore_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupCustody {
    pub encryption_key_version_id: Option<String>,
    pub auth_key_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupRetention {
    pub class: HomeBackupRetentionClass,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeBackupState {
    Requested,
    Creating,
    Ready,
    Failed,
    Expiring,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBackupErasure {
    pub erased_at: String,
    pub receipt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeBackupRetentionClass {
    Short,
    Hourly,
    Daily,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeBackupFailure {
    SourceChanged,
    CopyTimedOut,
    SourceUnavailable,
    CopyUnavailable,
    CopyMissing,
    CaptureCancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HomeBackupError {
    #[error("invalid home backup: {0}")]
    Invalid(&'static str),
    #[error("home backup authority changed")]
    Conflict,
    #[error("home backup counter exhausted")]
    Exhausted,
    #[error("home backup store unavailable")]
    Unavailable,
    #[error("home backup transaction outcome unknown")]
    CommitUnknown,
}

impl HomeBackupSchedule {
    /// Fence capture before any retirement effect. Preserve the revoked lease
    /// as the fixed drain deadline for already dispatched external calls.
    /// Repeated retirement attempts must never extend that deadline.
    pub fn retiring(&self) -> Result<Self, HomeBackupError> {
        self.validate()?;
        if self.state == HomeBackupScheduleState::Retiring {
            return Ok(self.clone());
        }
        if self.state != HomeBackupScheduleState::Active {
            return Err(HomeBackupError::Conflict);
        }
        let mut next = self.clone();
        next.state = HomeBackupScheduleState::Retiring;
        next.next_fence = increment(self.next_fence)?;
        next.revision = increment(self.revision)?;
        next.validate()?;
        Ok(next)
    }

    pub fn validate(&self) -> Result<(), HomeBackupError> {
        nonempty(&self.id)?;
        nonempty(&self.home.mode)?;
        nonempty(&self.home.home_source_id)?;
        nonempty(&self.home.home_identity)?;
        if let Some(reference) = &self.home.source_reference {
            nonempty(reference)?;
        }
        timestamp(&self.next_due_at)?;
        counter(self.revision)?;
        counter(self.next_fence)?;
        if self.last_ready_at.is_some() != self.last_ready_backup_id.is_some() {
            return invalid("incomplete latest ready point");
        }
        if let Some(ready) = &self.last_ready_at {
            timestamp(ready)?;
        }
        if let Some(id) = &self.last_ready_backup_id {
            nonempty(id)?;
        }
        validate_authority(self.lease.as_ref(), self.next_fence)?;
        if let Some(flight) = &self.in_flight {
            nonempty(&flight.backup_id)?;
            timestamp(&flight.started_at)?;
        }
        if self.state == HomeBackupScheduleState::Closed
            && (self.lease.is_some() || self.in_flight.is_some())
        {
            return invalid("closed schedule retains capture authority");
        }
        Ok(())
    }

    pub fn claim(&mut self, now: &str, lease: HomeBackupLease) -> Result<(), HomeBackupError> {
        self.validate()?;
        let at = timestamp(now)?;
        if self.state != HomeBackupScheduleState::Active
            || timestamp(&self.next_due_at)? > at
            || self
                .lease
                .as_ref()
                .is_some_and(|value| timestamp(&value.expires_at).is_ok_and(|expires| expires > at))
        {
            return Err(HomeBackupError::Conflict);
        }
        validate_lease(&lease)?;
        if timestamp(&lease.expires_at)? <= at || lease.fence != self.next_fence {
            return Err(HomeBackupError::Conflict);
        }
        let next_fence = increment(self.next_fence)?;
        let revision = increment(self.revision)?;
        self.next_fence = next_fence;
        self.revision = revision;
        self.lease = Some(lease);
        self.validate()
    }
}

impl HomeBackup {
    pub fn validate(&self) -> Result<(), HomeBackupError> {
        nonempty(&self.sandbox_id)?;
        counter(self.epoch)?;
        if self.id != format!("hb-{}-{}", self.sandbox_id, self.epoch) {
            return invalid("non-deterministic backup identity");
        }
        nonempty(&self.source.home_source_id)?;
        nonempty(&self.source.home_identity)?;
        nonempty(&self.source.runtime_version)?;
        nonempty(&self.source.boot_id)?;
        counter(self.source.runtime_generation)?;
        counter(self.revision)?;
        counter(self.next_fence)?;
        nonempty(&self.storage.snapshot_name)?;
        nonempty(&self.storage.snapshot_class)?;
        for reference in [
            &self.storage.source_reference,
            &self.custody.encryption_key_version_id,
        ]
        .into_iter()
        .flatten()
        {
            nonempty(reference)?;
        }
        for id in &self.custody.auth_key_ids {
            nonempty(id)?;
        }
        if self
            .custody
            .auth_key_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != self.custody.auth_key_ids.len()
        {
            return invalid("duplicate custody key identity");
        }
        if self.storage.restore_size_bytes == Some(0) {
            return invalid("empty restore size");
        }
        let requested = timestamp(&self.cut.requested_at)?;
        let expires = timestamp(&self.retention.expires_at)?;
        if let Some(created) = &self.cut.source_creation_time {
            if timestamp(created)? < requested {
                return invalid("capture predates request");
            }
        }
        if let Some(ready) = &self.cut.ready_at {
            let ready = timestamp(ready)?;
            let captured = self
                .cut
                .source_creation_time
                .as_ref()
                .ok_or(HomeBackupError::Invalid("ready point lacks capture time"))?;
            if ready < timestamp(captured)?
                || expires < ready
                || expires - ready > MAX_RETENTION_SECONDS
            {
                return invalid("invalid ready point or retention deadline");
            }
        } else if expires < requested || expires - requested > MAX_RETENTION_SECONDS {
            return invalid("unbounded pending capture");
        }
        if self.state == HomeBackupState::Ready
            && (self.cut.ready_at.is_none() || self.storage.source_reference.is_none())
        {
            return invalid("completed backup lacks evidence");
        }
        if (self.state == HomeBackupState::Failed && self.failure.is_none())
            || (!matches!(
                self.state,
                HomeBackupState::Failed | HomeBackupState::Expiring | HomeBackupState::Deleted
            ) && self.failure.is_some())
        {
            return invalid("failure state and reason disagree");
        }
        if self.state != HomeBackupState::Ready && self.restore_pins != 0 {
            return invalid("unavailable backup retains restore pins");
        }
        if (self.state == HomeBackupState::Deleted) != self.erasure.is_some() {
            return invalid("erasure receipt and state disagree");
        }
        if let Some(erasure) = &self.erasure {
            timestamp(&erasure.erased_at)?;
            nonempty(&erasure.receipt)?;
        }
        validate_authority(self.lease.as_ref(), self.next_fence)
    }
}

pub(super) fn timestamp(value: &str) -> Result<u64, HomeBackupError> {
    crate::seconds_from_rfc3339(value).ok_or(HomeBackupError::Invalid("noncanonical timestamp"))
}
fn nonempty(value: &str) -> Result<(), HomeBackupError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        invalid("invalid identifier")
    } else {
        Ok(())
    }
}
fn counter(value: u64) -> Result<(), HomeBackupError> {
    if value == 0 || value > i64::MAX as u64 {
        invalid("invalid counter")
    } else {
        Ok(())
    }
}
pub fn increment(value: u64) -> Result<u64, HomeBackupError> {
    value
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or(HomeBackupError::Exhausted)
}
fn validate_authority(lease: Option<&HomeBackupLease>, next: u64) -> Result<(), HomeBackupError> {
    if let Some(lease) = lease {
        validate_lease(lease)?;
        if lease.fence >= next {
            return invalid("lease exceeds fence high water");
        }
    }
    Ok(())
}
fn validate_lease(lease: &HomeBackupLease) -> Result<(), HomeBackupError> {
    let (deployment, attempt) = lease
        .holder
        .rsplit_once(':')
        .ok_or(HomeBackupError::Invalid("invalid lease holder"))?;
    nonempty(deployment)?;
    uuid::Uuid::parse_str(attempt)
        .map_err(|_| HomeBackupError::Invalid("invalid holder attempt"))?;
    uuid::Uuid::parse_str(&lease.token)
        .map_err(|_| HomeBackupError::Invalid("invalid lease token"))?;
    counter(lease.fence)?;
    timestamp(&lease.expires_at)?;
    Ok(())
}
fn invalid<T>(reason: &'static str) -> Result<T, HomeBackupError> {
    Err(HomeBackupError::Invalid(reason))
}
