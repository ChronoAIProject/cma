//! Adapter driving the Codex kernel inside a provisioned sandbox.
//!
//! The kernel never runs in the control plane. Each session gets its own
//! sandbox from the provider, running an image that carries the kernel and a
//! supervisor that hosts it under a real pseudo-terminal. This adapter owns
//! that lifecycle and relays the terminal back over a WebSocket.

use std::collections::BTreeMap;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

mod acp;
mod dialect;
mod environment;
mod permission;

use environment::absent;
pub use environment::CodexSandboxSpec;

use crate::sandbox::{ImageSpec, Sandbox, SandboxClient, SandboxError, SandboxRequest};
use crate::supervisor;
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

/// How long to wait for a runtime to declare which wire it speaks.
///
/// Paid once per attach, and only by a runtime that predates the declaration
/// and happens to be silent while it starts — one that knows the wire answers
/// immediately, and an older one usually answers with terminal output just as
/// fast. Bounded so a silent runtime settles on the older wire rather than
/// holding an attach open indefinitely.
pub(super) const DIALECT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

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
        // The wire is asked for rather than assumed. A pod that predates lanes
        // ignores the parameter and keeps speaking what it always did; one
        // that does not takes it as permission to frame. Neither has to guess,
        // which is what makes a half-rolled-out fleet safe.
        format!(
            "{scheme}://{host_and_path}/runtime/{session_id}/terminal?wire={}",
            supervisor::WIRE_VERSION
        )
    }

    /// Open an authenticated socket to a runtime's link.
    ///
    /// The proxied endpoint is authenticated, so the handshake carries the
    /// provider key. A browser could not set this header, which is exactly why
    /// the control plane holds the link and the browser talks only to the
    /// control plane.
    async fn connect(
        &self,
        handle: &RuntimeHandle,
        lanes: Option<&str>,
    ) -> Result<
        (
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            String,
        ),
        AdapterError,
    > {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let mut url = self.terminal_url(&endpoint.endpoint, &handle.0);
        if let Some(lanes) = lanes {
            url.push_str("&lanes=");
            url.push_str(lanes);
        }
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| AdapterError::Unavailable(format!("bad runtime url {url}: {e}")))?;
        let (header, key) = self.client.auth_header();
        request.headers_mut().insert(
            header,
            HeaderValue::from_str(key)
                .map_err(|e| AdapterError::Unavailable(format!("bad api key header: {e}")))?,
        );
        let (stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| AdapterError::Unavailable(format!("connect {url}: {e}")))?;
        Ok((stream, url))
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
        let (stream, url) = self.connect(handle, None).await?;
        tracing::info!(sandbox = %handle.0, %url, "attaching to sandbox terminal");
        let (mut sink, mut source) = stream.split();

        // Settled before anything is relayed, so both directions agree on how
        // a binary frame is read for the whole life of the link.
        let (framed, early) = Self::negotiate(&mut source).await;

        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_BUFFER);
        let (in_tx, mut in_rx) = mpsc::channel::<TerminalInput>(INPUT_BUFFER);

        let sandbox = handle.0.clone();
        tokio::spawn(async move {
            // Output that arrived while the wire was being settled. Sent
            // first, because it is the top of somebody's screen.
            if let Some(bytes) = early {
                if out_tx.send(bytes).await.is_err() {
                    return;
                }
            }
            while let Some(frame) = source.next().await {
                match frame {
                    Ok(Message::Binary(bytes)) => {
                        let payload = match framed {
                            true => match supervisor::unframe(&bytes) {
                                Some((supervisor::LANE_TERMINAL, payload)) => payload.to_vec(),
                                // A lane this attachment does not carry. The
                                // conversation plane opens its own link; a
                                // frame from it here would paint protocol
                                // across somebody's terminal.
                                Some((lane, _)) => {
                                    tracing::warn!(%sandbox, lane, "frame on a lane this attachment does not carry");
                                    continue;
                                }
                                None => continue,
                            },
                            false => bytes.to_vec(),
                        };
                        if out_tx.send(payload).await.is_err() {
                            break; // consumer went away
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {} // control frames beyond the declaration are not used yet
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
                let message = Self::encode(input, framed);
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

    async fn open_conversation(
        &self,
        handle: &RuntimeHandle,
        options: &crate::RuntimeOptions,
    ) -> Result<crate::ConversationChannel, AdapterError> {
        let (stream, url) = self.connect(handle, Some("conv")).await?;
        tracing::info!(sandbox = %handle.0, %url, "opening the sandbox conversation");
        acp::Bridge::open(stream, self.spec.workspace.clone(), options.permission).await
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

#[cfg(test)]
#[path = "link_tests.rs"]
mod link_tests;
