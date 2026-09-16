use layer2_core::resource_access::ResourceShareStore;
use layer2_core::sandbox::agent_io::AgentCommandWorkerStore;
use layer2_core::sandbox::store::{
    AgentLifecycleStore, AgentReadStore, AgentServiceStore, SandboxServiceStore,
    WorkspaceServiceStore,
};
use layer2_core::sandbox::AgentOperationStore;

/// Keep lifecycle and public conformance fixtures on the same real store contract.
pub(super) trait FixtureStore:
    AgentLifecycleStore
    + AgentReadStore
    + AgentServiceStore
    + AgentOperationStore
    + SandboxServiceStore
    + WorkspaceServiceStore
    + ResourceShareStore
    + AgentCommandWorkerStore
    + 'static
{
}

impl<T> FixtureStore for T where
    T: AgentLifecycleStore
        + AgentReadStore
        + AgentServiceStore
        + AgentOperationStore
        + SandboxServiceStore
        + WorkspaceServiceStore
        + ResourceShareStore
        + AgentCommandWorkerStore
        + 'static
{
}
