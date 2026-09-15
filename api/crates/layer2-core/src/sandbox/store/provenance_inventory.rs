use crate::resource_access::ResourceOwner;
use crate::sandbox::AgentListQuery;

use super::{AgentProvenanceSnapshot, PageRequest, SandboxStoreError};

/// Internal selection scope, never caller-supplied authority. Organization scope
/// includes explicit ownership and active grants, with overlap deduplicated.
#[async_trait::async_trait]
pub trait AgentProvenanceInventoryStore: Send + Sync {
    /// Stable Agent-ID order, at most limit + 1 snapshots including lookahead.
    /// Select by scope and ordinary filters before bounding the scan. Capture
    /// ancestry, policy and creation evidence under one storage snapshot. The
    /// caller must authorize every candidate, including excluded/lookahead rows.
    async fn scan_agent_provenance_inventory(
        &self,
        scope: &ResourceOwner,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<AgentProvenanceSnapshot>, SandboxStoreError>;
}
