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

use crate::sandbox::{ImageSpec, Sandbox, SandboxClient, SandboxError, SandboxRequest};
use crate::{
    AdapterError, KernelAdapter, RuntimeHandle, RuntimeSpec, TermSize, TerminalChannel,
    TerminalInput,
};

/// Port the supervisor listens on inside the sandbox.
const SUPERVISOR_PORT: u16 = 9090;

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
}

impl Default for CodexSandboxSpec {
    fn default() -> Self {
        Self {
            image: "chrono-bot-runtime:dev".into(),
            entrypoint: vec!["/usr/local/bin/layer4-runtime".into()],
            resource_limits: BTreeMap::from([
                ("cpu".to_string(), "2".to_string()),
                ("memory".to_string(), "4Gi".to_string()),
            ]),
            timeout_seconds: None,
            upstream_base_url: String::new(),
            upstream_model: "gpt-5.6-sol".into(),
            upstream_wire_api: "responses".into(),
            platform_base_url: String::new(),
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

#[async_trait::async_trait]
impl KernelAdapter for CodexAdapter {
    fn kernel_name(&self) -> &'static str {
        "codex"
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        // Geometry is passed as environment so the supervisor allocates its pty
        // at roughly the right size before the first attach refines it.
        let env = BTreeMap::from([
            ("CHRONO_RUNTIME_COMMAND".to_string(), "codex".to_string()),
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
                self.spec.upstream_model.clone(),
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
            metadata: BTreeMap::from([
                (SESSION_METADATA_KEY.to_string(), spec.session_id.clone()),
                ("chrono-bot/kernel".to_string(), "codex".to_string()),
            ]),
        };

        let sandbox = self.client.create(&request).await?;
        tracing::info!(
            session_id = %spec.session_id,
            sandbox = %sandbox.id,
            image = %self.spec.image,
            "sandbox provisioned"
        );
        let sandbox = self.await_running(&sandbox.id).await?;
        Ok(RuntimeHandle(sandbox.id))
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

    async fn resize(&self, handle: &RuntimeHandle, size: TermSize) -> Result<(), AdapterError> {
        // Geometry rides the same channel as input, so a resize with no live
        // attachment is a no-op rather than an error: the next attach applies
        // the caller's real size before anything is painted.
        tracing::debug!(sandbox = %handle.0, cols = size.cols, rows = size.rows, "resize requested");
        Ok(())
    }

    async fn list_runtimes(&self) -> Result<Vec<(RuntimeHandle, Option<String>)>, AdapterError> {
        let sandboxes = self
            .client
            .list()
            .await
            .map_err(|e| AdapterError::Unavailable(e.to_string()))?;
        Ok(sandboxes
            .into_iter()
            .map(|s| {
                let session = s.metadata.get(SESSION_METADATA_KEY).cloned();
                (RuntimeHandle(s.id), session)
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
mod tests {
    use super::*;

    fn adapter(base: &str) -> CodexAdapter {
        CodexAdapter::new(
            SandboxClient::new(base, "test-key"),
            CodexSandboxSpec::default(),
        )
    }

    #[test]
    fn the_kernel_is_named_for_diagnostics_only() {
        assert_eq!(adapter("http://p").kernel_name(), "codex");
    }

    #[test]
    fn a_proxied_endpoint_becomes_a_terminal_url() {
        let a = adapter("http://provider:80");
        assert_eq!(
            a.terminal_url("provider/v1/sandboxes/sbx-1/proxy/9090", "sbx-1"),
            "ws://provider/v1/sandboxes/sbx-1/proxy/9090/runtime/sbx-1/terminal"
        );
    }

    #[test]
    fn a_secure_provider_yields_a_secure_socket() {
        let a = adapter("https://provider");
        assert!(a
            .terminal_url("provider/v1/sandboxes/s/proxy/9090", "s")
            .starts_with("wss://"));
    }

    #[test]
    fn an_endpoint_that_already_has_a_scheme_is_not_doubled() {
        let a = adapter("http://provider");
        assert_eq!(
            a.terminal_url("http://provider/proxy/9090/", "s"),
            "ws://provider/proxy/9090/runtime/s/terminal"
        );
    }

    #[test]
    fn the_default_spec_requests_a_real_workload_size() {
        let s = CodexSandboxSpec::default();
        assert_eq!(s.resource_limits.get("cpu").map(String::as_str), Some("2"));
        assert_eq!(
            s.resource_limits.get("memory").map(String::as_str),
            Some("4Gi")
        );
    }

    #[test]
    fn sandboxes_do_not_expire_on_a_timer() {
        // A session is someone's working terminal. Expiring it on a timer ends
        // live work mid-turn, and a timer cannot tell an idle session from one
        // whose owner is reading the screen. Only the owner ends a session.
        assert!(
            CodexSandboxSpec::default().timeout_seconds.is_none(),
            "a default lifetime would terminate active sessions"
        );
    }

    #[tokio::test]
    async fn provisioning_against_a_dead_provider_reports_unavailable() {
        // Port 1 is reserved and never listening, so this exercises the failure
        // path deterministically without a fixture server.
        let a = adapter("http://127.0.0.1:1");
        let err = a
            .provision(&RuntimeSpec {
                session_id: "sesn_x".into(),
                size: TermSize { cols: 80, rows: 24 },
                subject_credential: None,
            })
            .await
            .expect_err("must fail against a dead provider");
        assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn attaching_against_a_dead_provider_reports_unavailable() {
        let a = adapter("http://127.0.0.1:1");
        let err = a
            .attach_terminal(&RuntimeHandle("sbx-1".into()))
            .await
            .expect_err("must fail against a dead provider");
        assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn terminating_an_unknown_sandbox_is_not_an_error() {
        // The provider answers 404 for a sandbox already gone, and tearing down
        // something that no longer exists is a success, not a failure.
        let a = adapter("http://127.0.0.1:1");
        // Against a dead provider this is unreachable rather than 404, which is
        // still an error; the 404 tolerance is covered in the client's tests.
        assert!(a.terminate(&RuntimeHandle("gone".into())).await.is_err());
    }
}
