//! Adapter driving the Codex kernel inside a provisioned sandbox.
//!
//! What is left here after the second kernel arrived: what this kernel's
//! sandbox is shaped like, what its agent needs in its environment, what it can
//! actually do, and the words its bridge uses that no other bridge does.
//! Everything else — bringing a sandbox up, the link back, the credential that
//! rides on it, the protocol client — is shared machinery in `hosted`, because
//! none of it changes with the agent inside.

mod config;
mod environment;
mod permission;

pub use environment::CodexSandboxSpec;

use crate::hosted::{AcpConversationBinding, HostedKernel};
use crate::sandbox::SandboxClient;
use crate::ConversationUpdate;
use crate::{
    AdapterError, ConversationChannel, KernelAdapter, KernelCapabilities, ResourceSample,
    RuntimeHandle, RuntimeIdentity, RuntimeListing, RuntimeMode, RuntimeOptions,
    RuntimeReservationIdentity, RuntimeSpec, TerminalChannel, WorkspaceCheckpointPackage,
    WorkspaceCheckpointRequest,
};
use tokio::sync::mpsc;

/// Drives Codex inside a provider-managed sandbox.
pub struct CodexAdapter {
    kernel: &'static str,
    hosted: HostedKernel,
    spec: CodexSandboxSpec,
}

impl CodexAdapter {
    /// Build an adapter over a sandbox provider.
    pub fn new(client: SandboxClient, spec: CodexSandboxSpec) -> Self {
        Self {
            kernel: "codex",
            hosted: HostedKernel::new(
                client,
                spec.deployment.shape.clone(),
                "codex",
                spec.deployment.skills_root.clone(),
                spec.deployment.agent_notes.clone(),
            ),
            spec,
        }
    }

    /// Build a Codex adapter under a deployment-owned opaque kernel name.
    ///
    /// A distinct name lets one deployment route a closed session profile to
    /// a dedicated provider lane while preserving Codex's implementation and
    /// keeping the selection provider-neutral above this layer.
    pub fn named(kernel: &'static str, client: SandboxClient, spec: CodexSandboxSpec) -> Self {
        Self {
            kernel,
            hosted: HostedKernel::new_strict(
                client,
                spec.deployment.shape.clone(),
                kernel,
                spec.deployment.skills_root.clone(),
                spec.deployment.agent_notes.clone(),
            ),
            spec,
        }
    }

    /// Bind Codex-specific permission vocabulary to shared hosted ACP.
    fn conversation_binding(&self, options: &RuntimeOptions) -> AcpConversationBinding {
        AcpConversationBinding::new(
            Some(
                permission::mode_for(options.permission, options.restricted_service_gateway)
                    .to_string(),
            ),
            options.resume_conversation.clone(),
            options.own_credential,
            options.working_directory.clone(),
            options.conversation_slot.clone(),
        )
    }
}

