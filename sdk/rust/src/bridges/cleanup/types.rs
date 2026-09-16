use super::super::BridgeAgentTarget;
use crate::resource_access::ResourceActor;
use serde::{Deserialize, Deserializer, Serialize};

string_enum! { /// Explicit effect requested for one exact retired child.
    pub enum BridgeChildCleanupAction { RemoveChild => "remove_child" }
}
string_enum! { /// Latest proved progress; acceptance alone does not confirm removal.
    pub enum BridgeChildCleanupProgress { NotRequested => "not_requested", Requested => "requested", AdmissionClosed => "admission_closed", RemovalStarted => "removal_started", ProcessClosed => "process_closed", HomeRemoved => "home_removed", WorktreeRemoved => "worktree_removed", Complete => "complete" }
}
string_enum! { /// Current observed holds. Uncertain commands and replies are never forced or resent.
    pub enum BridgeChildCleanupHold { AgentBusy => "agent_busy", WorkspaceBusy => "workspace_busy", PendingCommand => "pending_command", UncertainCommand => "uncertain_command", PendingReply => "pending_reply", UncertainReply => "uncertain_reply", OtherWorkspaceChildren => "other_workspace_children", InventoryLimit => "inventory_limit", AuthorityUnavailable => "authority_unavailable", ParentUnavailable => "parent_unavailable", RuntimeChanged => "runtime_changed", EvidenceUnavailable => "evidence_unavailable", EvidenceInconsistent => "evidence_inconsistent" }
}
string_enum! { /// Original allocation placement; only a retired reused-parent child admits cleanup.
    pub enum BridgeCleanupPlacement { ExistingAgent => "existing_agent", NewSandbox => "new_sandbox", ReuseParent => "reuse_parent" }
}
string_enum! { /// Current retained allocation lifecycle.
    pub enum BridgeCleanupAllocationState { Reserved => "reserved", Provisioning => "provisioning", Ready => "ready", NeedsConnection => "needs_connection", FailedRetryable => "failed_retryable", FailedTerminal => "failed_terminal", Retired => "retired" }
}

/// Explicit cleanup input. Preserve these revisions, action and key after any uncertain reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeChildCleanupRequest {
    /// Current Bridge revision observed before the explicit command.
    pub expected_bridge_revision: u64,
    /// Exact retired allocation revision. Cleanup does not increment this value.
    pub expected_allocation_revision: u64,
    /// Explicit child-removal consent.
    pub action: BridgeChildCleanupAction,
}
/// Immutable acceptance, separate from current progress and retained holds.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeChildCleanupReceipt {
    /// Stable cleanup job identity.
    pub id: String,
    /// Exact Bridge scope.
    pub bridge_id: String,
    /// Exact retired allocation.
    pub allocation_id: String,
    /// Original allocation generation, from one through 256.
    pub generation: u64,
    /// Exact child ancestry; never a replacement selected by the client.
    pub target: BridgeAgentTarget,
    /// Original accepted input.
    pub request: BridgeChildCleanupRequest,
    /// Actual Human or undelegated API actor that admitted cleanup.
    #[serde(deserialize_with = "manager")]
    pub requested_by: ResourceActor,
    /// Original acceptance in nonnegative Unix milliseconds.
    pub accepted_at_ms: u64,
}
/// Authorized progress for one exact child. Reads and accepted replays never admit another effect.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeChildCleanupStatus {
    /// Exact Bridge scope.
    pub bridge_id: String,
    /// Exact retained allocation.
    pub allocation_id: String,
    /// Original allocation generation.
    pub generation: u64,
    /// Retained allocation revision, separate from job progress.
    pub allocation_revision: u64,
    /// Exact Sandbox, Workspace, Worktree and Agent.
    pub target: BridgeAgentTarget,
    /// Null only before any explicit cleanup job was requested.
    #[serde(deserialize_with = "required_nullable")]
    pub receipt: Option<BridgeChildCleanupReceipt>,
    /// Latest proved progress, which can lag physical effects.
    pub progress: BridgeChildCleanupProgress,
    /// Bounded, ordered, unique reasons for the last retained hold.
    pub holds: Vec<BridgeChildCleanupHold>,
    /// Time of the retained observation; may predate acceptance when adopting old evidence.
    #[serde(deserialize_with = "required_nullable")]
    pub observed_at_ms: Option<u64>,
    /// True only when exact permanent removal evidence proves Complete.
    pub agent_capacity_released: bool,
    /// Always true for this child-only operation.
    pub workspace_retained: bool,
    /// Always true; conversation history is preserved.
    pub history_retained: bool,
}
/// Safe allocation inventory under current whole-parent management authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeCleanupAllocation {
    /// Stable allocation identity.
    pub id: String,
    /// Exact requested Bridge.
    pub bridge_id: String,
    /// Original generation, from one through 256.
    pub generation: u64,
    /// Current allocation revision.
    pub revision: u64,
    /// Exact child and retained parent identities.
    pub target: BridgeAgentTarget,
    /// Original allocation placement.
    pub placement: BridgeCleanupPlacement,
    /// Current allocation lifecycle.
    pub state: BridgeCleanupAllocationState,
}
/// Allocation-ID ordered inventory with an opaque continuation bound to the Bridge.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeCleanupAllocationPage {
    /// Authorized rows in strictly increasing allocation order.
    pub items: Vec<BridgeCleanupAllocation>,
    /// Pass unchanged for the same Bridge; null when no more rows were observed.
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<String>,
}
fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(deserializer)
}

fn manager<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<ResourceActor, D::Error> {
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Manager {
        Human {
            subject: String,
        },
        Api {
            subject: String,
            #[serde(deserialize_with = "required_nullable")]
            delegation_id: Option<String>,
        },
    }
    Ok(match Manager::deserialize(deserializer)? {
        Manager::Human { subject } => ResourceActor::Human { subject },
        Manager::Api {
            subject,
            delegation_id,
        } => ResourceActor::Api {
            subject,
            delegation_id,
        },
    })
}
