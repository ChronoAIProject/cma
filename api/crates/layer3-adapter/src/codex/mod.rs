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
    AdapterError, KernelAdapter, KernelCapabilities, RuntimeHandle, RuntimeListing, RuntimeSpec,
    TerminalChannel, TerminalInput,
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

/// Where this kernel records its conversation, relative to the pod's home.
///
/// This kernel's own name for its own layout, which is why it is spelled in
/// this kernel's directory. The supervisor is told it at provision; it used to
/// be a constant there, in the one component that is supposed to host any
/// kernel at all.
const SESSIONS_DIR: &str = ".codex/sessions";

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

/// The first thing asked for that is not on offer, if any.
///
/// Names one rather than all of them: a caller fixes the first and asks again,
/// and a list of everything wrong is longer to read without being more useful.
fn absent(wanted: &[String], provided: &[String]) -> Option<String> {
    wanted
        .iter()
        .find(|w| !provided.contains(w))
        .map(|w| w.to_string())
}

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
    /// Software the image already carries, as a caller would name it.
    ///
    /// Not a permission list — a statement of fact about the image this
    /// adapter starts. There is no channel for installing anything into a
    /// running sandbox, so a session asking for something outside this is
    /// asking for something it cannot be given, and is refused by name rather
    /// than started without it.
    pub provided_packages: Vec<String>,
    /// Skills the agent in this image can already load, by identifier.
    ///
    /// Same rule and the same reason: no delivery channel exists, so anything
    /// beyond this is a promise the session could not keep.
    pub provided_skills: Vec<String>,
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
            // Empty by default, which refuses everything. A deployment states
            // what its image carries; guessing here would accept a request the
            // image cannot honour, which is the failure this exists to stop.
            provided_packages: Vec::new(),
            provided_skills: Vec::new(),
        }
    }
}

/// Drives Codex inside a provider-managed sandbox.
pub struct CodexAdapter {
    client: SandboxClient,
    spec: CodexSandboxSpec,
    /// The open link to each runtime, by handle.
    ///
    /// Held because a credential has to reach a runtime that is already
    /// running, and this image's supervisor accepts one only over an
    /// attachment. Opening a second attachment to deliver it is not an option:
    /// attaching declares a geometry, and a connection made without one would
    /// resize the pty under whoever is looking at it.
    links: tokio::sync::Mutex<BTreeMap<String, mpsc::Sender<TerminalInput>>>,
}

impl CodexAdapter {
    /// Build an adapter over a sandbox provider.
    pub fn new(client: SandboxClient, spec: CodexSandboxSpec) -> Self {
        Self {
            client,
            spec,
            links: tokio::sync::Mutex::new(BTreeMap::new()),
        }
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
        let mut controlled = BTreeMap::from([
            ("CHRONO_RUNTIME_COMMAND".to_string(), "codex".to_string()),
            (
                "CHRONO_RUNTIME_RESUME_ARGS".to_string(),
                self.spec.resume_args.clone(),
            ),
            // Where this kernel records its conversation, told to the pod
            // rather than known by it. The supervisor hosts any kernel; the
            // directory belongs to this one, so its name is spelled here.
            (
                "CHRONO_RUNTIME_SNAPSHOT_DIR".to_string(),
                SESSIONS_DIR.to_string(),
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
        ]);
        // The session's own variables, added last and refused where they
        // collide. `apply_to` is what enforces that: the names above are how
        // the runtime finds its upstream, its proxy and its credential, so a
        // caller who could overwrite one could redirect the agent's traffic —
        // or the credential attached to it — somewhere of their choosing.
        spec.environment.apply_to(&mut controlled);
        controlled
    }
}

#[async_trait::async_trait]
impl KernelAdapter for CodexAdapter {
    fn kernel_name(&self) -> &'static str {
        "codex"
    }

    fn capabilities(&self) -> KernelCapabilities {
        // Honest for the pseudo-terminal path, which is all this adapter drives
        // today: bytes for a screen, and a conversation the supervisor can
        // capture and hand to a replacement runtime. Nothing structured crosses
        // the boundary yet, so nothing structured is advertised.
        KernelCapabilities {
            conversation: false,
            terminal: true,
            snapshot: true,
            steer: false,
            usage: false,
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
        // Variables are delivered below. Software and skills are delivered by
        // the image and the agent in it, and nothing can be added to a running
        // sandbox — so what the image already carries is honoured and anything
        // else is named and refused.
        if let Some(missing) = absent(&spec.environment.packages, &self.spec.provided_packages) {
            return Err(AdapterError::Unsupported(format!(
                "this runtime's image does not carry {missing}, and software cannot be installed into a running sandbox"
            )));
        }
        if let Some(missing) = absent(&spec.environment.skills, &self.spec.provided_skills) {
            return Err(AdapterError::Unsupported(format!(
                "this runtime's agent cannot load the skill {missing}, and skills cannot be delivered into a running sandbox"
            )));
        }

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

        // Remembered so a credential renewed later can be delivered without
        // opening a second attachment.
        self.links
            .lock()
            .await
            .insert(handle.0.clone(), in_tx.clone());

        Ok(TerminalChannel {
            output: out_rx,
            input: in_tx,
        })
    }

    async fn refresh_credential(
        &self,
        handle: &RuntimeHandle,
        credential: &str,
    ) -> Result<(), AdapterError> {
        // This image's supervisor learns of a new credential from a control
        // frame on the terminal link — which is where the frame has always
        // gone, and why it keeps going there: a runtime already running was
        // built against that dialect, and §5 does not permit breaking it to
        // tidy up a name.
        let link = self.links.lock().await.get(&handle.0).cloned();
        let Some(link) = link else {
            return Err(AdapterError::Unavailable(format!(
                "runtime {} has no open link to carry a credential",
                handle.0
            )));
        };
        // Never logged with its value, here or anywhere below this line.
        if link
            .send(TerminalInput::Credential(credential.to_string()))
            .await
            .is_err()
        {
            // The link died since it was recorded. Forgotten rather than
            // retried: the next attachment records a live one, and keeping a
            // dead sender would make every renewal fail the same way forever.
            self.links.lock().await.remove(&handle.0);
            return Err(AdapterError::Unavailable(format!(
                "runtime {}'s link closed before the credential reached it",
                handle.0
            )));
        }
        tracing::info!(sandbox = %handle.0, "credential delivered to the runtime");
        Ok(())
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
        // Dropped first: the link belongs to a sandbox that is about to stop
        // existing, and keeping it would leave this map growing with the
        // remains of every session this process has ever served.
        self.links.lock().await.remove(&handle.0);
        self.client.delete(&handle.0).await?;
        tracing::info!(sandbox = %handle.0, "sandbox deleted");
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