#[async_trait::async_trait]
impl KernelAdapter for CodexAdapter {
    fn kernel_name(&self) -> &'static str {
        self.kernel
    }

    fn capabilities(&self) -> KernelCapabilities {
        // Every one of these was proven against the pinned kernel version
        // before it was claimed here, rather than assumed from its
        // documentation — see the spike recorded on issue #76.
        KernelCapabilities {
            conversation: true,
            terminal: true,
            // A rollout carried into a fresh home resumes by identifier, which
            // is what makes a session survive its runtime being replaced.
            snapshot: true,
            // The kernel accepts input mid-turn and continues the same turn
            // rather than needing it cancelled and restarted.
            steer: true,
            interrupt: true,
            // Reported per turn, with the context window alongside.
            usage: true,
            resources: true,
        }
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        // An own-account session is provisioned now rather than refused. The
        // refusal that stood here was right for as long as nothing signed the
        // agent in — a session accepted and then run on the delegated
        // credential would spend the platform's budget while its owner
        // believed otherwise. What changed is that the kernel can now sign
        // itself in inside the sandbox, and its configuration names no proxy,
        // so an own-account session either spends the owner's account or
        // cannot reach a model at all. Neither outcome is the quiet
        // substitution the refusal existed to prevent.

        // Refused for the same reason, and it is the same defect: a caller
        // asked for something, was told nothing, and got a session without it.
        self.spec
            .deployment
            .refuse_undeliverable(&spec.environment)?;
        self.spec.deployment.refuse_missing_policy_verifier(spec)?;

        self.hosted
            .provision(spec, self.environment_for(spec))
            .await
    }

    async fn snapshot(&self, handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        self.hosted.snapshot(handle).await
    }

    async fn snapshot_conversation(
        &self,
        handle: &RuntimeHandle,
        conversation_id: &str,
    ) -> Result<Option<Vec<u8>>, AdapterError> {
        self.hosted
            .snapshot_conversation(handle, conversation_id)
            .await
    }

    async fn prepare_workspace_source(
        &self,
        handle: &RuntimeHandle,
        request: &crate::WorkspaceSourceRequest,
    ) -> Result<crate::WorkspaceSourceReceipt, AdapterError> {
        self.hosted.prepare_workspace_source(handle, request).await
    }

    async fn add_workspace_tree(
        &self,
        handle: &RuntimeHandle,
        request: &crate::WorkspaceTreeRequest,
    ) -> Result<crate::WorkspaceTreeReceipt, AdapterError> {
        self.hosted.add_workspace_tree(handle, request).await
    }

    async fn remove_workspace_tree(
        &self,
        handle: &RuntimeHandle,
        name: &str,
    ) -> Result<(), AdapterError> {
        self.hosted.remove_workspace_tree(handle, name).await
    }

    async fn run_workspace_setup(
        &self,
        handle: &RuntimeHandle,
        commands: &[String],
    ) -> Result<crate::WorkspaceSetupReceipt, AdapterError> {
        self.hosted.run_workspace_setup(handle, commands).await
    }

    async fn checkpoint_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceCheckpointRequest,
    ) -> Result<WorkspaceCheckpointPackage, AdapterError> {
        self.hosted.checkpoint_workspace(handle, request).await
    }

    async fn resume_workspace_writers(
        &self,
        handle: &RuntimeHandle,
        resume_token: &str,
    ) -> Result<(), AdapterError> {
        self.hosted
            .resume_workspace_writers(handle, resume_token)
            .await
    }

    async fn attach_terminal(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        self.hosted.attach_terminal(handle).await
    }

    async fn open_conversation(
        &self,
        handle: &RuntimeHandle,
        options: &RuntimeOptions,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel, AdapterError> {
        self.hosted
            .open_acp_conversation(handle, self.conversation_binding(options), updates)
            .await
    }

    async fn refresh_credential(
        &self,
        handle: &RuntimeHandle,
        credential: &crate::RuntimeCredential,
    ) -> Result<(), AdapterError> {
        self.hosted.refresh_credential(handle, credential).await
    }

    async fn install_runtime_policy(
        &self,
        handle: &RuntimeHandle,
        envelope: &str,
    ) -> Result<crate::RuntimePolicyReceipt, AdapterError> {
        self.hosted.install_runtime_policy(handle, envelope).await
    }

    async fn install_sponsored_model_authority(
        &self,
        handle: &RuntimeHandle,
        authority: &crate::SponsoredModelRuntimeAuthority,
    ) -> Result<crate::SponsoredModelAuthorityReceipt, AdapterError> {
        self.hosted
            .install_sponsored_model_authority(handle, authority)
            .await
    }

    async fn observe_runtime_policy(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<crate::RuntimePolicyObservation, AdapterError> {
        self.hosted.observe_runtime_policy(handle).await
    }

    async fn probe_runtime_gateway(
        &self,
        handle: &RuntimeHandle,
        request: &crate::RuntimeGatewayProbeRequest,
    ) -> Result<crate::RuntimeGatewayProbeReceipt, AdapterError> {
        self.hosted.probe_runtime_gateway(handle, request).await
    }

    async fn switch_plane(
        &self,
        handle: &RuntimeHandle,
        mode: RuntimeMode,
    ) -> Result<(), AdapterError> {
        // The command for each front-end, in this kernel's own vocabulary.
        // The supervisor refuses a command it was not told at boot it may run.
        let command = match mode {
            RuntimeMode::Terminal => "codex".to_string(),
            RuntimeMode::Conversation => self.spec.deployment.bridge_command.clone(),
        };
        self.hosted.switch_plane(handle, &command).await
    }

    async fn list_runtimes(&self) -> Result<Vec<RuntimeListing>, AdapterError> {
        self.hosted.list_runtimes().await
    }

    async fn watch_resources(
        &self,
        handle: &RuntimeHandle,
        readings: mpsc::Sender<ResourceSample>,
    ) -> Result<(), AdapterError> {
        self.hosted.watch_resources(handle, readings).await
    }

    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        self.hosted.terminate(handle).await
    }

    async fn terminate_if_identity(&self, identity: &RuntimeIdentity) -> Result<(), AdapterError> {
        self.hosted.terminate_if_identity(identity).await
    }

    async fn discover_runtime_candidates(
        &self,
        reservation: &RuntimeReservationIdentity,
    ) -> Result<Vec<RuntimeHandle>, AdapterError> {
        self.hosted.discover_runtime_candidates(reservation).await
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
