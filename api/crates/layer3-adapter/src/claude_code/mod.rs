//! Adapter driving the Claude Code kernel inside a provisioned sandbox.
//!
//! The second kernel behind the same contract, and the reason to trust the
//! contract at all: nothing in Layer 2 changed to accommodate it, and adding it
//! meant adding a directory rather than editing the trait it implements.
//!
//! What is here is this kernel and nothing else — what its bridge is called,
//! where it keeps its conversations, its private words for how much the agent
//! may do, and an honest account of what it cannot do. Bringing a sandbox up,
//! the link back, the credential that rides on it and the protocol client are
//! shared machinery in `hosted`, because none of that changes with the agent
//! inside.
//!
//! ## What this kernel cannot do, and why it says so
//!
//! **No terminal.** Deliberately out of scope: this kernel has a terminal
//! application, but bringing it up is a change of its own and a capability
//! claimed before it works is discovered by whoever tried to use it.
//!
//! **No snapshot, so no rotation.** This is the one that costs something, and
//! it is stated rather than worked around. The kernel files each conversation
//! under a directory named for the working directory it was started in, and
//! resumes one by identifier out of that directory. The supervisor carries a
//! conversation without reading it — deliberately, so a kernel changing its
//! format cannot break the pod hosting it — and lays it down under a name of
//! its own. A carried conversation therefore lands somewhere this kernel does
//! not look, and asking it to resume by identifier would find nothing, start a
//! fresh conversation and report success. Claiming the ability would turn a
//! rotation into silent loss of somebody's history; declining it means
//! rotation refuses and leaves the session running, which is what §5 asks for.
//!
//! **No usage, no steering.** Neither is reported or accepted by the pinned
//! bridge. Claimed, they would be operations a session accepts and silently
//! drops.

mod environment;
mod permission;

pub use environment::ClaudeCodeSandboxSpec;

use crate::hosted::{AcpConversationBinding, HostedKernel};
use crate::sandbox::SandboxClient;
use crate::ConversationUpdate;
use crate::{
    AdapterError, ConversationChannel, KernelAdapter, KernelCapabilities, ResourceSample,
    RuntimeHandle, RuntimeIdentity, RuntimeListing, RuntimeOptions, RuntimeReservationIdentity,
    RuntimeSpec,
};
use tokio::sync::mpsc;

/// Drives Claude Code inside a provider-managed sandbox.
pub struct ClaudeCodeAdapter {
    hosted: HostedKernel,
    spec: ClaudeCodeSandboxSpec,
}

impl ClaudeCodeAdapter {
    /// Build an adapter over a sandbox provider.
    pub fn new(client: SandboxClient, spec: ClaudeCodeSandboxSpec) -> Self {
        Self {
            hosted: HostedKernel::new(
                client,
                spec.deployment.shape.clone(),
                "claude_code",
                spec.deployment.skills_root.clone(),
                spec.deployment.agent_notes.clone(),
            ),
            spec,
        }
    }

    /// Bind Claude Code's permission vocabulary to shared hosted ACP.
    fn conversation_binding(&self, options: &RuntimeOptions) -> AcpConversationBinding {
        AcpConversationBinding::new(
            Some(permission::mode_for(options.permission).to_string()),
            options.resume_conversation.clone(),
            // This kernel refuses own-account sessions during provisioning.
            false,
        )
    }
}

#[async_trait::async_trait]
impl KernelAdapter for ClaudeCodeAdapter {
    fn kernel_name(&self) -> &'static str {
        "claude_code"
    }

    fn capabilities(&self) -> KernelCapabilities {
        // Every claim below was checked against the pinned bridge before it was
        // made. The absences are argued in this module's header; each one is a
        // request that fails by name at the moment of asking rather than an
        // operation a session accepts and quietly drops.
        KernelCapabilities {
            conversation: true,
            terminal: false,
            snapshot: false,
            steer: false,
            interrupt: true,
            usage: false,
            resources: true,
        }
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        // Refused permanently, not pending. The word "yet" stood here while
        // the codex adapter carried the same refusal, and that one has since
        // been lifted — so leaving it would read as a feature on its way,
        // which this is not.
        //
        // This kernel's vendor states that credentials from a personal
        // subscription may not be used from another product or service, and
        // enforces it on their side. A session doing that would be spending an
        // account in a way its owner had been invited to believe was fine
        // right up until the account was disabled. §9: declare the absence
        // rather than let somebody discover it.
        //
        // What this kernel can compliantly spend is a key or token issued for
        // programmatic use, which is a different setting and not this one.
        if spec.options.own_credential {
            return Err(AdapterError::Unsupported(
                "this kernel cannot run on a personal subscription: its vendor does not permit \
                 those credentials outside their own tools. An API key or gateway token is the \
                 supported way to spend your own account here"
                    .to_string(),
            ));
        }

        // Refused for the same reason, and it is the same defect: a caller
        // asked for something, was told nothing, and got a session without it.
        self.spec
            .deployment
            .refuse_undeliverable(&spec.environment)?;
        self.spec.deployment.refuse_missing_policy_verifier(spec)?;

        // Refused rather than started empty. This kernel has no terminal plane
        // here, and a sandbox brought up for a terminal session would be a pod
        // nobody can attach to — billed, running, and useless. The core checks
        // this too, from the capabilities above; this is the same refusal made
        // by the only component that could be wrong about it.
        if matches!(spec.options.mode, crate::RuntimeMode::Terminal) {
            return Err(AdapterError::Unsupported(
                "this kernel serves conversation sessions, not terminal sessions".to_string(),
            ));
        }

        // A carried conversation would land where this kernel does not look, so
        // resuming it is refused rather than attempted. Reaching here means
        // rotation asked anyway; failing is what keeps the session it came from
        // running instead of replacing it with an empty one.
        if spec.restore_from.is_some() {
            return Err(AdapterError::Unsupported(
                "this kernel cannot resume a conversation carried from another runtime".to_string(),
            ));
        }

        self.hosted
            .provision(spec, self.environment_for(spec))
            .await
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
