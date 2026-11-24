//! Read-only observations of deferred runtime bindings. Observations grant no authority.
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{Caller, Capability, SandboxId, SubjectId};

pub const MAX_REPORTERS: usize = 16;
pub const MAX_HOLDS_PER_REPORTER: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    Unavailable,
    Raced,
    Cancelled,
    Deadline,
    Corrupt,
    Conflict,
    Refused,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldPhase {
    Deferred,
    Retrying,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterFailureClass {
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingHold {
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub reason: HoldReason,
    pub phase: HoldPhase,
    pub first_held_at: String,
    pub attempts: u32,
    pub retry_not_before: Option<String>,
    pub adapter_error_class: Option<AdapterFailureClass>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingHoldSample {
    pub observed_at: String,
    pub truncated: bool,
    pub holds: Vec<BindingHold>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BindingHoldReport {
    /// Opaque label valid for this observation, never a network address.
    pub reporter: String,
    pub sample: BindingHoldSample,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BindingHoldOverview {
    pub observed_at: String,
    pub observed_sandboxes: usize,
    pub registered_reporters: usize,
    pub unavailable_reporters: usize,
    pub truncated: bool,
    pub reports: Vec<BindingHoldReport>,
}

#[derive(Debug, thiserror::Error)]
pub enum BindingDiagnosticsError {
    #[error("cross-owner read capability is required")]
    Forbidden,
    #[error("binding observations are unavailable")]
    Unavailable,
}

#[async_trait::async_trait]
pub trait BindingHoldSource: Send + Sync {
    async fn observe(
        &self,
        principal: &SubjectId,
    ) -> Result<BindingHoldOverview, BindingDiagnosticsError>;
}

pub struct BindingDiagnosticsService(Arc<dyn BindingHoldSource>);

impl BindingDiagnosticsService {
    pub fn new(source: Arc<dyn BindingHoldSource>) -> Self {
        Self(source)
    }

    pub async fn read(
        &self,
        caller: &Caller,
    ) -> Result<BindingHoldOverview, BindingDiagnosticsError> {
        if !caller.authorization.allows(Capability::ReadAllSandboxes) {
            return Err(BindingDiagnosticsError::Forbidden);
        }
        self.0.observe(&caller.subject).await
    }
}
