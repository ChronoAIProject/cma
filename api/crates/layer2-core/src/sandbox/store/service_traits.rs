use super::*;

pub trait SandboxServiceStore:
    SandboxStore
    + SandboxEventStore
    + SandboxAdmissionStore
    + SandboxTransactionStore
    + SandboxLifecycleCommandStore
    + SandboxLaunchCommandStore
{
}

impl<T> SandboxServiceStore for T where
    T: SandboxStore
        + SandboxEventStore
        + SandboxAdmissionStore
        + SandboxTransactionStore
        + SandboxLifecycleCommandStore
        + SandboxLaunchCommandStore
{
}

pub trait WorkspaceServiceStore:
    SandboxStore + WorkspaceStore + WorkspaceAdmissionStore + SandboxEventStore
{
}

impl<T> WorkspaceServiceStore for T where
    T: SandboxStore + WorkspaceStore + WorkspaceAdmissionStore + SandboxEventStore
{
}

pub trait WorkspaceLifecycleStore:
    SandboxStore
    + WorkspaceStore
    + WorkspaceWorkerStore
    + SandboxTransactionStore
    + AgentStore
    + AgentControlStore
{
}

pub trait AgentServiceStore:
    SandboxStore
    + WorkspaceStore
    + WorktreeStore
    + AgentStore
    + AgentAdmissionStore
    + AgentConversationStore
    + AgentControlStore
    + AgentLogStore
{
}

pub trait AgentLifecycleStore:
    SandboxStore + WorkspaceStore + WorktreeStore + AgentStore + AgentLogStore + AgentWorkerStore
{
}

pub trait AgentReadStore:
    SandboxStore + WorkspaceStore + WorktreeStore + AgentStore + AgentLogStore + AgentReadQueryStore
{
}

pub trait AgentConversationUpdateStore: AgentStore + AgentConversationStore {}

impl<T> AgentConversationUpdateStore for T where T: AgentStore + AgentConversationStore {}

impl<T> AgentLifecycleStore for T where
    T: SandboxStore
        + WorkspaceStore
        + WorktreeStore
        + AgentStore
        + AgentLogStore
        + AgentWorkerStore
{
}

impl<T> AgentReadStore for T where
    T: SandboxStore
        + WorkspaceStore
        + WorktreeStore
        + AgentStore
        + AgentLogStore
        + AgentReadQueryStore
{
}

impl<T> AgentServiceStore for T where
    T: SandboxStore
        + WorkspaceStore
        + WorktreeStore
        + AgentStore
        + AgentAdmissionStore
        + AgentConversationStore
        + AgentControlStore
        + AgentLogStore
{
}

impl<T> WorkspaceLifecycleStore for T where
    T: SandboxStore
        + WorkspaceStore
        + WorkspaceWorkerStore
        + SandboxTransactionStore
        + AgentStore
        + AgentControlStore
{
}
