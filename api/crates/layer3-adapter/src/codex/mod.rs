//! Adapter driving the Codex kernel inside a provisioned sandbox.
//!
//! The kernel never runs in the control plane. Each session gets its own
//! sandbox from the provider, running an image that carries the kernel and a
//! supervisor that hosts it under a real pseudo-terminal. This adapter owns
//! that lifecycle and relays the terminal back over a WebSocket.

use std::collections::BTreeMap;

use crate::RuntimePermission;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::sandbox::{ImageSpec, Sandbox, SandboxClient, SandboxError, SandboxRequest};
use crate::{
    AdapterError, KernelAdapter, RuntimeHandle, RuntimeListing, RuntimeSpec, TerminalChannel,
    TerminalInput,
};

/// Port the supervisor listens on inside the sandbox.
const SUPERVISOR_PORT: u16 = 9090;

/// Metadata key naming the control plane that created a sandbox.
///
/// Written at provisioning and read by reconciliation, so a provider shared
/// between control planes can be told apart. Absent on sandboxes created before
/// this was recorded, which reconciliation treats as its own business to decide.
const CREATOR_METADATA_KEY: &str = "chrono-bot/control-plane";

/// Where the supervisor serves and accepts the agent's conversation.
const CONVERSATION_PATH: &str = "/runtime/conversation";

/// Metadata key correlating a sandbox back to its session.
///
/// Written at provisioning and read by reconciliation, so the two must agree;
/// naming it once is what keeps them in step.
const SESSION_METADATA_KEY: &str = "chrono-bot/session";

/// How many output chunks may queue before the relay applies backpressure.
///
/// A full-screen application repaints in bursts, so a too-small buffer stalls
/// the reader on every repaint.
const OUTPUT_BUFFER: usize = 256;

/// How many input events may queue toward the runtime.
const INPUT_BUFFER: usize = 64;

/// How long to wait for a freshly created sandbox to start serving.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How often to re-check a sandbox that is not yet running.
const READY_POLL: std::time::Duration = std::time::Duration::from_secs(2);

impl From<SandboxError> for AdapterError {
    fn from(e: SandboxError) -> Self {
        match e {
            SandboxError::Rejected { status, detail } => {
                AdapterError::Rejected(format!("{status}: {detail}"))
            }
            other => AdapterError::Unavailable(other.to_string()),
        }
    }
}

/// How sandboxes for this kernel are shaped.
#[derive(Debug, Clone)]
pub struct CodexSandboxSpec {
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
    /// Where the supervisor forwards the agent's model calls.
    ///
    /// Empty means the agent runs with no model endpoint: still a usable
    /// terminal, but unable to complete a turn.
    pub upstream_base_url: String,
    /// Model the agent should select.
    pub upstream_model: String,
    /// Request shape the upstream speaks.
    pub upstream_wire_api: String,
    /// Root of the identity platform the session's CLI acts against.
    ///
    /// Empty means the session gets a terminal and a model but cannot use the
    /// owner's connected capabilities.
    pub platform_base_url: String,
    /// The subcommand that makes the agent continue a named conversation.
    ///
    /// The conversation's own identifier is appended to this, so it must be a
    /// form that takes one. Asking for "the latest" instead does not work: the
    /// agent selects among conversations recorded in the current working
    /// directory, and one carried from another pod was recorded somewhere
    /// else, so it finds nothing, starts a fresh conversation and reports
    /// success — silent loss rather than a failure.
    ///
    /// Configured rather than fixed, because this belongs to the agent and
    /// changes between its versions, and getting it wrong has to be
    /// correctable without building a new image.
    pub resume_args: String,
}

impl Default for CodexSandboxSpec {
    fn default() -> Self {
        Self {
            image: "chrono-bot-runtime:dev".into(),
            entrypoint: vec!["/usr/local/bin/layer4-runtime".into()],
            resource_limits: BTreeMap::from([
                ("cpu".to_string(), "1".to_string()),
                ("memory".to_string(), "1Gi".to_string()),
            ]),
            timeout_seconds: None,
            upstream_base_url: String::new(),
            upstream_model: "gpt-5.6-sol".into(),
            upstream_wire_api: "responses".into(),
            platform_base_url: String::new(),
            resume_args: "resume".into(),
        }
    }
}

