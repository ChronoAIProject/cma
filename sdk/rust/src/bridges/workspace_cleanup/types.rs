use super::super::BridgeAgentTarget;
use crate::resource_access::ResourceActor;
use serde::{Deserialize, Deserializer, Serialize};

string_enum! { /// Explicit effect requested for one exact allocation-owned Workspace.
    pub enum BridgeWorkspaceCleanupAction { RemoveOwnedWorkspace => "remove_owned_workspace" }
}
string_enum! { /// Latest proved progress; acceptance alone does not confirm removal.
    pub enum BridgeWorkspaceCleanupProgress { NotRequested => "not_requested", Requested => "requested", AdmissionClosed => "admission_closed", RemovalStarted => "removal_started", RuntimeRemoved => "runtime_removed", Complete => "complete" }
}
string_enum! { /// Current observed eligibility and progress holds; no failed observation grants effects.
    pub enum BridgeWorkspaceCleanupHold { AllocationNotRetired => "allocation_not_retired", WorkspaceNotOwned => "workspace_not_owned", ChildCleanupIncomplete => "child_cleanup_incomplete", WorkspaceBusy => "workspace_busy", OtherWorkspaceChildren => "other_workspace_children", InventoryLimit => "inventory_limit", AuthorityUnavailable => "authority_unavailable", ParentUnavailable => "parent_unavailable", RuntimeUnsupported => "runtime_unsupported", RuntimeChanged => "runtime_changed", EvidenceUnavailable => "evidence_unavailable", EvidenceInconsistent => "evidence_inconsistent" }
}
/// Explicit cleanup input. Preserve these revisions, action and key after any uncertain reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeWorkspaceCleanupRequest {
    /// Current Bridge revision observed before the explicit command.
    pub expected_bridge_revision: u64,
    /// Exact retired allocation revision. Cleanup does not increment this value.
    pub expected_allocation_revision: u64,
    /// Current original Workspace revision observed before consent.
    pub expected_workspace_revision: u64,
    /// Exact completed child receipt reviewed before the separate command.
    pub child_cleanup_receipt_id: String,
    /// Explicit consent to remove the originally owned Workspace.
    pub action: BridgeWorkspaceCleanupAction,
}
/// Immutable acceptance, separate from current progress and retained holds.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeWorkspaceCleanupReceipt {
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
    pub request: BridgeWorkspaceCleanupRequest,
    /// Actual Human or undelegated API actor that admitted cleanup.
    #[serde(deserialize_with = "manager")]
    pub requested_by: ResourceActor,
    /// Original acceptance in nonnegative Unix milliseconds.
    pub accepted_at_ms: u64,
}
/// Authorized progress for one exact owned Workspace. Reads and accepted replays never admit another effect.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeWorkspaceCleanupStatus {
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
    /// Current exact Workspace revision; required for eligibility and any accepted job.
    #[serde(deserialize_with = "required_nullable")]
    pub workspace_revision: Option<u64>,
    /// Original child receipt, retained even after separate Workspace removal.
    #[serde(deserialize_with = "required_nullable")]
    pub child_cleanup_receipt_id: Option<String>,
    /// Current eligibility for new consent; acceptance or elapsed time does not imply eligibility.
    pub eligible: bool,
    /// Current demonstrated runtime support; historical receipt recovery does not require true.
    pub runtime_supported: bool,
    /// Null only before any explicit Workspace cleanup job was requested.
    #[serde(deserialize_with = "required_nullable")]
    pub receipt: Option<BridgeWorkspaceCleanupReceipt>,
    /// Latest proved progress, which can lag physical effects.
    pub progress: BridgeWorkspaceCleanupProgress,
    /// Bounded, ordered, unique reasons for the last retained hold.
    pub holds: Vec<BridgeWorkspaceCleanupHold>,
    /// Time of the retained observation; may predate acceptance when adopting old evidence.
    #[serde(deserialize_with = "required_nullable")]
    pub observed_at_ms: Option<u64>,
    /// True only when exact permanent removal evidence proves Complete.
    pub workspace_capacity_released: bool,
    /// Always true; this operation preserves the Sandbox.
    pub parent_retained: bool,
    /// Always true; conversation history is preserved.
    pub history_retained: bool,
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
