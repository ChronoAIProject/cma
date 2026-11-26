//! Public request and response shapes for CMA Agents.

pub(crate) use super::agent_list_shapes::{AgentListItemView, AgentPageView};
use super::agent_observation_shapes::{AgentRuntimeMetadataView, AgentTurnUsageView};
use layer2_core::profiles::{PermissionLevel, SkillSource, ThinkingEffort};
use layer2_core::sandbox::{
    Agent, AgentLineage, AgentStatus, FirstMessageDelivery, SandboxStatus, WorkspaceStatus,
    WorktreeStatus,
};
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentProfileReferenceBody {
    pub(crate) id: String,
    pub(crate) revision: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateAgentRequest {
    pub(crate) agent_profile: AgentProfileReferenceBody,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) first_message: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentTextRequest {
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalDecisionBody {
    Allow,
    Deny,
    Abort,
}

impl From<ApprovalDecisionBody> for layer2_core::sandbox::AgentApprovalDecision {
    fn from(value: ApprovalDecisionBody) -> Self {
        match value {
            ApprovalDecisionBody::Allow => Self::Allow,
            ApprovalDecisionBody::Deny => Self::Deny,
            ApprovalDecisionBody::Abort => Self::Abort,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApprovalDecisionRequest {
    pub(crate) decision: ApprovalDecisionBody,
}

/// Owner stop requests have one semantic reason. Operational failure reasons
/// are assigned by the core and cannot be forged through the owner API.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OwnerStopReasonBody {
    Owner,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StopAgentRequest {
    #[serde(default)]
    pub(crate) reason: Option<OwnerStopReasonBody>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentProfileSummaryView {
    id: String,
    revision: u32,
    digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[schema(value_type = String)]
    thinking_effort: ThinkingEffort,
    #[schema(value_type = String)]
    permission_level: PermissionLevel,
    skills: Vec<AgentSkillSummaryView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentSkillSummaryView {
    source: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

impl From<&Agent> for AgentProfileSummaryView {
    fn from(agent: &Agent) -> Self {
        Self {
            id: agent.profile.id.to_string(),
            revision: agent.profile.revision,
            digest: agent.profile.digest.clone(),
            model: agent.profile.content.model.clone(),
            thinking_effort: agent.profile.content.thinking_effort,
            permission_level: agent.profile.content.permission_level,
            skills: agent
                .profile
                .content
                .skills
                .iter()
                .map(|skill| AgentSkillSummaryView {
                    source: match skill.source {
                        SkillSource::Ornn => "ornn".into(),
                    },
                    id: skill.id.to_string(),
                    version: skill.version.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WorktreeSummaryView {
    id: String,
    name: String,
    path: String,
    branch: String,
    base_commit: String,
    #[schema(value_type = String)]
    status: WorktreeStatus,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WorkspaceContextView {
    id: String,
    name: String,
    path: String,
    #[schema(value_type = String)]
    status: WorkspaceStatus,
    profile: InstanceProfileReferenceView,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<super::instances_openapi_shapes::WorkspaceSourceView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SandboxContextView {
    id: String,
    name: String,
    #[schema(value_type = String)]
    status: SandboxStatus,
    profile: SandboxProfileReferenceView,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SandboxProfileReferenceView {
    #[serde(flatten)]
    reference: InstanceProfileReferenceView,
    /// Whether this caller could read the exact pinned revision through the public
    /// Sandbox Profile API at the time of this response. Omitted when undetermined.
    #[serde(skip_serializing_if = "Option::is_none")]
    readable: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct InstanceProfileReferenceView {
    id: String,
    revision: u32,
    digest: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct FirstMessageStateView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    purpose: Option<layer2_core::sandbox::FirstMessagePurpose>,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_sequence: Option<u64>,
    #[schema(value_type = String)]
    state: FirstMessageDelivery,
    queued_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentBootstrapView {
    operation_id: String,
    sandbox_id: String,
    workspace_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentView {
    #[serde(skip_serializing_if = "Option::is_none")]
    task_acceptance: Option<super::task_acceptance_shapes::TaskAcceptanceView>,
    /// Browser page for this CMA Agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) callback_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_metadata: Option<AgentRuntimeMetadataView>,
    pub(super) id: String,
    #[schema(value_type = String)]
    status: AgentStatus,
    profile: AgentProfileSummaryView,
    worktree: WorktreeSummaryView,
    workspace: WorkspaceContextView,
    sandbox: SandboxContextView,
    /// Whether the immutable Sandbox profile requested the desktop session.
    desktop: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    bootstrap: Option<AgentBootstrapView>,
    conforms_to: Vec<&'static str>,
    reach: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    paused_notice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_message: Option<FirstMessageStateView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<AgentFailureView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_reason: Option<String>,
    /// The owner may request a restart; admission still verifies the retained runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) restart_available: Option<bool>,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ready_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deleted_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentFailureView {
    class: String,
    detail: String,
    remediation: String,
}

impl AgentView {
    pub(super) fn with_sandbox_profile_readability(mut self, readable: Option<bool>) -> Self {
        self.sandbox.profile.readable = readable;
        self
    }

    pub(crate) fn from_lineage(lineage: AgentLineage, paused_notice: Option<String>) -> Self {
        let AgentLineage {
            agent,
            worktree,
            workspace,
            sandbox,
        } = lineage;
        let bootstrap = agent
            .bootstrap_lineage(&sandbox, &workspace)
            .map(|operation_id| AgentBootstrapView {
                operation_id: operation_id.into(),
                sandbox_id: sandbox.id.to_string(),
                workspace_id: workspace.id.to_string(),
            });
        let first_message =
            agent
                .conversation
                .first_message
                .as_ref()
                .map(|message| FirstMessageStateView {
                    state: message.state,
                    purpose: message.purpose,
                    turn_sequence: message.turn_sequence,
                    queued_at: message.queued_at.clone(),
                    completed_at: message.completed_at.clone(),
                    failed_at: message.failed_at.clone(),
                });
        Self {
            callback_url: None,
            task_acceptance: agent
                .conversation
                .observations
                .task_acceptance
                .as_ref()
                .map(|task| task.summary().into()),
            id: agent.id.to_string(),
            desktop: !agent.profile.content.isolated_agent
                && sandbox
                    .profile
                    .content
                    .environment_variables
                    .iter()
                    .any(|variable| {
                        variable.name == "CMA_MACHINE_INCLUDED" && variable.value == "1"
                    }),
            bootstrap,
            status: agent.status,
            profile: (&agent).into(),
            runtime_metadata: AgentRuntimeMetadataView::from_agent(&agent),
            worktree: WorktreeSummaryView {
                id: worktree.id.to_string(),
                name: worktree.name,
                path: worktree.path,
                branch: worktree.branch,
                base_commit: worktree.base_commit,
                status: worktree.status,
            },
            workspace: WorkspaceContextView {
                id: workspace.id.to_string(),
                name: workspace.name,
                path: workspace.path,
                status: workspace.status,
                profile: InstanceProfileReferenceView {
                    id: workspace.profile.id.to_string(),
                    revision: workspace.profile.revision,
                    digest: workspace.profile.digest,
                },
                resolved_commit: workspace.resolved_commit,
                source: matches!(
                    workspace.source,
                    layer2_core::sandbox::WorkspaceSource::RepositorySnapshot { .. }
                        | layer2_core::sandbox::WorkspaceSource::PublicRepositorySnapshot { .. }
                )
                .then(|| {
                    super::instances_openapi_shapes::WorkspaceSourceView::from(&workspace.source)
                }),
            },
            sandbox: SandboxContextView {
                id: sandbox.id.to_string(),
                name: sandbox.name,
                status: sandbox.status,
                profile: SandboxProfileReferenceView {
                    reference: InstanceProfileReferenceView {
                        id: sandbox.profile.id.to_string(),
                        revision: sandbox.profile.revision,
                        digest: sandbox.profile.digest,
                    },
                    readable: None,
                },
            },
            conforms_to: agent_conformance(&agent.capabilities),
            reach: sandbox
                .credential
                .effective_reaches()
                .into_iter()
                .map(|service| service.to_string())
                .collect(),
            owner_title: agent.conversation.preferences.title,
            title: agent.title,
            paused_notice,
            first_message,
            failure: agent.failure.map(|failure| AgentFailureView {
                remediation: super::remediation::guidance(&failure.class).into(),
                class: failure.class,
                detail: failure.detail,
            }),
            stop_reason: agent.stop_reason.map(|reason| reason.as_str().into()),
            restart_available: None,
            created_at: agent.created_at,
            updated_at: agent.updated_at,
            ready_at: agent.ready_at,
            stopped_at: agent.stopped_at,
            deleted_at: agent.deleted_at,
        }
    }
}

fn agent_conformance(capabilities: &RuntimeCapabilities) -> Vec<&'static str> {
    let mut classes = vec![
        concat!("https://spec.chrono-ai.fun/cma/1.0/conf", "/core"),
        concat!("https://spec.chrono-ai.fun/cma/1.0/conf", "/approvals"),
        concat!("https://spec.chrono-ai.fun/cma/1.0/conf", "/events-replay"),
    ];
    for (capability, class) in [
        (
            RuntimeCapability::Conversation,
            concat!("https://spec.chrono-ai.fun/cma/1.0/conf", "/conversation"),
        ),
        (
            RuntimeCapability::Steer,
            concat!("https://spec.chrono-ai.fun/cma/1.0/conf", "/steering"),
        ),
        (
            RuntimeCapability::Interrupt,
            concat!("https://spec.chrono-ai.fun/cma/1.0/conf", "/interrupt"),
        ),
        (
            RuntimeCapability::Usage,
            concat!("https://spec.chrono-ai.fun/cma/1.0/conf", "/usage"),
        ),
        (
            RuntimeCapability::Resources,
            concat!("https://spec.chrono-ai.fun/cma/1.0/conf", "/resources"),
        ),
    ] {
        if capabilities.supports(capability) {
            classes.push(class);
        }
    }
    classes
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct InputAcceptedView {
    pub(crate) queued: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentEventPageView {
    pub(crate) items: Vec<AgentEventView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentEventView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) retained_output: Option<super::agent_retained_output::RetainedOutputAvailability>,
    pub(crate) seq: u64,
    pub(crate) appended_at: String,
    #[schema(value_type = Object)]
    pub(crate) kind: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentTranscriptView {
    pub(crate) agent_id: String,
    pub(crate) through_seq: u64,
    pub(crate) turns: Vec<AgentTranscriptTurnView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub(crate) next: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentTranscriptTurnView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) retained_output: Option<super::agent_retained_output::RetainedOutputAvailability>,
    pub(crate) turn_sequence: u64,
    pub(crate) started_at: String,
    pub(crate) owner_message: String,
    pub(crate) agent_messages: Vec<String>,
    pub(crate) messages: Vec<AgentTranscriptMessageView>,
    #[schema(value_type = Object)]
    pub(crate) outcome: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentTranscriptMessageView {
    pub(crate) sequence: u64,
    pub(crate) role: AgentTranscriptRoleView,
    pub(crate) content: String,
    pub(crate) appended_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentTranscriptRoleView {
    User,
    Assistant,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentUsageView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) current_turn: Option<AgentTurnUsageView>,
    pub(crate) agent_id: String,
    pub(crate) through_seq: u64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) context_used: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) context_window: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentResourcesView {
    pub(crate) agent_id: String,
    pub(crate) sandbox_id: String,
    pub(crate) uptime_ms: u64,
    pub(crate) cpu_usage_usec: Option<u64>,
    pub(crate) cpu_millis_limit: Option<u32>,
    pub(crate) memory_bytes_used: Option<u64>,
    pub(crate) memory_bytes_limit: Option<u64>,
    pub(crate) memory_bytes_peak: Option<u64>,
    pub(crate) stall_permille: Option<u16>,
    pub(crate) ceiling_hits: Option<u64>,
    pub(crate) allocation_failures: Option<u64>,
    pub(crate) kills: Option<u64>,
    pub(crate) received_bytes: Option<u64>,
    pub(crate) sent_bytes: Option<u64>,
}
