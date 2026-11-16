//! Independent operator control over new work; existing work keeps running.

use super::Administration;
use super::ceilings::{
    self, AdmissionCapacityCounts, AdmissionCeilings, AdmissionCeilingsChange,
    AdmissionCeilingsStateView,
};
use crate::{Error, Result};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

/// A revision-checked operator command. Neither command is retried implicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeAdmissionRequest {
    /// Nonblank reason, at most 512 UTF-8 bytes, without control characters.
    pub reason: String,
    /// Independent admission revision; resume must name the active stop.
    pub expected_revision: u64,
}

/// The durable stop selected by an authorized operator.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OperatorAdmissionStop {
    /// Commit time in Unix seconds.
    pub since: u64,
    /// Independent admission revision.
    pub revision: u64,
    /// Operator subject, available only on the authorized administration API.
    pub actor: String,
    /// Bounded operator reason.
    #[serde(deserialize_with = "reason")]
    pub reason: String,
}

/// Immutable evidence for one actual admission change.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AdmissionControlChange {
    /// Resulting admission revision.
    pub revision: u64,
    /// Commit time in Unix seconds.
    pub at: u64,
    /// Operator subject.
    pub actor: String,
    /// Bounded operator reason.
    #[serde(deserialize_with = "reason")]
    pub reason: String,
    /// Previous stop; null when this change stops open admission.
    #[serde(deserialize_with = "nullable")]
    pub old_stop: Option<OperatorAdmissionStop>,
    /// Resulting stop; null when this change resumes admission.
    #[serde(deserialize_with = "nullable")]
    pub new_stop: Option<OperatorAdmissionStop>,
}

/// Current admission state returned by stop and resume commands.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AdmissionControlView {
    /// Independent admission revision; zero before the first change.
    pub revision: u64,
    /// Active operator stop, if any.
    #[serde(deserialize_with = "nullable")]
    pub operator_stop: Option<OperatorAdmissionStop>,
    /// Most recent actual change, if any.
    #[serde(deserialize_with = "nullable")]
    pub last_change: Option<AdmissionControlChange>,
    /// Whether the deployment allows operator control writes.
    pub writes_available: bool,
}

/// One consistent read of admission state and bounded immutable history.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AdmissionControlReadView {
    /// Current control fields, flattened in the wire representation.
    #[serde(flatten)]
    pub control: AdmissionControlView,
    /// At most twenty consecutive changes, newest revision first.
    #[serde(deserialize_with = "history")]
    pub history: Vec<AdmissionControlChange>,
    /// Independently revised ceiling policy; absent on servers predating ceilings.
    #[serde(default, deserialize_with = "ceilings::optional")]
    pub ceilings: Option<AdmissionCeilingsStateView>,
    /// Limits after the deployment home cap; its reserve may exceed that cap.
    #[serde(default, deserialize_with = "ceilings::optional")]
    pub effective_limits: Option<AdmissionCeilings>,
    /// On-demand bounded observations; absent on servers predating ceilings.
    #[serde(default, deserialize_with = "ceilings::optional")]
    pub capacity: Option<AdmissionCapacityCounts>,
    /// At most twenty ceiling changes, newest first; independent of stop history.
    #[serde(default, deserialize_with = "ceilings::optional_history")]
    pub ceilings_history: Option<Vec<AdmissionCeilingsChange>>,
}

impl Administration<'_> {
    /// Read admission state and the newest twenty immutable changes.
    pub async fn read_admission(&self) -> Result<AdmissionControlReadView> {
        self.client.get(&["admin", "admission"], &[]).await
    }

    /// Stop new work. A duplicate stop returns the original operator's state.
    pub async fn stop_admission(
        &self,
        body: &ChangeAdmissionRequest,
    ) -> Result<AdmissionControlView> {
        self.change_admission("stop", body).await
    }

    /// Resume at the observed admission revision. Read again after uncertainty
    /// or a conflict before deciding whether to issue another command.
    pub async fn resume_admission(
        &self,
        body: &ChangeAdmissionRequest,
    ) -> Result<AdmissionControlView> {
        self.change_admission("resume", body).await
    }

    async fn change_admission(
        &self,
        operation: &str,
        body: &ChangeAdmissionRequest,
    ) -> Result<AdmissionControlView> {
        if !valid_reason(&body.reason) {
            return Err(Error::validation(
                "reason must contain 1 to 512 bytes without control characters",
            ));
        }
        self.client
            .operation(
                Method::POST,
                &["admin", "admission", operation],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
}

pub(super) fn valid_reason(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

pub(super) fn reason<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if !valid_reason(&value) {
        return Err(D::Error::custom("invalid admission reason"));
    }
    Ok(value)
}

pub(super) fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(deserializer)
}

fn history<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<AdmissionControlChange>, D::Error> {
    let changes = Vec::<AdmissionControlChange>::deserialize(deserializer)?;
    if changes.len() > 20
        || changes
            .windows(2)
            .any(|pair| pair[1].revision.checked_add(1) != Some(pair[0].revision))
    {
        return Err(D::Error::custom(
            "expected at most twenty consecutive admission changes, newest first",
        ));
    }
    Ok(changes)
}
