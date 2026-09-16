//! Observations of existing durable reads, never recovery authority or a second
//! work scan. IDs are correlation fields only, never log-metric dimensions.
use layer2_core::sandbox::{
    agent_io::{AdmittedAgentCommand, AgentActor, AgentResponseStatus, AgentSourceKind},
    AgentOperationDelivery,
};
use sha2::{Digest, Sha256};

pub(crate) const QUEUE_WARN_MS: i64 = 60_000;
pub(crate) const PENDING_WARN_MS: i64 = 300_000;
pub(crate) const DISPATCH_WARN_MS: i64 = 30_000;

#[derive(Clone, Copy)]
pub(crate) enum PendingStage {
    Inbox,
    Allocation,
    ChildCleanup,
    WorkspaceCleanup,
}
impl PendingStage {
    fn label(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Allocation => "allocation",
            Self::ChildCleanup => "child_cleanup",
            Self::WorkspaceCleanup => "workspace_cleanup",
        }
    }
}

pub(crate) fn now_ms() -> i64 {
    mongodb::bson::DateTime::now().timestamp_millis()
}

pub(crate) fn timestamp_ms(value: &str) -> Option<i64> {
    mongodb::bson::DateTime::parse_rfc3339_str(value)
        .ok()
        .map(|value| value.timestamp_millis())
}

pub(crate) fn age_ms(now: i64, since: i64) -> i64 {
    now.saturating_sub(since).max(0)
}

/// Age is since durable admission,
/// not since process startup. A held job is not reported as completed work.
pub(crate) fn pending(stage: PendingStage, id: &str, since_ms: i64, held: bool) {
    let age_ms = age_ms(now_ms(), since_ms);
    let work_id_sha256 = format!("{:x}", Sha256::digest(id.as_bytes()));
    tracing::info!(
        target: "cma::bridge::operations",
        operation = "pending", worker = stage.label(), age_ms, held,
        overdue = age_ms >= PENDING_WARN_MS, work_id_sha256,
        "channel_health"
    );
}

/// Describes unresolved provider effects without claiming an orphan exists or
/// authorizing a new create. Existing allocation evidence drives reconciliation.
pub(crate) fn mint_unknown(id: &str) {
    let work_id_sha256 = format!("{:x}", Sha256::digest(id.as_bytes()));
    tracing::info!(
        target: "cma::bridge::operations",
        operation = "allocation_effect", outcome = "mint_unknown", work_id_sha256,
        "channel_health"
    );
}

pub(crate) fn credential_expiry(id: &str, expires_at_ms: i64) {
    let remaining_ms = expires_at_ms.saturating_sub(now_ms());
    let work_id_sha256 = format!("{:x}", Sha256::digest(id.as_bytes()));
    tracing::info!(
        target: "cma::bridge::operations",
        operation = "credential_expiry", remaining_ms,
        expired = remaining_ms <= 0, expiring = remaining_ms <= PENDING_WARN_MS,
        work_id_sha256, "channel_health"
    );
}

pub(crate) fn retirement(id: &str, attempts: u32, outcome: &'static str) {
    let work_id_sha256 = format!("{:x}", Sha256::digest(id.as_bytes()));
    tracing::info!(
        target: "cma::bridge::operations",
        operation = "retirement", outcome, attempts,
        stalled = outcome != "advanced" && attempts >= 5, work_id_sha256,
        "channel_health"
    );
}

pub(crate) fn command(record: &AdmittedAgentCommand) {
    if record.status.is_terminal() {
        return;
    }
    let envelope = &record.receipt.envelope;
    let traffic = if envelope.source.kind == AgentSourceKind::Channel
        || matches!(envelope.actor, AgentActor::Channel { .. })
    {
        "channel"
    } else {
        "ordinary"
    };
    let delivery = record
        .work
        .observation
        .as_ref()
        .map(|value| value.delivery)
        .or_else(|| record.work.dispatch.as_ref().map(|value| value.delivery));
    let outcome = command_outcome(record.status, delivery);
    let now = now_ms();
    let age_ms = timestamp_ms(&record.receipt.admitted_at).map(|at| age_ms(now, at));
    let dispatch_age_ms = record
        .work
        .dispatch
        .as_ref()
        .and_then(|value| timestamp_ms(&value.fenced_at))
        .map(|at| self::age_ms(now, at));
    tracing::info!(
        target: "cma::bridge::operations",
        operation = "command", traffic, outcome,
        age_ms, dispatch_age_ms,
        queue_overdue = outcome == "queued" && age_ms.is_some_and(|age| age >= QUEUE_WARN_MS),
        dispatch_overdue = outcome == "dispatch_ambiguous"
            && dispatch_age_ms.is_some_and(|age| age >= DISPATCH_WARN_MS),
        command_id = envelope.command_id.as_str(),
        "channel_health"
    );
}

fn command_outcome(
    status: AgentResponseStatus,
    delivery: Option<AgentOperationDelivery>,
) -> &'static str {
    match delivery {
        Some(AgentOperationDelivery::DispatchFenced | AgentOperationDelivery::RecoveryRequired) => {
            "dispatch_ambiguous"
        }
        Some(AgentOperationDelivery::RuntimeLost) => "runtime_lost",
        _ if status == AgentResponseStatus::Queued => "queued",
        _ if status == AgentResponseStatus::WaitingAction => "waiting_action",
        _ => "active",
    }
}

#[derive(Clone, Copy)]
pub(crate) enum AuthorityFailure {
    MembershipUnavailable,
    MembershipDenied,
    MembershipAdminRequired,
    HumanTokenExpired,
    HumanRefreshFailed,
    ControlGrantExpired,
    CredentialVerificationFailed,
    CredentialExpired,
}
pub(crate) fn authority_failure(failure: AuthorityFailure) {
    let reason = match failure {
        AuthorityFailure::MembershipUnavailable => "membership_unavailable",
        AuthorityFailure::MembershipDenied => "membership_denied",
        AuthorityFailure::MembershipAdminRequired => "membership_admin_required",
        AuthorityFailure::HumanTokenExpired => "human_token_expired",
        AuthorityFailure::HumanRefreshFailed => "human_refresh_failed",
        AuthorityFailure::ControlGrantExpired => "control_grant_expired",
        AuthorityFailure::CredentialVerificationFailed => "credential_verification_failed",
        AuthorityFailure::CredentialExpired => "credential_lease_expired",
    };
    tracing::info!(
        target: "cma::bridge::operations",
        operation = "authority", reason,
        "channel_health"
    );
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
