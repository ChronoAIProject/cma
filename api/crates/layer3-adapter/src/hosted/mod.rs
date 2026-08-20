//! A kernel hosted in a provider-managed sandbox.
//!
//! Shared machinery, like `sandbox` and `terminal`: everything about running a
//! kernel in a pod that does not depend on which kernel it is. Bringing a
//! sandbox up and waiting for it, handing a conversation to the one that
//! replaces it, capturing one from the one being replaced, the link back and
//! the credential that rides on it — none of that changes with the agent
//! inside.
//!
//! It exists because the second kernel arrived. The first one's directory held
//! all of this, which looked right until there was something to compare it
//! against: a kernel's directory should hold what is true of that kernel and
//! nothing else, and none of this was.
//!
//! What a kernel's own directory keeps is small and real — what its sandbox is
//! shaped like, what environment its agent needs, what it can actually do, and
//! the words its bridge uses that no other bridge does.

pub(crate) mod acp;
mod deployment;
mod handshake;
mod inbound;
mod link;
pub(super) mod turn;

use std::collections::BTreeMap;

use tokio::sync::{mpsc, oneshot};

use crate::sandbox::{ImageSpec, Sandbox, SandboxClient, SandboxError, SandboxRequest};
use crate::{AdapterError, RuntimeHandle, RuntimeListing, RuntimeSpec, TerminalInput};

pub use deployment::{Deployment, PLACEHOLDER_CREDENTIAL};

/// Port the supervisor listens on inside the sandbox.
pub(crate) const SUPERVISOR_PORT: u16 = 9090;

/// Metadata key naming the control plane that created a sandbox.
///
/// Written at provisioning and read by reconciliation, so a provider shared
/// between control planes can be told apart. Absent on sandboxes created before
/// this was recorded, which reconciliation treats as its own business to decide.
const CREATOR_METADATA_KEY: &str = "chrono-bot/control-plane";

/// Metadata key correlating a sandbox back to its session.
///
/// Written at provisioning and read by reconciliation, so the two must agree;
/// naming it once is what keeps them in step.
const SESSION_METADATA_KEY: &str = "chrono-bot/session";

/// Metadata key naming which kernel a sandbox is running.
const KERNEL_METADATA_KEY: &str = "chrono-bot/kernel";

/// Where the supervisor serves and accepts the agent's conversation.
const CONVERSATION_PATH: &str = "/runtime/conversation";

/// How long to wait for a freshly created sandbox to start serving.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How often to re-check a sandbox that is not yet running.
const READY_POLL: std::time::Duration = std::time::Duration::from_secs(2);

impl From<SandboxError> for AdapterError {
    fn from(e: SandboxError) -> Self {
        match e {
            // A runtime answering "not implemented" is stating an absence, not
            // reporting a failure. Read as a rejection it would look like
            // something going wrong every time rotation asked a runtime that
            // was never able to answer.
            SandboxError::Rejected { status: 501, .. } => AdapterError::Unsupported(
                "this runtime declares no support for what was asked".to_string(),
            ),
            SandboxError::Rejected { status, detail } => {
                AdapterError::Rejected(format!("{status}: {detail}"))
            }
            other => AdapterError::Unavailable(other.to_string()),
        }
    }
}

/// How sandboxes are shaped, whichever kernel is in them.
#[derive(Debug, Clone)]
pub struct SandboxShape {
    /// Image carrying the kernel and the supervisor.
    pub image: String,
    /// Entrypoint that starts the supervisor.
    pub entrypoint: Vec<String>,
    /// Hard resource caps for each sandbox.
    pub resource_limits: BTreeMap<String, String>,
    /// Seconds before the provider reclaims the sandbox, or `None` for never.
    ///
    /// `None` is the default and is deliberate. A session is a person's working
    /// terminal: expiring it on a timer ends live work mid-turn, and no timer
    /// can tell an idle session from one whose owner is reading. Teardown is an
    /// explicit act by the owner, and the reconciliation sweep is what reclaims
    /// sandboxes whose session is genuinely gone.
    pub timeout_seconds: Option<u64>,
    /// Directory the agent works in, inside its sandbox.
    pub workspace: String,
}

impl Default for SandboxShape {
    fn default() -> Self {
        Self {
            image: "chrono-bot-runtime:dev".into(),
            entrypoint: vec!["/usr/local/bin/layer4-runtime".into()],
            resource_limits: BTreeMap::from([
                ("cpu".to_string(), "1".to_string()),
                ("memory".to_string(), "1Gi".to_string()),
            ]),
            timeout_seconds: None,
            workspace: "/home/chrono".into(),
        }
    }
}

/// A kernel running in a provider-managed sandbox.
/// Who is waiting to hear whether a plane switch happened, by runtime handle.
///
/// Shared rather than owned: a link's inbound task is what hears the answer,
/// and it outlives the call that asked the question.
#[derive(Clone, Default)]
pub(crate) struct SwitchWaiters(
    std::sync::Arc<tokio::sync::Mutex<BTreeMap<String, oneshot::Sender<bool>>>>,
);

