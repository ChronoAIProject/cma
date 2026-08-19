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

use crate::hosted::{acp, HostedKernel};
use crate::sandbox::SandboxClient;
use crate::{
    AdapterError, ConversationChannel, KernelAdapter, KernelCapabilities, RuntimeHandle,
    RuntimeListing, RuntimeMode, RuntimeOptions, RuntimeSpec, TerminalChannel,
};

/// Drives Codex inside a provider-managed sandbox.
pub struct CodexAdapter {
    hosted: HostedKernel,
    spec: CodexSandboxSpec,
}

impl CodexAdapter {
    /// Build an adapter over a sandbox provider.
    pub fn new(client: SandboxClient, spec: CodexSandboxSpec) -> Self {
        Self {
            hosted: HostedKernel::new(client, spec.deployment.shape.clone(), "codex"),
            spec,
        }
    }
}

#[async_trait::async_trait]
impl KernelAdapter for CodexAdapter {
    fn kernel_name(&self) -> &'static str {
        "codex"
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
        }
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        // Refused rather than ignored. Nothing in the runtime authenticates the
        // agent against the owner's own vendor account, so a session that asked
        // for it and was accepted would run on the delegated credential — the
        // owner's bill and the owner's entitlements — while believing otherwise.
        // A session that cannot be what was asked for must not start at all.
        if spec.options.own_credential {
            return Err(AdapterError::Unsupported(
                "this kernel does not support the owner's own credential yet".to_string(),
            ));
        }

        // Refused for the same reason, and it is the same defect: a caller
        // asked for something, was told nothing, and got a session without it.
        self.spec
            .deployment
            .refuse_undeliverable(&spec.environment)?;

        self.hosted
            .provision(spec, self.environment_for(spec))
            .await
    }

    async fn snapshot(&self, handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        self.hosted.snapshot(handle).await
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
    ) -> Result<ConversationChannel, AdapterError> {
        let (stream, url) = self.hosted.connect(handle, Some("conv")).await?;
        tracing::info!(sandbox = %handle.0, %url, "opening the sandbox conversation");
        let (conversation, control) = acp::Bridge::open(
            stream,
            self.hosted.workspace().to_string(),
            Some(permission::mode_for(options.permission).to_string()),
            options.resume_conversation.clone(),
            Some((self.hosted.switch_waiters(), handle.0.clone())),
        )
        .await?;
        // Recorded here so a credential can reach a session that never attaches
        // a terminal — which is every conversation session, and every session
        // at all on a kernel that serves only conversations.
        self.hosted.remember_link(&handle.0, control).await;
        Ok(conversation)
    }

    async fn refresh_credential(
        &self,
        handle: &RuntimeHandle,
        credential: &str,
    ) -> Result<(), AdapterError> {
        self.hosted.refresh_credential(handle, credential).await
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

    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        self.hosted.terminate(handle).await
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
