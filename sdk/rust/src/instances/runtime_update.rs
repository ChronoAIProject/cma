use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

use super::{Sandbox, Sandboxes};
use crate::{IdempotencyKey, Result, resources::identity};

string_enum! {
    /// Durable progress of the managed runtime update.
    pub enum SandboxRuntimeUpdateStatus {
        UpToDate => "up_to_date", Available => "available", Queued => "queued",
        Updating => "updating", Failed => "failed"
    }
}

/// The exact released image target shown by Sandbox Details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSandboxRuntimeRequest {
    /// Immutable reference copied from `latest_version`, or a failed pinned target.
    pub target_version: String,
}

/// Server-owned daily check schedule.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxRuntimeUpdateSchedule {
    /// IANA time zone, currently Asia/Singapore.
    pub timezone: String,
    /// Local wall time, currently 02:00.
    pub local_time: String,
}

/// Safe failure information for an exact-phase retry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxRuntimeUpdateFailure {
    /// Stable error classification.
    pub code: String,
    /// User-facing safe description.
    pub message: String,
}

/// Owner-visible software update information, absent on older servers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxRuntimeUpdate {
    /// Image currently bound to the active or suspended runtime.
    pub current_version: Option<String>,
    /// Server's current released managed runtime.
    pub latest_version: String,
    /// Whether this runtime differs from the released managed runtime.
    pub update_available: bool,
    /// Durable request progress.
    pub state: SandboxRuntimeUpdateStatus,
    /// Whether the owner may request or retry an update now.
    pub can_update: bool,
    /// Whether this Sandbox supports updating on its exact original home.
    pub preserves_home: bool,
    /// Scheduled managed update check.
    pub schedule: SandboxRuntimeUpdateSchedule,
    /// Safe eligibility explanation, when present.
    pub blocked_reason: Option<String>,
    /// Accepted target retained through deployment changes.
    pub target_version: Option<String>,
    /// Durable request timestamp.
    pub requested_at: Option<String>,
    /// Start of admitted replacement work.
    pub started_at: Option<String>,
    /// Successful completion timestamp.
    pub completed_at: Option<String>,
    /// Parked update failure, if any.
    pub failure: Option<SandboxRuntimeUpdateFailure>,
}

impl Sandboxes<'_> {
    /// Queue an exact managed runtime update without replacing the original home.
    pub async fn update_runtime(
        &self,
        id: &str,
        body: &UpdateSandboxRuntimeRequest,
        key: &IdempotencyKey,
    ) -> Result<Sandbox> {
        let value: Sandbox = self
            .client
            .command(
                Method::POST,
                &["sandboxes", id, "runtime-update"],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await?;
        identity(&value.id, id)?;
        Ok(value)
    }
}
