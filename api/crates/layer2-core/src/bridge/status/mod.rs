//! Read-only partition observations. Historical source/response correlation is
//! never a grant to invoke, reset, detach, retry or remove a retained target.
mod access;
mod service;
mod usage;
mod validation;
mod views;

use super::{allocation::AllocationRecord, ingress::StoredInbox, BridgeManagementLease};
use crate::{resource_access::ResourceTarget, sandbox::agent_io::*};
pub use service::BridgePartitionService;
pub use usage::*;
pub use views::*;

pub const MAX_PARTITION_PAGE: u32 = 25;
pub const MAX_PARTITION_PENDING: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum PartitionStatusError {
    #[error(transparent)]
    Bridge(#[from] super::BridgeError),
    #[error("invalid partition status query")]
    Invalid,
    #[error("partition not found")]
    NotFound,
    #[error("partition or resource authorization changed")]
    Conflict,
    #[error("partition status unavailable")]
    Unavailable,
    #[error("partition evidence is inconsistent")]
    Corrupt,
}

/// Internal snapshot only. No aggregate or source is serialized directly.
#[derive(Clone)]
pub struct PartitionEvidence {
    pub allocation: AllocationRecord,
    pub current: bool,
    pub present: Vec<PartitionResource>,
    pub first_source: Option<StoredInbox>,
    pub observed_at_ms: i64,
}

#[derive(Clone)]
pub struct PartitionCommandEvidence {
    pub command: AdmittedAgentCommand,
    pub sidecar: ChannelCommandSidecar,
    /// None denotes an old command whose accepted source was not retained.
    pub source: Option<StoredInbox>,
}

#[derive(Clone)]
pub struct PartitionActivityEvidence {
    pub pending: Vec<PartitionCommandEvidence>,
    pub latest: Option<PartitionCommandEvidence>,
    pub observed_at_ms: i64,
}

impl PartitionResource {
    pub fn target(self, ids: &super::BridgeAgentTarget) -> ResourceTarget {
        match self {
            Self::Sandbox => ResourceTarget::Sandbox {
                sandbox_id: ids.sandbox_id.clone(),
            },
            Self::Workspace => ResourceTarget::Workspace {
                workspace_id: ids.workspace_id.clone(),
            },
            Self::Worktree => ResourceTarget::Worktree {
                worktree_id: ids.worktree_id.clone(),
            },
            Self::Agent => ResourceTarget::Agent {
                agent_id: ids.agent_id.clone(),
            },
        }
    }
}

#[async_trait::async_trait]
pub trait BridgePartitionStore: Send + Sync {
    /// Complete, source-free inventory up to the fixed observation bound. More
    /// rows return None, never partial counts masquerading as global usage.
    async fn usage(
        &self,
        _manager: &BridgeManagementLease,
    ) -> Result<Option<Vec<PartitionEvidence>>, PartitionStatusError> {
        Err(PartitionStatusError::Unavailable)
    }
    /// Exact Bridge/owner/config snapshots, ascending allocation ID, limit + 1.
    async fn list(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<PartitionEvidence>, PartitionStatusError>;
    async fn detail(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<PartitionEvidence>, PartitionStatusError>;
    /// Only exact Channel sidecars; at most 65 pending and one latest command.
    /// Implementations must bound indexed scans before decoding retained input.
    async fn activity(
        &self,
        manager: &BridgeManagementLease,
        allocation: &AllocationRecord,
    ) -> Result<PartitionActivityEvidence, PartitionStatusError>;
}
