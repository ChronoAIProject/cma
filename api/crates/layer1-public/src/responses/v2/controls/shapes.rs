use layer2_core::sandbox::{
    agent_io::{
        CommandStatusView, ResponseControlObservation, TargetedControlStatusView, TargetedResponse,
    },
    AgentApprovalDecision,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct V2ResponseControlRequest {
    /// Bound turn from the controls observation; retain unchanged on retry.
    #[schema(minimum = 1, maximum = 9223372036854775807_u64)]
    pub turn_sequence: u64,
    /// Agent aggregate revision, distinct from profile revision.
    #[schema(minimum = 1, maximum = 9223372036854775807_u64)]
    pub expected_agent_revision: u64,
    pub command: V2ResponseControlCommand,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum V2ResponseControlCommand {
    Cancel {},
    Steer {
        text: String,
    },
    ResolveApproval {
        approval_id: String,
        decision: V2ApprovalDecision,
    },
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum V2ApprovalDecision {
    Allow,
    Deny,
    Abort,
}

impl From<V2ApprovalDecision> for AgentApprovalDecision {
    fn from(value: V2ApprovalDecision) -> Self {
        match value {
            V2ApprovalDecision::Allow => Self::Allow,
            V2ApprovalDecision::Deny => Self::Deny,
            V2ApprovalDecision::Abort => Self::Abort,
        }
    }
}

macro_rules! public_enum {
    ($name:ident, $core:path, [$($variant:ident),+ $(,)?]) => {
        #[derive(Debug, Serialize, ToSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl From<$core> for $name {
            fn from(value: $core) -> Self {
                use $core as Core;
                match value { $(Core::$variant => Self::$variant),+ }
            }
        }
    };
}
public_enum!(
    V2CommandState,
    layer2_core::sandbox::agent_io::AgentResponseStatus,
    [
        Queued,
        InProgress,
        WaitingAction,
        Completed,
        Failed,
        Cancelled
    ]
);
public_enum!(
    V2CommandDelivery,
    layer2_core::sandbox::AgentOperationDelivery,
    [
        DispatchFenced,
        WriteConfirmed,
        ExecutionObserved,
        PreWriteFailed,
        RecoveryRequired,
        RuntimeLost
    ]
);
public_enum!(
    V2CommandFailure,
    layer2_core::sandbox::agent_io::CommandFailureCode,
    [
        QueueTimeout,
        AuthorityRevoked,
        AuthorityUnavailable,
        CredentialSetupRequired,
        ConnectionsRequired,
        AgentUnavailable,
        TargetTurnChanged,
        RuntimeUnavailable,
        RuntimeLost,
        ExecutionFailed,
        OwnerBudgetExhausted,
        TurnBudgetExhausted,
        BudgetUnavailable
    ]
);

#[derive(Debug, Serialize, ToSchema)]
pub struct V2CommandReceipt {
    pub command_id: String,
    pub agent_id: String,
    /// Present for a message command. Controls have a separate explicit target.
    pub response_id: Option<String>,
    pub admitted_at: String,
    pub agent_order: u64,
    pub event_cursor: u64,
}

/// Command completion means delivery; inspect the target response's durable
/// status separately to establish actual cancellation or completion.
#[derive(Debug, Serialize, ToSchema)]
pub struct V2CommandStatus {
    pub object: &'static str,
    pub receipt: V2CommandReceipt,
    pub status: V2CommandState,
    pub queue_deadline: String,
    pub turn_sequence: Option<u64>,
    pub observed_cursor: Option<u64>,
    pub delivery: Option<V2CommandDelivery>,
    pub failure: Option<V2CommandFailure>,
}

impl From<CommandStatusView> for V2CommandStatus {
    fn from(value: CommandStatusView) -> Self {
        let receipt = value.receipt;
        Self {
            object: "cma.agent.command",
            receipt: V2CommandReceipt {
                command_id: receipt.command_id.to_string(),
                agent_id: receipt.agent_id.to_string(),
                response_id: receipt.response_id.map(|id| id.to_string()),
                admitted_at: receipt.admitted_at,
                agent_order: receipt.agent_order,
                event_cursor: receipt.event_cursor,
            },
            status: value.status.into(),
            queue_deadline: value.queue_deadline,
            turn_sequence: value.turn_sequence,
            observed_cursor: value.observed_cursor,
            delivery: value.delivery.map(Into::into),
            failure: value.failure.map(|code| {
                if code.budget_error().is_some() {
                    code.into()
                } else {
                    code.legacy_code().into()
                }
            }),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct V2ResponseControlTarget {
    pub response_id: String,
    pub turn_sequence: u64,
}
impl From<TargetedResponse> for V2ResponseControlTarget {
    fn from(value: TargetedResponse) -> Self {
        Self {
            response_id: value.response_id.to_string(),
            turn_sequence: value.turn_sequence,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct V2ResponseControl {
    pub object: &'static str,
    pub control: V2CommandStatus,
    pub target: V2ResponseControlTarget,
    pub expected_agent_revision: u64,
}
impl From<TargetedControlStatusView> for V2ResponseControl {
    fn from(value: TargetedControlStatusView) -> Self {
        Self {
            object: "cma.response.control",
            control: value.control.into(),
            target: value.target.into(),
            expected_agent_revision: value.expected_agent_revision,
        }
    }
}

/// Observed runtime support is neither caller authorization nor a promise that
/// the response/approval will still be active when a control is admitted.
#[derive(Debug, Serialize, ToSchema)]
pub struct V2ResponseControlRuntimeSupport {
    pub cancel: bool,
    pub steer: bool,
    pub resolve_approval: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct V2ResponseControls {
    pub object: &'static str,
    pub response: V2CommandStatus,
    /// Null until durable response dispatch binds its original turn.
    pub target: Option<V2ResponseControlTarget>,
    pub agent_revision: u64,
    pub target_active: bool,
    pub runtime_support: V2ResponseControlRuntimeSupport,
}
impl From<ResponseControlObservation> for V2ResponseControls {
    fn from(value: ResponseControlObservation) -> Self {
        Self {
            object: "cma.response.controls",
            response: value.response.into(),
            target: value.target.map(Into::into),
            agent_revision: value.agent_revision,
            target_active: value.target_active,
            runtime_support: V2ResponseControlRuntimeSupport {
                cancel: value.runtime_support.cancel,
                steer: value.runtime_support.steer,
                resolve_approval: value.runtime_support.resolve_approval,
            },
        }
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use layer2_core::sandbox::agent_io::{
        AgentResponseStatus, CommandFailureCode, CommandReceiptView,
    };

    #[test]
    fn retained_precise_failures_project_to_legacy_status_without_changing_the_read() {
        for failure in [
            CommandFailureCode::CredentialSetupRequired,
            CommandFailureCode::ConnectionsRequired,
        ] {
            let retained = CommandStatusView {
                receipt: CommandReceiptView {
                    command_id: layer2_core::ids::AgentCommandId("cmd_compat".into()),
                    agent_id: layer2_core::AgentId("agt_compat".into()),
                    response_id: None,
                    admitted_at: "2026-11-06T00:00:00Z".into(),
                    agent_order: 1,
                    event_cursor: 1,
                },
                status: AgentResponseStatus::Failed,
                queue_deadline: "2026-11-06T00:05:00Z".into(),
                turn_sequence: None,
                observed_cursor: None,
                delivery: None,
                failure: Some(failure),
            };
            let wire = serde_json::to_value(V2CommandStatus::from(retained.clone())).unwrap();
            assert_eq!(wire["failure"], "execution_failed");
            assert_eq!(retained.failure, Some(failure));
        }
    }
}
