//! Durable Launch progress event vocabulary.

use super::{BlockedReason, LaunchFailure, LaunchState, RepeatChoice, RequirementState, Step};
use crate::agents::FirstMessageDelivery;
use serde::Deserialize;

string_enum! { /// Recoverable orchestration effect.
    pub enum EffectKind { CreateSandbox => "create_sandbox", ReviveSandbox => "revive_sandbox", RepairSandboxServiceGrant => "repair_sandbox_service_grant", CreateWorkspace => "create_workspace", CreateAgent => "create_agent" }
}
event_enum! { "type"; /// One durable change to a Launch.
    pub enum LaunchEventKind {
        Created => "created" {}, ConfirmationRecorded => "confirmation_recorded" {},
        StateChanged => "state_changed" { from: LaunchState, to: LaunchState },
        StepChanged => "step_changed" { step: Step },
        SandboxResolved => "sandbox_resolved" { sandbox_id: String, reused: bool },
        SandboxReplaced => "sandbox_replaced" { previous_sandbox_id: String, sandbox_id: String },
        WorkspaceResolved => "workspace_resolved" { workspace_id: String, reused: bool },
        WorkspaceReplaced => "workspace_replaced" { previous_workspace_id: String, workspace_id: String },
        AgentResolved => "agent_resolved" { agent_id: String, reused: bool },
        RepeatChoiceRecorded => "repeat_choice_recorded" { choice: RepeatChoice },
        RequirementChanged => "requirement_changed" { service: String, state: RequirementState },
        Blocked => "blocked" { reason: BlockedReason }, Resumed => "resumed" { reason: BlockedReason },
        Deferred => "deferred" { retry_at: String }, RetryScheduled => "retry_scheduled" { attempt: u32, retry_at: String },
        EffectReconciled => "effect_reconciled" { kind: EffectKind, result_id: String },
        FirstMessageChanged => "first_message_changed" { state: FirstMessageDelivery },
        Failed => "failed" { failure: LaunchFailure }, Cancelled => "cancelled" {}, Completed => "completed" {}
    }
}

/// Monotonic owner-only progress event.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LaunchEvent {
    /// Owning Launch identity.
    pub launch_id: String,
    /// Durable event sequence.
    pub seq: u64,
    /// Append timestamp.
    pub appended_at: String,
    /// Typed durable change.
    pub kind: LaunchEventKind,
}

/// Bounded progress page with an exact exclusive replay cursor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LaunchEventPage {
    /// Requested Launch identity.
    pub launch_id: String,
    /// Ordered durable events.
    pub events: Vec<LaunchEvent>,
    /// Last event sequence, or the supplied cursor for an empty page.
    pub next_cursor: u64,
}
