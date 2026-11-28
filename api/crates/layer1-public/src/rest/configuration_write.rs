use axum::{
    http::{header, HeaderMap},
    response::Response,
};
use layer2_core::sandbox::{AgentConfigurationRead, AgentOperationError, AgentReadService};
use layer2_core::{AgentId, Caller};

use crate::{AuthenticatedResourceCaller, DelegatedCredential};

use super::InstanceProblem;

pub(in crate::rest) fn wants_reasons(headers: &HeaderMap) -> bool {
    headers
        .get("prefer")
        .is_some_and(|value| value == "cma-settings-reasons")
}

pub(in crate::rest) fn private_response(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    response
}

#[derive(Clone, Copy)]
pub(crate) enum ConfigurationWriteReason {
    DispatchInProgress,
    DispatchRecoveryRequired,
    StaleRevision,
    AdmissionConflict,
    AgentNotIdle,
    OutcomeUnconfirmed,
}

impl ConfigurationWriteReason {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::DispatchInProgress => "dispatch_in_progress",
            Self::DispatchRecoveryRequired => "dispatch_recovery_required",
            Self::StaleRevision => "stale_revision",
            Self::AdmissionConflict => "admission_conflict",
            Self::AgentNotIdle => "agent_not_idle",
            Self::OutcomeUnconfirmed => "outcome_unconfirmed",
        }
    }

    pub(super) const fn detail(self) -> &'static str {
        match self {
            Self::DispatchInProgress => {
                "A dispatch was in progress when this update was refused. Wait for it to settle and refresh before applying again."
            }
            Self::DispatchRecoveryRequired => {
                "A prior dispatch required reconciliation when this update was refused. Refresh after it is reconciled."
            }
            Self::StaleRevision => {
                "The preferences revision changed before this update. Refresh preferences and apply again."
            }
            Self::AdmissionConflict => {
                "The Agent changed during admission of this update. Refresh before applying again."
            }
            Self::AgentNotIdle => {
                "The Agent was not eligible for this update. Refresh when it is ready for configuration changes."
            }
            Self::OutcomeUnconfirmed => {
                "The update outcome is unconfirmed. Retry with the original request and idempotency key."
            }
        }
    }
}

/// Classify only the original attempt. The command receipt retains this reason
/// even when the Agent's state changes before an exact-key replay.
pub(in crate::rest) fn operation_problem(
    error: AgentOperationError,
    reason_requested: bool,
) -> InstanceProblem {
    use ConfigurationWriteReason as Reason;
    let reason = match &error {
        AgentOperationError::DispatchInProgress { .. } => Some(Reason::DispatchInProgress),
        AgentOperationError::DispatchRecoveryRequired { .. } => {
            Some(Reason::DispatchRecoveryRequired)
        }
        AgentOperationError::PreferencesRevisionMismatch => Some(Reason::StaleRevision),
        AgentOperationError::ConfigurationAdmissionConflict => Some(Reason::AdmissionConflict),
        AgentOperationError::StatusConflict { .. }
        | AgentOperationError::FirstMessageReserved { .. } => Some(Reason::AgentNotIdle),
        AgentOperationError::Conflict
        | AgentOperationError::PreviousTurnConflict
        | AgentOperationError::DeliveryAmbiguous { .. }
        | AgentOperationError::OutcomeUnavailable(_)
        | AgentOperationError::RuntimeAttachmentUnavailable(_)
        | AgentOperationError::RuntimeUnavailable(_)
        | AgentOperationError::Unavailable(_)
        | AgentOperationError::Corrupt(_)
        | AgentOperationError::LogCapacityExceeded => Some(Reason::OutcomeUnconfirmed),
        _ => None,
    };
    let problem = InstanceProblem::from(error);
    match reason {
        Some(reason) => {
            // The request span already carries a bounded request hash.
            tracing::info!(
                reason = reason.as_str(),
                "Agent configuration write did not confirm"
            );
            if reason_requested {
                InstanceProblem::ConfigurationWrite(Box::new(problem), reason)
            } else {
                problem
            }
        }
        None => problem,
    }
}

/// A cached problem is still an Agent resource projection. Recheck current
/// access before the idempotency store can return it to the caller.
pub(in crate::rest) async fn authorize_receipt(
    reads: &AgentReadService,
    caller: &Caller,
    resource: Option<&AuthenticatedResourceCaller>,
    id: &AgentId,
) -> Result<Option<AgentConfigurationRead>, InstanceProblem> {
    if let Some(resource) = resource {
        let read = reads
            .configuration_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                id,
            )
            .await?;
        Ok(Some(read))
    } else {
        reads.lineage(caller, id).await?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, response::IntoResponse};

    use super::*;
    use layer2_core::sandbox::AgentStatus;

    async fn body(problem: InstanceProblem) -> (axum::http::StatusCode, serde_json::Value) {
        let response = problem.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn configuration_reasons_preserve_legacy_codes_without_exposing_diagnostics() {
        for (error, reason) in [
            (
                AgentOperationError::DispatchRecoveryRequired {
                    operation_sequence: 81,
                },
                "dispatch_recovery_required",
            ),
            (
                AgentOperationError::StatusConflict {
                    status: AgentStatus::Working,
                },
                "agent_not_idle",
            ),
            (
                AgentOperationError::ConfigurationAdmissionConflict,
                "admission_conflict",
            ),
            (AgentOperationError::Conflict, "outcome_unconfirmed"),
            (
                AgentOperationError::DeliveryAmbiguous {
                    operation_sequence: 81,
                    detail: "synthetic diagnostic".into(),
                },
                "outcome_unconfirmed",
            ),
        ] {
            let (legacy_status, legacy) = body(InstanceProblem::from(error.clone())).await;
            assert_eq!(
                body(operation_problem(error.clone(), false)).await,
                (legacy_status, legacy.clone())
            );
            let (status, configuration) = body(operation_problem(error, true)).await;
            assert_eq!(legacy_status, status);
            assert_eq!(legacy["code"], configuration["code"]);
            assert!(legacy.get("settings_update_reason").is_none());
            assert_eq!(configuration["settings_update_reason"], reason);
            assert!(!configuration.to_string().contains("synthetic diagnostic"));
            assert!(configuration.get("operation_sequence").is_none());
        }
    }
}
