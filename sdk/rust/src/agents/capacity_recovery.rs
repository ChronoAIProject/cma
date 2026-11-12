//! Explicit capacity-recovery requests. Never invent or retry an original input.
use super::Agents;
use crate::{
    Error, Result,
    http::{Request, receive},
    resources::identity,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Immutable original failure identity supplied by the recovery operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityRecoveryRequest {
    /// Original Agent identity.
    pub agent_id: String,
    /// Original Agent creation key.
    pub creation_key: String,
    /// Original creation fingerprint.
    pub creation_fingerprint: String,
    /// Original failure's global log sequence.
    pub failure_seq: u64,
    /// Unchanged request timestamp.
    pub requested_at: String,
}
/// Authorized metadata; this is not permission to activate recovery.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CapacityRecoveryPreflight {
    /// Requested Agent identity.
    pub agent_id: String,
    /// Current eligibility classification, never an admission receipt.
    pub disposition: String,
    /// Current Agent revision.
    pub observed_revision: u64,
    /// Current log high-water.
    pub through_seq: u64,
    /// Time of the authorized observation.
    pub evaluated_at: String,
}
/// Personal authorized status of an original recovery request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CapacityRecoveryStatus {
    /// Requested Agent identity.
    pub agent_id: String,
    /// Current eligibility classification.
    pub disposition: String,
    /// Accepted work phase, when available.
    pub phase: Option<String>,
    /// Recorded bounded attempts.
    pub attempts: u32,
    /// Earliest retry time, if present.
    pub retry_at: Option<String>,
    /// Original operation identity; never authority.
    pub operation_key: Option<String>,
}
/// A recorded result or an uncertain commit. Uncertainty never authorizes replay.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CapacityRecoveryOutcome {
    /// The exact recovery attempt is known to have aborted; never replays input.
    KnownAbort,
    /// Commit must be reconciled using the unchanged original request.
    Unknown,
    /// Durable result projected without native/home identities.
    Recorded {
        /// Original operation identity.
        operation_key: String,
        /// Durable phase.
        phase: String,
        /// Recorded attempts.
        attempts: u32,
        /// Earliest retry time.
        retry_at: Option<String>,
        /// Durable completion time, if complete.
        completed_at: Option<String>,
    },
}
impl Agents<'_> {
    /// Read authorized metadata without admitting recovery.
    pub async fn capacity_recovery_preflight(&self, id: &str) -> Result<CapacityRecoveryPreflight> {
        let value: CapacityRecoveryPreflight = self
            .client
            .get(&["agents", id, "capacity-recovery", "dry-run"], &[])
            .await?;
        identity(&value.agent_id, id)?;
        Ok(value)
    }
    /// Observe current status without an original-request lookup.
    pub async fn capacity_recovery_status(&self, id: &str) -> Result<CapacityRecoveryStatus> {
        let value: CapacityRecoveryStatus = self
            .client
            .get(&["agents", id, "capacity-recovery"], &[])
            .await?;
        identity(&value.agent_id, id)?;
        Ok(value)
    }
    /// Inspect the unchanged original request; no admission or runtime effects.
    pub async fn capacity_recovery_request_status(
        &self,
        body: &CapacityRecoveryRequest,
    ) -> Result<CapacityRecoveryStatus> {
        let value: CapacityRecoveryStatus = self
            .client
            .operation(
                Method::POST,
                &["agents", &body.agent_id, "capacity-recovery", "status"],
                Some(body),
                StatusCode::OK,
            )
            .await?;
        identity(&value.agent_id, &body.agent_id)?;
        Ok(value)
    }
    /// Explicitly request admission. The server's activation gate remains authoritative.
    pub async fn admit_capacity_recovery(
        &self,
        body: &CapacityRecoveryRequest,
    ) -> Result<CapacityRecoveryOutcome> {
        self.capacity_recovery_command(body, "admit").await
    }
    /// Reconcile accepted work with its exact original request; never sends Agent input.
    pub async fn resume_capacity_recovery(
        &self,
        body: &CapacityRecoveryRequest,
    ) -> Result<CapacityRecoveryOutcome> {
        self.capacity_recovery_command(body, "resume").await
    }
    async fn capacity_recovery_command(
        &self,
        body: &CapacityRecoveryRequest,
        action: &str,
    ) -> Result<CapacityRecoveryOutcome> {
        let segments = [
            "agents",
            body.agent_id.as_str(),
            "capacity-recovery",
            action,
        ];
        let mut request = Request::operation(Method::POST, &segments, Some(body), StatusCode::OK)?;
        request.statuses.push(StatusCode::ACCEPTED);
        request.statuses.push(StatusCode::CONFLICT);
        let response = receive(&self.client.inner, request).await?;
        if response.status == StatusCode::CONFLICT {
            if serde_json::from_slice::<serde_json::Value>(&response.body).ok()
                == Some(serde_json::json!({"outcome": "known_abort"}))
            {
                return Ok(CapacityRecoveryOutcome::KnownAbort);
            }
            return Err(Error::from_http_problem(
                response.status.as_u16(),
                response
                    .headers
                    .get("x-request-id")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned),
                response
                    .headers
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok()),
                &response.body,
            ));
        }
        let value = serde_json::from_slice(&response.body)
            .map_err(|_| Error::protocol("invalid capacity recovery outcome"))?;
        match (&value, response.status) {
            (CapacityRecoveryOutcome::Recorded { .. }, StatusCode::OK)
            | (CapacityRecoveryOutcome::Unknown, StatusCode::ACCEPTED) => Ok(value),
            _ => Err(Error::protocol(
                "recovery outcome does not match its HTTP status",
            )),
        }
    }
}
