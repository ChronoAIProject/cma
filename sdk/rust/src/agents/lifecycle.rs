//! Durable inline Agent provisioning and owner-visible recovery.

use super::Agents;
use crate::{
    IdempotencyKey, Result,
    profiles::{PermissionLevel, ProfileReference, ThinkingEffort},
    resources::identity,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

/// Inline configuration; omitted image and profile options use deployment defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecycleCreateRequest {
    /// Agent display name.
    pub name: String,
    /// Exact initial task, up to 262144 UTF-8 bytes; omission retains setup behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<String>,
    /// Prepare the Agent browser before its first task; omission preserves existing behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<bool>,
    /// Provision an immutable Linux desktop runtime shared by the owner and Agent.
    #[serde(default, skip_serializing_if = "is_false")]
    pub machine_included: bool,
    /// Existing owned Workspace; inherits its image, repository and Sandbox scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Existing owned Sandbox; creates an empty workspace for the new Agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,
    /// Retained for compatibility; the server rejects custom image selection with
    /// `image_selection_unsupported`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<AgentLifecycleImage>,
    /// Optional GitHub workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<AgentLifecycleRepository>,
    /// Inline model, skills, instructions and service access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<AgentLifecycleProfile>,
}

/// Retains historical custom image identities and versions; the server rejects new selections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecycleImage {
    /// Image identity.
    pub id: String,
    /// Published version; omission resolves the current version at first admission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// GitHub repository to prepare in the new Sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecycleRepository {
    /// HTTPS GitHub repository URL.
    pub url: String,
    /// Optional branch, tag or commit.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// Optional overrides for the deployment's Agent defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecycleProfile {
    /// Explicit model name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Model reasoning effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ThinkingEffort>,
    /// Agent approval policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionLevel>,
    /// Additional instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Ornn skills and optional published versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<AgentLifecycleSkill>>,
    /// Exact service access requested for the Agent key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<AgentLifecyclePermissions>,
}

/// Ornn skill selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecycleSkill {
    /// Catalogue identity.
    pub id: String,
    /// Published version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Service access requested for the Agent key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecyclePermissions {
    /// Exact NyxID service identities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<String>>,
}

string_enum! { /// Admission acknowledgement; readiness is reported by the operation.
    pub enum AgentLifecycleReceiptStatus { Provisioning => "provisioning" }
}
string_enum! { /// Global operation state, independent of phase readiness.
    pub enum AgentLifecycleStatus {
        Queued => "queued", Running => "running", WaitingForConnection => "waiting_for_connection",
        Succeeded => "succeeded", Failed => "failed"
    }
}

/// Durable receipt for an admitted or resumed request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecycleCreateReceipt {
    /// Operation identity to poll.
    pub id: String,
    /// Admission acknowledgement.
    pub status: AgentLifecycleReceiptStatus,
}

/// Owner-bound resource for service connection recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLifecycleConnectionTarget {
    /// Pre-admission requirements from a pinned internal Sandbox configuration.
    SandboxProfile {
        /// Immutable profile identity.
        profile: ProfileReference,
    },
    /// Existing Sandbox requirements.
    Sandbox {
        /// Sandbox identity.
        id: String,
    },
    /// Existing Workspace requirements.
    Workspace {
        /// Workspace identity.
        id: String,
    },
    /// Existing Agent requirements.
    Agent {
        /// Agent identity.
        id: String,
    },
    /// Trigger Launch requirements.
    Launch {
        /// Launch identity.
        id: String,
    },
}

/// Services that must be connected before resuming the identical request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecycleConnectionRequired {
    /// Missing service identities.
    pub services: Vec<String>,
    /// Owner-bound connection recovery target.
    pub target: AgentLifecycleConnectionTarget,
}

/// One durable provisioning phase event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecycleEvent {
    /// Monotonic event sequence.
    pub sequence: u64,
    /// Provisioning gate.
    pub phase: String,
    /// Per-phase state; ready does not imply global success.
    pub status: String,
    /// Safe human-readable progress.
    pub message: String,
    /// RFC 3339 event timestamp.
    pub occurred_at: String,
}

/// Owner-visible durable provisioning progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLifecycleOperation {
    /// Browser page for the actual CMA Agent, when available.
    #[serde(default)]
    pub callback_url: Option<String>,
    /// Durable operation identity.
    pub id: String,
    /// Global provisioning state.
    pub status: AgentLifecycleStatus,
    /// Current provisioning gate.
    pub phase: String,
    /// Admitted Agent; wait for succeeded before opening it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Safe terminal failure detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Durable phase events.
    pub events: Vec<AgentLifecycleEvent>,
    /// Structured connection recovery when provisioning is waiting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<AgentLifecycleConnectionRequired>,
}

impl Agents<'_> {
    /// Admit or resume the identical inline request with its original key.
    pub async fn create_lifecycle(
        &self,
        body: &AgentLifecycleCreateRequest,
        key: &IdempotencyKey,
    ) -> Result<AgentLifecycleCreateReceipt> {
        self.client
            .command(
                Method::POST,
                &["agents"],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await
    }

    /// Read owner-visible progress; succeeded means the Agent is ready.
    pub async fn read_lifecycle(&self, id: &str) -> Result<AgentLifecycleOperation> {
        let value: AgentLifecycleOperation = self.client.get(&["agent-lifecycle", id], &[]).await?;
        identity(&value.id, id)?;
        Ok(value)
    }
}
