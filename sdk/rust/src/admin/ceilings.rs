//! Independently revised admission ceilings and bounded capacity observations.

use super::{
    Administration,
    admission::{nullable, reason, valid_reason},
};
use crate::{Error, Result};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

/// Capacity reserved for existing consumers within the corresponding fleet ceiling.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExistingReserve {
    /// Retained home slots unavailable to public owners; unset means no reserve.
    pub retained_homes: Option<u32>,
    /// Active Agent slots unavailable to public owners; unset means no reserve.
    pub active_sessions: Option<u32>,
}

/// Operator ceilings. Unset is unlimited; zero refuses additional capacity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdmissionCeilings {
    /// Newly registered public subjects after durable activation.
    pub registrations: Option<u32>,
    /// Retained Sandbox homes across the fleet.
    pub retained_homes: Option<u32>,
    /// Agents with working or waiting-approval status across the fleet.
    pub active_sessions: Option<u32>,
    /// Active Agents per owner, including existing consumers.
    pub owner_active_sessions: Option<u32>,
    /// Existing-consumer shares within the fleet ceilings.
    pub existing_reserve: ExistingReserve,
}

impl AdmissionCeilings {
    fn valid_reserve(&self) -> bool {
        [
            (self.existing_reserve.retained_homes, self.retained_homes),
            (self.existing_reserve.active_sessions, self.active_sessions),
        ].into_iter().all(|(reserve, ceiling)| {
            !matches!((reserve, ceiling), (Some(reserve), Some(ceiling)) if reserve > ceiling)
        })
    }
}

/// Full ceiling replacement using its own revision, independent of stop/resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceAdmissionCeilingsRequest {
    /// Full replacement; omitted individual limits are unset, not preserved.
    pub ceilings: AdmissionCeilings,
    /// Nonblank reason, at most 512 UTF-8 bytes, without control characters.
    pub reason: String,
    /// Observed ceiling revision, not the admission stop revision.
    pub expected_revision: u64,
}

/// Immutable evidence for one actual ceiling replacement.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AdmissionCeilingsChange {
    /// Resulting independent ceiling revision.
    pub revision: u64,
    /// Commit time in Unix seconds.
    pub at: u64,
    /// Operator subject.
    pub actor: String,
    /// Bounded operator reason.
    #[serde(deserialize_with = "reason")]
    pub reason: String,
    /// Full policy before the change.
    #[serde(deserialize_with = "configured_ceilings")]
    pub old_ceilings: AdmissionCeilings,
    /// Full policy after the change.
    #[serde(deserialize_with = "configured_ceilings")]
    pub new_ceilings: AdmissionCeilings,
}

/// Current ceiling state returned by GET and the replacement command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AdmissionCeilingsStateView {
    /// Independent ceiling revision; zero before the first change.
    pub revision: u64,
    /// Configured operator ceilings, before the deployment cap.
    #[serde(deserialize_with = "configured_ceilings")]
    pub ceilings: AdmissionCeilings,
    /// Most recent actual replacement, or null before the first change.
    #[serde(deserialize_with = "nullable")]
    pub last_change: Option<AdmissionCeilingsChange>,
    /// Whether the independent deployment ceiling write gate is open.
    pub writes_available: bool,
}

/// A bounded observation; saturation means the true count may be higher.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CapacityCount {
    /// Number observed up to the query bound.
    pub observed: u64,
    /// Maximum number the on-demand query observes.
    pub bound: u64,
    /// Whether the observation reached its bound.
    pub saturated: bool,
}

/// Advisory observations computed on demand, not reservations for another write.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AdmissionCapacityCounts {
    /// Registration observation; null when that ceiling is unset.
    #[serde(deserialize_with = "nullable")]
    pub registrations: Option<CapacityCount>,
    /// Retained home observation; null when no effective home limit is set.
    #[serde(deserialize_with = "nullable")]
    pub retained_homes: Option<CapacityCount>,
    /// Exact pending Launch home reservations, even when the home limit is unset.
    pub pending_launch_homes: u64,
    /// Active Agent observation; null when that fleet ceiling is unset.
    #[serde(deserialize_with = "nullable")]
    pub active_sessions: Option<CapacityCount>,
}

impl Administration<'_> {
    /// Replace all ceilings with independent CAS. Never retries implicitly;
    /// read current state after a conflict or uncertain outcome before deciding.
    pub async fn replace_admission_ceilings(
        &self,
        body: &ReplaceAdmissionCeilingsRequest,
    ) -> Result<AdmissionCeilingsStateView> {
        if !valid_reason(&body.reason) {
            return Err(Error::validation(
                "reason must contain 1 to 512 bytes without control characters",
            ));
        }
        if !body.ceilings.valid_reserve() {
            return Err(Error::validation("existing reserve exceeds its ceiling"));
        }
        self.client
            .operation(
                Method::PUT,
                &["admin", "admission", "ceilings"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
}

fn configured_ceilings<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<AdmissionCeilings, D::Error> {
    let value = AdmissionCeilings::deserialize(deserializer)?;
    if !value.valid_reserve() {
        return Err(D::Error::custom("existing reserve exceeds its ceiling"));
    }
    Ok(value)
}

pub(super) fn optional<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

pub(super) fn optional_history<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Vec<AdmissionCeilingsChange>>, D::Error> {
    let changes = Vec::<AdmissionCeilingsChange>::deserialize(deserializer)?;
    if changes.len() > 20
        || changes
            .windows(2)
            .any(|pair| pair[1].revision.checked_add(1) != Some(pair[0].revision))
    {
        return Err(D::Error::custom(
            "expected at most twenty consecutive ceilings changes, newest first",
        ));
    }
    Ok(Some(changes))
}