/// Drives Codex inside a provider-managed sandbox.
pub struct CodexAdapter {
    client: SandboxClient,
    spec: CodexSandboxSpec,
}

impl CodexAdapter {
    /// Build an adapter over a sandbox provider.
    pub fn new(client: SandboxClient, spec: CodexSandboxSpec) -> Self {
        Self { client, spec }
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

    /// Turn a provider endpoint into the supervisor's terminal URL.
    ///
    /// The provider returns a host and path without a scheme, so the scheme is
    /// taken from the base URL it was reached on.
    fn terminal_url(&self, endpoint: &str, session_id: &str) -> String {
        let scheme = if self.client.base_url().starts_with("https://") {
            "wss"
        } else {
            "ws"
        };
        let host_and_path = endpoint
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        format!("{scheme}://{host_and_path}/runtime/{session_id}/terminal")
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

impl CodexAdapter {
    /// The environment a sandbox is started with.
    ///
    /// Separated from provisioning because this is where the core's intent is
    /// rendered into this kernel's own vocabulary, and a wrong rendering here
    /// grants more than the person asked for. Worth being able to inspect
    /// without standing up a provider.
    fn environment_for(&self, spec: &RuntimeSpec) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("CHRONO_RUNTIME_COMMAND".to_string(), "codex".to_string()),
            (
                "CHRONO_RUNTIME_RESUME_ARGS".to_string(),
                self.spec.resume_args.clone(),
            ),
            ("TERM".to_string(), "xterm-256color".to_string()),
            // The supervisor forwards here and attaches the owner's credential.
            // The credential itself never travels as environment: it arrives
            // over the control channel and lives only in supervisor memory.
            (
                "CHRONO_UPSTREAM_BASE_URL".to_string(),
                self.spec.upstream_base_url.clone(),
            ),
            (
                "CHRONO_UPSTREAM_MODEL".to_string(),
                // A model the session asked for wins over the deployment's
                // default; the default is what an unconfigured session gets.
                spec.options
                    .model
                    .clone()
                    .unwrap_or_else(|| self.spec.upstream_model.clone()),
            ),
            // This kernel's own names for the intent the core expressed. The
            // translation lives here because these strings mean nothing to any
            // other runtime.
            (
                "CHRONO_AGENT_EFFORT".to_string(),
                spec.options
                    .reasoning_effort
                    .clone()
                    .unwrap_or_else(|| "medium".to_string()),
            ),
            (
                "CHRONO_AGENT_APPROVAL".to_string(),
                match spec.options.permission {
                    RuntimePermission::Guarded => "on-request",
                    RuntimePermission::Autonomous => "on-failure",
                    // No approvals at all. Survivable only because the sandbox
                    // is still the boundary; this removes the ones inside it.
                    RuntimePermission::Unrestricted => "never",
                }
                .to_string(),
            ),
            (
                "CHRONO_AGENT_SANDBOX".to_string(),
                match spec.options.permission {
                    RuntimePermission::Guarded => "read-only",
                    RuntimePermission::Autonomous => "workspace-write",
                    RuntimePermission::Unrestricted => "danger-full-access",
                }
                .to_string(),
            ),
            (
                "CHRONO_AGENT_OWN_CREDENTIAL".to_string(),
                spec.options.own_credential.to_string(),
            ),
            (
                "CHRONO_UPSTREAM_WIRE_API".to_string(),
                self.spec.upstream_wire_api.clone(),
            ),
            (
                "CHRONO_PLATFORM_BASE_URL".to_string(),
                self.spec.platform_base_url.clone(),
            ),
            // The CLI reads its endpoint and token from the environment, and
            // environment wins over any saved session. Pointing it at the local
            // proxy with an inert token means the real credential never lands
            // in the sandbox, and the CLI never tries to refresh one itself.
            ("NYXID_URL".to_string(), "http://127.0.0.1:9092".to_string()),
            (
                "NYXID_ACCESS_TOKEN".to_string(),
                "local-proxy-no-credential-needed".to_string(),
            ),
            (
                "CHRONO_RUNTIME_COLS".to_string(),
                spec.size.cols.to_string(),
            ),
            (
                "CHRONO_RUNTIME_ROWS".to_string(),
                spec.size.rows.to_string(),
            ),
        ])
    }
}

