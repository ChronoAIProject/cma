//! Public-boundary foundations retained while the resource APIs are rebuilt.

pub mod a2a;
pub mod agent_page;
pub mod agent_v2;
pub mod agui;
pub mod auth;
mod conversation;
#[path = "rest/desktop_telemetry.rs"]
pub mod desktop_telemetry;
pub mod mcp;
mod model_budget_problem;
mod model_funding_problem;
pub mod pty;
mod resource_auth;
pub mod responses;
pub mod rest;
pub mod stream_telemetry;
pub mod tickets;

pub use auth::{AuthMethod, AuthenticatedCaller, AuthenticatedSubject, DelegatedCredential};
pub use conversation::AgentConversationState;
pub use resource_auth::AuthenticatedResourceCaller;
pub use tickets::{
    AttachmentClaim, AttachmentTickets, IssuedTicket, MemoryTicketStore, RedeemedTicket,
    StoredTicket, TicketId, TicketStore,
};

/// A public-interface operation that remains available during the rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    Discover,
    /// Author, publish, share, and retire owned profiles.
    ManageProfiles,
    /// Search and read published profiles.
    ReadProfileMarketplace,
    /// Copy a published profile revision into an owned private draft.
    ForkMarketplaceProfile,
    /// Create, read, stop, and delete owned Agents.
    ManageAgents,
    /// Send a new message to an Agent.
    SendAgentInput,
    /// Replay one Agent's durable events.
    StreamAgentEvents,
    /// Resolve an Agent approval request.
    ResolveAgentApproval,
    /// Interrupt an Agent's active turn.
    InterruptAgent,
    /// Add steering input to an Agent's active turn.
    SteerAgent,
    /// Read an Agent's durable token usage.
    ReadAgentUsage,
    /// Read the resources attributed to an Agent's Sandbox.
    ReadAgentResources,
    /// Inspect and control deployment-wide resources.
    Administer,
    /// Create, list, inspect, and retire owned Sandboxes.
    ManageSandboxes,
    /// Create, list, inspect, and remove owned Workspaces.
    ManageWorkspaces,
    /// Inspect Worktrees attached to owned Workspaces.
    ReadWorktrees,
    /// Replay an owned Sandbox event stream.
    ReadSandboxEvents,
}

/// Protocol profiles declare only operations they currently implement.
pub trait InterfaceProfile {
    fn interface_id(&self) -> &'static str;
    fn operations(&self) -> &'static [Operation];
}

#[cfg(test)]
mod observation_test_support;

#[cfg(test)]
mod model_funding_problem_tests;
