//! Durable visitor Launch state and admission requests.

use super::connections::Requirements;
use crate::{agents::FirstMessageDelivery, profiles::EnvironmentVariable};
use serde::{Deserialize, Serialize};

string_enum! { /// Durable Launch lifecycle.
    pub enum LaunchState {
        Draft => "draft", Resolving => "resolving", SandboxPending => "sandbox_pending",
        WorkspacePending => "workspace_pending", AgentPending => "agent_pending",
        MessagePending => "message_pending", Ready => "ready", Completed => "completed",
        FailedRetryable => "failed_retryable", FailedTerminal => "failed_terminal", Cancelled => "cancelled"
    }
}
string_enum! { /// Visitor-facing progress step.
    pub enum StepKey { SigningIn => "signing_in", CheckingRequirements => "checking_requirements", Sandbox => "sandbox", Workspace => "workspace", Agent => "agent", FirstMessage => "first_message", Ready => "ready" }
}
string_enum! { /// Durable progress state of one step.
    pub enum StepStatus { Pending => "pending", Running => "running", Completed => "completed", Blocked => "blocked", Retryable => "retryable", Failed => "failed" }
}
string_enum! { /// Visitor's explicit repeat-launch decision.
    pub enum RepeatChoice { Resume => "resume", New => "new" }
}
string_enum! { /// Surface on which a visitor supplied consent.
    pub enum ConfirmationSurface { Landing => "landing", Review => "review" }
}
string_enum! { /// Resource covered by a trust disclosure.
    pub enum TrustResource { Sandbox => "sandbox", Workspace => "workspace" }
}

/// One named progress step.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Step {
    /// Stable step key.
    pub key: StepKey,
    /// Current progress state.
    pub status: StepStatus,
    /// Safe visitor-facing detail.
    pub safe_detail: String,
    /// Step start timestamp.
    pub started_at: Option<String>,
    /// Step completion timestamp.
    pub completed_at: Option<String>,
}

event_enum! { "type"; /// Requirement that prevents immediate admission.
    pub enum ReviewNeed {
        Connection => "connection" { service: String },
        TrustConfirmation => "trust_confirmation" { commands_digest: String },
        OwnerRequiresReview => "owner_requires_review" {},
        RepeatChoice => "repeat_choice" {}
    }
}
event_enum! { "type"; /// Durable reason orchestration is currently blocked.
    pub enum BlockedReason {
        ConnectionRequired => "connection_required" { service: String },
        OrchestrationPaused => "orchestration_paused" {},
        SandboxPausedReviving => "sandbox_paused_reviving" {},
        RecoveryRequired => "recovery_required" {}
    }
}
event_enum! { "state"; /// Resolution of one visitor-owned resource.
    pub enum ResolvedResource {
        Pending => "pending" {}, Reused => "reused" { id: String }, Created => "created" { id: String }
    }
}

/// Stable safe failure evidence, excluding provider payloads and credentials.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LaunchFailure {
    /// Machine-readable failure code.
    pub code: String,
    /// Safe public detail.
    pub safe_detail: String,
    /// Whether supported recovery may retry this operation.
    pub retryable: bool,
    /// Failure timestamp.
    pub occurred_at: String,
    /// Optional owner remediation.
    pub remediation: Option<String>,
}

/// Resolved hierarchy and outstanding repeat choice.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Resolution {
    /// Visitor Sandbox resolution.
    pub sandbox: ResolvedResource,
    /// Visitor Workspace resolution.
    pub workspace: ResolvedResource,
    /// Visitor Agent resolution.
    pub agent: ResolvedResource,
    /// Whether admission needs an explicit repeat choice.
    pub choice_required: bool,
}

/// Review requirements computed by the server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Review {
    /// Outstanding visitor requirements.
    pub needs: Vec<ReviewNeed>,
    /// Whether explicit start consent can currently be accepted.
    pub can_start_now: bool,
}

/// Exact commands and plain configuration covered by visitor confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TrustDisclosure {
    /// Resource receiving these commands.
    pub resource: TrustResource,
    /// Resource display name.
    pub name: String,
    /// Command digest.
    pub commands_digest: String,
    /// Exact commands to review.
    pub commands: Vec<String>,
    /// Plain environment configuration.
    pub environment_variables: Vec<EnvironmentVariable>,
}