#[async_trait::async_trait]
impl KernelAdapter for CodexAdapter {
    fn kernel_name(&self) -> &'static str {
        "codex"
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        // Geometry is passed as environment so the supervisor allocates its pty
        // at roughly the right size before the first attach refines it.
        let env = self.environment_for(spec);

        let request = SandboxRequest {
            image: ImageSpec {
                uri: self.spec.image.clone(),
            },
            entrypoint: self.spec.entrypoint.clone(),
            env,
            resource_limits: self.spec.resource_limits.clone(),
            timeout: self.spec.timeout_seconds,
            // Correlates a sandbox back to the session that owns it, so an
            // orphan can be found and reclaimed.
            metadata: {
                let mut m = BTreeMap::from([
                    (SESSION_METADATA_KEY.to_string(), spec.session_id.clone()),
                    ("chrono-bot/kernel".to_string(), "codex".to_string()),
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
            image = %self.spec.image,
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

    async fn snapshot(&self, handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
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

    async fn attach_terminal(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let url = self.terminal_url(&endpoint.endpoint, &handle.0);
        tracing::info!(sandbox = %handle.0, %url, "attaching to sandbox terminal");

        // The proxied endpoint is authenticated, so the handshake carries the
        // provider key. A browser could not set this header, which is exactly
        // why the control plane holds the attachment and the browser talks only
        // to the control plane.
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| AdapterError::Unavailable(format!("bad terminal url {url}: {e}")))?;
        let (header, key) = self.client.auth_header();
        request.headers_mut().insert(
            header,
            HeaderValue::from_str(key)
                .map_err(|e| AdapterError::Unavailable(format!("bad api key header: {e}")))?,
        );

        let (stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| AdapterError::Unavailable(format!("connect {url}: {e}")))?;
        let (mut sink, mut source) = stream.split();

        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_BUFFER);
        let (in_tx, mut in_rx) = mpsc::channel::<TerminalInput>(INPUT_BUFFER);

        let sandbox = handle.0.clone();
        tokio::spawn(async move {
            while let Some(frame) = source.next().await {
                match frame {
                    Ok(Message::Binary(bytes)) => {
                        if out_tx.send(bytes.to_vec()).await.is_err() {
                            break; // consumer went away
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {} // supervisor control frames are not used yet
                    Err(e) => {
                        tracing::warn!(%sandbox, error = %e, "sandbox terminal read failed");
                        break;
                    }
                }
            }
            tracing::info!(%sandbox, "sandbox output relay ended");
        });

        let sandbox = handle.0.clone();
        tokio::spawn(async move {
            while let Some(input) = in_rx.recv().await {
                let message = match input {
                    TerminalInput::Bytes(b) => Message::Binary(b),
                    TerminalInput::Resize(size) => Message::Text(format!(
                        r#"{{"t":"resize","cols":{},"rows":{}}}"#,
                        size.cols, size.rows
                    )),
                    TerminalInput::Signal(sig) => {
                        Message::Text(format!(r#"{{"t":"signal","sig":"{sig}"}}"#))
                    }
                    TerminalInput::Credential(value) => Message::Text(
                        serde_json::json!({ "t": "credential", "value": value }).to_string(),
                    ),
                };
                if let Err(e) = sink.send(message).await {
                    tracing::warn!(%sandbox, error = %e, "sandbox terminal write failed");
                    break;
                }
            }
            tracing::info!(%sandbox, "sandbox input relay ended");
        });

        Ok(TerminalChannel {
            output: out_rx,
            input: in_tx,
        })
    }

    async fn list_runtimes(&self) -> Result<Vec<RuntimeListing>, AdapterError> {
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

    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        self.client.delete(&handle.0).await?;
        tracing::info!(sandbox = %handle.0, "sandbox deleted");
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