impl SwitchWaiters {
    /// Register interest in the next answer from this runtime.
    pub(crate) async fn expect(&self, handle: &str) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        // A second switch replaces the first waiter; the earlier caller sees
        // its sender dropped and rotates, which is the safe reading.
        self.0.lock().await.insert(handle.to_string(), tx);
        rx
    }

    /// Hand an answer to whoever asked. An answer nobody awaits is dropped:
    /// it belongs to a caller that has already given up and rotated.
    pub(crate) async fn settle(&self, handle: &str, accepted: bool) {
        if let Some(waiter) = self.0.lock().await.remove(handle) {
            let _ = waiter.send(accepted);
        }
    }

    /// Forget a waiter whose link has gone.
    pub(crate) async fn forget(&self, handle: &str) {
        self.0.lock().await.remove(handle);
    }
}

pub struct HostedKernel {
    pub(crate) client: SandboxClient,
    shape: SandboxShape,
    /// Which kernel this is, recorded on the sandboxes it creates.
    kernel: &'static str,
    /// The open link to each runtime, by handle.
    ///
    /// Held because a credential has to reach a runtime that is already
    /// running, and the supervisor accepts one only over a link that is
    /// already open. Opening a second attachment to deliver it is not an
    /// option: attaching declares a geometry, and a connection made without
    /// one would resize the pty under whoever is looking at it.
    links: tokio::sync::Mutex<BTreeMap<String, mpsc::Sender<TerminalInput>>>,
    /// Who is waiting to hear whether a plane switch was performed.
    ///
    /// A switch is only recorded once the runtime says it happened (#289):
    /// the frame reaching the socket says nothing about the program actually
    /// being swapped, and a supervisor too old to know the frame ignores it
    /// in silence. One waiter per runtime, because a session has one plane
    /// and cannot be moved twice at once.
    switch_waiters: SwitchWaiters,
}

impl HostedKernel {
    /// Host `kernel` in sandboxes of this shape, from this provider.
    pub fn new(client: SandboxClient, shape: SandboxShape, kernel: &'static str) -> Self {
        Self {
            client,
            shape,
            kernel,
            links: tokio::sync::Mutex::new(BTreeMap::new()),
            switch_waiters: SwitchWaiters::default(),
        }
    }

    /// Where the agent works, inside its sandbox.
    pub fn workspace(&self) -> &str {
        &self.shape.workspace
    }

    /// Bring a sandbox up with this environment, and wait until it serves.
    pub async fn provision(
        &self,
        spec: &RuntimeSpec,
        env: BTreeMap<String, String>,
    ) -> Result<RuntimeHandle, AdapterError> {
        let request = SandboxRequest {
            image: ImageSpec {
                uri: self.shape.image.clone(),
            },
            entrypoint: self.shape.entrypoint.clone(),
            env,
            resource_limits: self.shape.resource_limits.clone(),
            timeout: self.shape.timeout_seconds,
            // Correlates a sandbox back to the session that owns it, so an
            // orphan can be found and reclaimed.
            metadata: {
                let mut m = BTreeMap::from([
                    (SESSION_METADATA_KEY.to_string(), spec.session_id.clone()),
                    (KERNEL_METADATA_KEY.to_string(), self.kernel.to_string()),
                ]);
                if let Some(creator) = spec.created_by.as_ref() {
                    m.insert(CREATOR_METADATA_KEY.to_string(), creator.clone());
                }
                m
            },
        };

        let sandbox = self.client.create(&request).await?;
        tracing::info!(
            session_id = %spec.session_id,
            sandbox = %sandbox.id,
            kernel = self.kernel,
            image = %self.shape.image,
            "sandbox provisioned"
        );
        let sandbox = self.await_running(&sandbox.id).await?;

        // Handed over before anything attaches, which is what makes this
        // possible at all: the supervisor does not start the agent until the
        // first attachment, so a conversation delivered now is in place before
        // the agent looks for one.
        if let Some(conversation) = spec.restore_from.as_ref() {
            self.hand_over(&sandbox.id, conversation).await?;
        }

        Ok(RuntimeHandle(sandbox.id))
    }

    /// Capture the agent's conversation so it can be resumed elsewhere.
    pub async fn snapshot(&self, handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let captured = self
            .client
            .get_from_sandbox(&endpoint.endpoint, CONVERSATION_PATH)
            .await?;
        match captured.as_ref() {
            Some(bytes) => tracing::info!(
                sandbox = %handle.0,
                bytes = bytes.len(),
                "captured this session's conversation"
            ),
            None => tracing::info!(sandbox = %handle.0, "this session has no conversation yet"),
        }
        Ok(captured)
    }

    /// Every sandbox this provider is running, with what each was created for.
    pub async fn list_runtimes(&self) -> Result<Vec<RuntimeListing>, AdapterError> {
        let sandboxes = self
            .client
            .list()
            .await
            .map_err(|e| AdapterError::Unavailable(e.to_string()))?;
        Ok(sandboxes
            .into_iter()
            .map(|s| RuntimeListing {
                session: s.metadata.get(SESSION_METADATA_KEY).cloned(),
                created_by: s.metadata.get(CREATOR_METADATA_KEY).cloned(),
                handle: RuntimeHandle(s.id),
            })
            .collect())
    }