/// Frozen command review state for one Launch.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Trust {
    /// Trust contract version.
    pub version: u32,
    /// Combined confirmation digest when confirmation is required.
    pub confirmation_digest: Option<String>,
    /// Sandbox command digest.
    pub sandbox_digest: Option<String>,
    /// Workspace command digest.
    pub workspace_digest: Option<String>,
    /// Exact review disclosures.
    pub disclosures: Vec<TrustDisclosure>,
}

/// Durable record of explicit visitor consent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Confirmation {
    /// UI surface on which consent was supplied.
    pub surface: ConfirmationSurface,
    /// Reviewed command digest, when required.
    pub trust_confirmed_digest: Option<String>,
    /// Explicit acknowledged review items.
    pub acknowledgements: Vec<String>,
    /// Consent timestamp.
    pub confirmed_at: String,
}

/// Immutable association to an admitted Trigger first-message response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FirstMessageCommand {
    /// Existing Agent conversation identity.
    pub agent_id: String,
    /// Common command identity for authorized status reads.
    pub command_id: String,
    /// The one admitted response; reading it does not resend input.
    pub response_id: String,
}

/// Complete owner-only Launch document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Launch {
    /// Launch identity.
    pub id: String,
    /// Source Trigger owner identity.
    pub trigger_id: String,
    /// Immutable Trigger revision digest.
    pub revision_digest: String,
    /// Public Trigger identity.
    pub public_id: String,
    /// Attributed placement, when valid.
    pub placement_id: Option<String>,
    /// Attributed public origin, when valid.
    pub referrer_origin: Option<String>,
    /// Durable lifecycle.
    pub state: LaunchState,
    /// Progress steps.
    pub steps: Vec<Step>,
    /// Visitor-owned resource resolution.
    pub resolution: Resolution,
    /// Durable connection requirement states.
    pub requirements: Requirements,
    /// Current review admission state.
    pub review: Review,
    /// Frozen trust disclosure.
    pub trust: Trust,
    /// First-message delivery state.
    pub first_message: FirstMessageDelivery,
    /// Omitted for legacy launches and reuse that attached no prompt.
    pub first_message_command: Option<FirstMessageCommand>,
    /// Recorded explicit confirmations.
    pub confirmations: Vec<Confirmation>,
    /// Current orchestration blocker.
    pub blocked_reason: Option<BlockedReason>,
    /// Last safe failure.
    pub failure: Option<LaunchFailure>,
    /// Latest durable event sequence.
    pub event_head: u64,
    /// Creation timestamp.
    pub created_at: String,
    /// Latest update timestamp.
    pub updated_at: String,
    /// Admission timestamp.
    pub started_at: Option<String>,
    /// Agent readiness timestamp.
    pub ready_at: Option<String>,
    /// Durable optimistic version.
    pub revision: u64,
}

/// Explicit visitor admission or resumable review creation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateLaunchRequest {
    /// Registered placement identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<String>,
    /// Public referrer origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referrer_origin: Option<String>,
    /// Explicit consent to start immediately only if review requirements permit.
    pub start_if_ready: bool,
    /// Explicit repeat decision when required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<RepeatChoice>,
}

/// Explicit consent after reading the current Launch review.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartLaunchRequest {
    /// Exact reviewed command digest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_confirmed_digest: Option<String>,
    /// Review items explicitly acknowledged by the visitor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acknowledgements: Vec<String>,
}

/// Existing Agent evidence carried by `trigger_repeat_choice_required`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ExistingLaunchAgent {
    /// Existing visitor-owned Agent identity.
    pub agent_id: String,
    /// Owner title.
    pub title: Option<String>,
    /// Agent creation timestamp.
    pub created_at: String,
    /// Safe first-message excerpt.
    pub first_message_excerpt: String,
}

/// Extract structured repeat-choice evidence without interpreting problem prose.
pub fn repeat_choice_from(error: &crate::Error) -> crate::Result<Option<ExistingLaunchAgent>> {
    match error {
        crate::Error::Api {
            code: Some(code),
            extensions,
            ..
        } if code == "trigger_repeat_choice_required" => {
            serde_json::from_value(extensions.get("existing").cloned().unwrap_or_default())
                .map(Some)
                .map_err(|_| {
                    crate::Error::protocol("repeat-choice problem omitted existing Agent evidence")
                })
        }
        _ => Ok(None),
    }
}
