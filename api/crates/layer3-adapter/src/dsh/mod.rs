//! Adapter driving the DeepSeek Harness inside a provisioned sandbox.
//!
//! The third kernel, and the one that tested whether the contract was a
//! contract or a description of the first two. It speaks a different protocol
//! — not the Agent Client Protocol at all — and it can do less. Neither fact
//! reached Layer 2: what is different lives in this directory, and what it
//! cannot do is declared rather than discovered.
//!
//! ## What is different, and stays here
//!
//! The harness serves a JSON-RPC wire of its own with three methods —
//! `initialize`, `session/prompt`, `shutdown` — and a stream of session-log
//! events. Turning that stream into the contract's vocabulary is this
//! directory's work, in `translate`, and nothing above sees the difference.
//!
//! ## What it cannot do, and says so
//!
//! **No terminal.** The harness ships none: its bundles are a headless app, a
//! web host and a launcher, and there is no screen to attach to.
//!
//! **No interrupt.** Its wire has no cancel, and the harness's own
//! documentation says a client abandons a turn by closing the process. Closing
//! the process ends the session, which is not what interrupting means. Proven
//! rather than read: a signal to a live turn was tried, and the turn ran to
//! completion.
//!
//! **No snapshot, so no rotation.** The session store carries fine — it is
//! opaque bytes in a keyed tree — but nothing can re-serve it: the wire's only
//! session call creates, and the persistence layer refuses a log it did not
//! write, loudly. Claiming the ability would turn a rotation into a session
//! that ends its first turn with a collision error.
//!
//! **No steering.** A prompt sent mid-turn is queued for the next one.
//!
//! **No guarded mode.** The harness has an approval policy and will stop to
//! ask — and its headless wire has no way to relay the question. A guarded
//! session would block on something nobody can see, which is worse than being
//! told at creation that this kernel cannot be guarded.

mod environment;
mod translate;

pub use environment::DshSandboxSpec;

use crate::hosted::HostedKernel;
use crate::sandbox::SandboxClient;
use crate::ConversationUpdate;
use crate::{
    AdapterError, ConversationChannel, KernelAdapter, KernelCapabilities, ResourceSample,
    RuntimeHandle, RuntimeIdentity, RuntimeListing, RuntimeOptions, RuntimePermission,
    RuntimeReservationIdentity, RuntimeSpec,
};
use tokio::sync::mpsc;

/// Drives the DeepSeek Harness inside a provider-managed sandbox.
pub struct DshAdapter {
    hosted: HostedKernel,
    spec: DshSandboxSpec,
}

impl DshAdapter {
    /// Build an adapter over a sandbox provider.
    pub fn new(client: SandboxClient, spec: DshSandboxSpec) -> Self {
        Self {
            hosted: HostedKernel::new(
                client,
                spec.deployment.shape.clone(),
                "dsh",
                spec.deployment.skills_root.clone(),
                spec.deployment.agent_notes.clone(),
            ),
            spec,
        }
    }
}

#[async_trait::async_trait]
impl KernelAdapter for DshAdapter {
    fn kernel_name(&self) -> &'static str {
        "dsh"
    }

    fn capabilities(&self) -> KernelCapabilities {
        // Each of these was checked against the pinned harness, driving real
        // turns through the profile this deployment bakes. The absences are
        // argued in this module's header.
        KernelCapabilities {
            conversation: true,
            terminal: false,
            snapshot: false,
            steer: false,
            interrupt: false,
            usage: true,
            resources: true,
        }
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        // Refused rather than ignored. Nothing in the runtime authenticates the
        // agent against the owner's own vendor account, so a session that asked
        // for it and was accepted would run on the delegated credential — the
        // owner's bill and the owner's entitlements — while believing otherwise.
        if spec.options.own_credential {
            return Err(AdapterError::Unsupported(
                "this kernel does not support the owner's own credential yet".to_string(),
            ));
        }

        self.spec
            .deployment
            .refuse_undeliverable(&spec.environment)?;

        // Refused rather than started empty: this kernel has no terminal plane,
        // so a sandbox brought up for a terminal session would be a pod nobody
        // can attach to — billed, running, and useless.
        if matches!(spec.options.mode, crate::RuntimeMode::Terminal) {
            return Err(AdapterError::Unsupported(
                "this kernel serves conversation sessions, not terminal sessions".to_string(),
            ));
        }

        // The refusal that matters most here. This agent stops and asks when
        // its policy says to, and the wire it is driven over has no way to
        // carry the question — so a guarded session would block on something
        // nobody can see or answer. Told at creation, its owner can choose a
        // kernel that can be guarded; started anyway, they find out by waiting.
        if matches!(spec.options.permission, RuntimePermission::Guarded) {
            return Err(AdapterError::Unsupported(
                "this kernel cannot ask its owner for permission, so it cannot run a guarded session"
                    .to_string(),
            ));
        }

        // A carried conversation cannot be re-served, so resuming one is
        // refused rather than attempted. Reaching here means rotation asked
        // anyway; failing is what keeps the session it came from running.
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
        let (stream, url) = self.hosted.connect(handle, Some("conv")).await?;
        tracing::info!(sandbox = %handle.0, %url, "opening the sandbox conversation");
        // The permission mode is not sent: this harness reads it from its
        // environment at boot, and the one mode it could be asked to change to
        // is the one this kernel refuses to provision at all.
        let (conversation, control) = translate::Harness::open(
            stream,
            self.hosted.workspace().to_string(),
            &self.spec,
            options.resume_conversation.clone(),
            self.hosted.link_lifecycle(&handle.0),
            updates,
        )
        .await?;
        // The harness registered this generation before starting its reader.
        drop(control);
        Ok(conversation)
    }

    async fn refresh_credential(
        &self,
        handle: &RuntimeHandle,
        credential: &str,
    ) -> Result<(), AdapterError> {
        self.hosted.refresh_credential(handle, credential).await
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