    /// Tear a sandbox down.
    pub async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        // Dropped first: the link belongs to a sandbox that is about to stop
        // existing, and keeping it would leave this map growing with the
        // remains of every session this process has ever served.
        self.links.lock().await.remove(&handle.0);
        self.client.delete(&handle.0).await?;
        tracing::info!(sandbox = %handle.0, "sandbox deleted");
        Ok(())
    }

    /// Give a new sandbox the conversation it is taking over.
    ///
    /// A failure here fails the provisioning. The alternative is a sandbox
    /// that starts a fresh conversation while the control plane believes the
    /// old one was carried across — and the runtime it came from is about to
    /// be torn down, so nothing would notice until its owner did.
    async fn hand_over(&self, sandbox: &str, conversation: &[u8]) -> Result<(), AdapterError> {
        let endpoint = self.client.endpoint(sandbox, SUPERVISOR_PORT).await?;
        self.client
            .post_to_sandbox(&endpoint.endpoint, CONVERSATION_PATH, conversation.to_vec())
            .await?;
        tracing::info!(
            %sandbox,
            bytes = conversation.len(),
            "handed this session's conversation to its new runtime"
        );
        Ok(())
    }

    /// Remember the link to a runtime, so a credential can reach it later.
    pub(crate) async fn remember_link(&self, handle: &str, link: mpsc::Sender<TerminalInput>) {
        self.links.lock().await.insert(handle.to_string(), link);
    }

    /// The link to a runtime, if one is open.
    pub(crate) async fn link_to(&self, handle: &str) -> Option<mpsc::Sender<TerminalInput>> {
        self.links.lock().await.get(handle).cloned()
    }

    /// Forget a link that has closed.
    pub(crate) async fn forget_link(&self, handle: &str) {
        self.links.lock().await.remove(handle);
        // A waiter on a link that has gone will never be answered, so it is
        // dropped here rather than left to time out: dropping the sender
        // wakes the waiter at once and the caller rotates.
        self.switch_waiters.forget(handle).await;
    }

    /// The registry a link's inbound task answers into.
    pub(crate) fn switch_waiters(&self) -> SwitchWaiters {
        self.switch_waiters.clone()
    }

    /// Wait until a sandbox reports itself running.
    async fn await_running(&self, id: &str) -> Result<Sandbox, AdapterError> {
        let deadline = std::time::Instant::now() + READY_TIMEOUT;
        loop {
            let sandbox = self.client.get(id).await?;
            if sandbox.is_running() {
                return Ok(sandbox);
            }
            if std::time::Instant::now() >= deadline {
                return Err(AdapterError::Unavailable(format!(
                    "sandbox {id} did not start within {}s",
                    READY_TIMEOUT.as_secs()
                )));
            }
            tokio::time::sleep(READY_POLL).await;
        }
    }
}

#[cfg(test)]
#[path = "hosted_tests.rs"]
mod hosted_tests;

/// The socket a conversation rides, exposed for control frames alone.
///
/// A credential travels to a runtime as a control frame on a link the adapter
/// has recorded. Recording one used to happen in exactly one place — attaching
/// a terminal — so a conversation session had no recorded link, and every
/// attempt to give it a credential failed with "has no open link to carry a
/// credential". Two of the three kernels serve only conversations, so for
/// those there was no path to a credential at all.
///
/// Only a credential is written here. Anything else arriving on this sender is
/// dropped loudly rather than framed onto a lane it does not belong to: the
/// conversation itself has its own typed channel, and terminal bytes have no
/// meaning on a socket nobody is painting.
pub(crate) fn control_lane<S>(
    sink: std::sync::Arc<tokio::sync::Mutex<S>>,
) -> mpsc::Sender<TerminalInput>
where
    S: futures_util::SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<TerminalInput>(CONTROL_BUFFER);
    tokio::spawn(async move {
        while let Some(input) = rx.recv().await {
            let message = match input {
                TerminalInput::Credential(value) => tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::json!({ "t": "credential", "value": value }).to_string(),
                ),
                TerminalInput::SwitchPlane { command } => {
                    tokio_tungstenite::tungstenite::Message::Text(
                        serde_json::json!({ "t": "switch_plane", "command": command }).to_string(),
                    )
                }
                _ => {
                    tracing::warn!(
                        "a conversation's control lane carries credentials and plane switches only"
                    );
                    continue;
                }
            };
            // Never logged with a credential's value, here or anywhere below this line.
            if sink.lock().await.send(message).await.is_err() {
                tracing::warn!("a conversation's control lane closed");
                break;
            }
        }
    });
    tx
}

/// How many control frames may queue for one runtime.
///
/// Small on purpose: the only thing that travels here is a credential, and a
/// backlog of them means the newest is the only one worth sending.
const CONTROL_BUFFER: usize = 4;
