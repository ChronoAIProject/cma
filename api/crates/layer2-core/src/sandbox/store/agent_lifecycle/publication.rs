/// Compare-and-set proof for the parents observed while rebuilding a publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentWorkPublicationParents {
    pub sandbox_revision: u64,
    pub workspace_revision: u64,
    pub worktree_revision: u64,
    pub lineage: AgentWorkPublicationLineage,
}

/// A preflight rejection carries the exact versions that demonstrated invalid lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentWorkPublicationLineage {
    Authorized,
    Rejected { workspace_foreign: bool },
}

/// Publication disposition before authorizing a fresh mutation body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentWorkPublication {
    Published,
    KnownAborted,
}
