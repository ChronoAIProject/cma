//! Codex-specific configuration, home layout, and bridge launch.

mod config;
mod connections;
mod construction;
pub(crate) mod deliverable_output;
pub use construction::{CodexAdapter, CodexSandboxSpec};
mod filesystem;
mod home;
mod hosted_runtime;
mod recovery;
use hosted_runtime::CodexRuntime;

use std::sync::Arc;

use tokio::sync::mpsc;

pub use config::{render_config, CodexConfig};
pub use connections::{CODEX_SUBSCRIPTION_CONNECTION, CONNECTION_SERVICES, PUBLIC_CODEX_SERVICE};
pub(crate) use home::RESERVED_ENVIRONMENT_NAMES;
pub use home::{
    checkpoint_exclusions, render_agent_home, render_launch, AGENT_HOME_ROOT, BRIDGE_PROGRAM,
    SHARED_AUTH_PATH,
};

use crate::hosted::{
    HostedAdapter, HostedAgentOpenSettings, HostedAgentRuntime, HostedBackend, HostedDeployment,
    InMemoryHostedBackend,
};
use crate::{
    AdapterCapabilities, AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest,
    AgentHomeRequest, AgentLaunch, AgentOpenRequest, AgentSlot, CheckpointHoldReceipt,
    CheckpointHoldRequest, CheckpointReceipt, CheckpointRequest, CheckpointWriterResumeReceipt,
    CheckpointWriterResumeRequest, ConversationChannel, ConversationSnapshot, ConversationUpdate,
    CredentialFrame, InstallReceipt, InstallRequest, ResourceReading, RestoreBootstrapReceipt,
    RestoreBootstrapRequest, RestoreRequest, RuntimeAdoptionRequest, RuntimeHandle,
    RuntimeObservation, RuntimeProvisionRequest, SandboxAdapter, SandboxSpec, SetupReceipt,
    WorkspaceReceipt, WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest,
    WorktreeReceipt, WorktreeRef, WorktreeRequest,
};

mod adapter;

#[cfg(test)]
#[path = "golden_tests.rs"]
mod tests;

#[cfg(test)]
mod attachments_tests;

#[cfg(test)]
mod termination_tests;
