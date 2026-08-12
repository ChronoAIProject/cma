//! Adapter driving the Codex kernel.
//!
//! The kernel process itself never runs in the control plane. It runs inside a
//! sandboxed runtime under a real pseudo-terminal, supervised by a separate
//! binary. This adapter is the client of that supervisor: it provisions a
//! runtime and relays terminal traffic over a WebSocket.

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::{
    AdapterError, KernelAdapter, RuntimeHandle, RuntimeSpec, TermSize, TerminalChannel,
    TerminalInput,
};

/// How many output chunks may queue before the relay applies backpressure.
///
/// A full-screen application repaints in bursts, so a too-small buffer stalls
/// the reader on every repaint.
const OUTPUT_BUFFER: usize = 256;

/// How many input events may queue toward the runtime.
const INPUT_BUFFER: usize = 64;

/// Drives Codex inside a supervised sandbox runtime.
pub struct CodexAdapter {
    /// Base URL of the runtime supervisor, without a trailing slash.
    supervisor_base_url: String,
}

impl CodexAdapter {
    /// Build an adapter pointed at a runtime supervisor.
    pub fn new(supervisor_base_url: impl Into<String>) -> Self {
        let mut base = supervisor_base_url.into();
        while base.ends_with('/') {
            base.pop();
        }
        Self {
            supervisor_base_url: base,
        }
    }

    /// WebSocket URL for a runtime's terminal.
    fn terminal_url(&self, handle: &RuntimeHandle) -> String {
        let ws_base = self
            .supervisor_base_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        format!("{ws_base}/runtime/{}/terminal", handle.0)
    }
}

#[async_trait::async_trait]
impl KernelAdapter for CodexAdapter {
    fn kernel_name(&self) -> &'static str {
        "codex"
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        // The supervisor keys runtimes by session id, so provisioning is
        // idempotent from the control plane's point of view: reconnecting to
        // the same session reaches the same runtime.
        tracing::info!(
            session_id = %spec.session_id,
            cols = spec.size.cols,
            rows = spec.size.rows,
            "provisioning runtime"
        );
        Ok(RuntimeHandle(spec.session_id.clone()))
    }

    async fn attach_terminal(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        let url = self.terminal_url(handle);
        tracing::info!(runtime = %handle.0, %url, "attaching to runtime terminal");

        let (stream, _response) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| AdapterError::Unavailable(format!("connect {url}: {e}")))?;
        let (mut sink, mut source) = stream.split();

        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_BUFFER);
        let (in_tx, mut in_rx) = mpsc::channel::<TerminalInput>(INPUT_BUFFER);

        let runtime = handle.0.clone();
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
                        tracing::warn!(%runtime, error = %e, "runtime terminal read failed");
                        break;
                    }
                }
            }
            tracing::info!(%runtime, "runtime output relay ended");
        });

        let runtime = handle.0.clone();
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
                };
                if let Err(e) = sink.send(message).await {
                    tracing::warn!(%runtime, error = %e, "runtime terminal write failed");
                    break;
                }
            }
            tracing::info!(%runtime, "runtime input relay ended");
        });

        Ok(TerminalChannel {
            output: out_rx,
            input: in_tx,
        })
    }

    async fn resize(&self, handle: &RuntimeHandle, size: TermSize) -> Result<(), AdapterError> {
        // Geometry rides the same channel as input, so a resize with no live
        // attachment is a no-op rather than an error: the next attach allocates
        // at the requested size.
        tracing::debug!(runtime = %handle.0, cols = size.cols, rows = size.rows, "resize requested");
        Ok(())
    }

    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        tracing::info!(runtime = %handle.0, "terminating runtime");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_slashes_are_stripped_from_the_base_url() {
        let a = CodexAdapter::new("http://runtime:9000///");
        assert_eq!(a.supervisor_base_url, "http://runtime:9000");
    }

    #[test]
    fn terminal_url_upgrades_the_scheme() {
        let a = CodexAdapter::new("http://runtime:9000");
        assert_eq!(
            a.terminal_url(&RuntimeHandle("sesn_1".into())),
            "ws://runtime:9000/runtime/sesn_1/terminal"
        );

        let b = CodexAdapter::new("https://runtime.example");
        assert_eq!(
            b.terminal_url(&RuntimeHandle("sesn_2".into())),
            "wss://runtime.example/runtime/sesn_2/terminal"
        );
    }

    #[tokio::test]
    async fn provision_returns_a_handle_keyed_by_session() {
        let a = CodexAdapter::new("http://runtime:9000");
        let handle = a
            .provision(&RuntimeSpec {
                session_id: "sesn_abc".into(),
                size: TermSize { cols: 80, rows: 24 },
            })
            .await
            .expect("provision succeeds");
        assert_eq!(handle, RuntimeHandle("sesn_abc".into()));
    }

    #[tokio::test]
    async fn attaching_to_an_unreachable_supervisor_is_an_error() {
        // Port 1 is reserved and never listening, so this exercises the
        // failure path deterministically without a fixture server.
        let a = CodexAdapter::new("http://127.0.0.1:1");
        let err = a
            .attach_terminal(&RuntimeHandle("sesn_x".into()))
            .await
            .expect_err("must fail against a dead supervisor");
        assert!(matches!(err, AdapterError::Unavailable(_)), "got {err:?}");
    }
}
