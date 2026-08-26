//! Talking to a runtime that is already running.
//!
//! Split from the sandbox lifecycle so both stay within the project's size
//! limit, on a real division: that file brings a runtime into existence, and
//! this settles how one is spoken to once it does.
//!
//! Two dialects, because CLAUDE.md §5 has both halves of a rollout in flight at
//! once — pods that predate lanes running against a control plane that knows
//! them, and the reverse. Neither guesses: the control plane asks for the wire
//! it speaks, and a pod that knows lanes answers by declaring itself.

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::{AbortLinkOnDrop, HostedKernel, SUPERVISOR_PORT};
use crate::supervisor;
use crate::task_retirement::TaskRetirement;
use crate::{
    AdapterError, RuntimeHandle, TerminalChannel, TerminalInput, TerminalInputSender,
    TerminalOutput,
};

/// How many output chunks may queue before the relay applies backpressure.
///
/// A full-screen application repaints in bursts, so a too-small buffer stalls
/// the reader on every repaint.
const OUTPUT_BUFFER: usize = 256;

/// How many input events may queue toward the runtime.
const INPUT_BUFFER: usize = 64;

/// How long to wait for a runtime to declare which wire it speaks.
///
/// Paid once per attach, and only by a runtime that predates the declaration
/// and happens to be silent while it starts — one that knows the wire answers
/// immediately, and an older one usually answers with terminal output just as
/// fast. Bounded so a silent runtime settles on the older wire rather than
/// holding an attach open indefinitely.
pub(crate) const DIALECT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a runtime has to say whether it switched plane.
///
/// The work is a process restart the supervisor has already been told to
/// make, so an answer is quick or is not coming. The bound is paid only by a
/// runtime too old to know the frame, and paying it once buys the rotation
/// that carries such a session correctly.
const SWITCH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a runtime has to confirm an atomic credential installation.
#[cfg(not(test))]
const CREDENTIAL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(test)]
const CREDENTIAL_DEADLINE: std::time::Duration = std::time::Duration::from_millis(100);

/// The tag a supervisor answers a plane switch with, matching the runtime's
/// own constant. One word, agreed at both ends.
pub(crate) const SWITCH_RESULT: &str = "switch_plane_result";

/// The tag a supervisor answers a credential installation with.
pub(crate) const CREDENTIAL_RESULT: &str = "credential_result";

/// Decode a process ending reported by the supervisor.
///
/// The `code` field is required even when its value is null. Treating a missing
/// field as a signal ending would fabricate a report from a malformed or newer
/// control frame.
pub(crate) fn exit_report(text: &str) -> Option<TerminalOutput> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("t")?.as_str()? != "exit" {
        return None;
    }
    let code = match value.get("code")? {
        serde_json::Value::Null => None,
        serde_json::Value::Number(number) => Some(i32::try_from(number.as_i64()?).ok()?),
        _ => return None,
    };
    Some(TerminalOutput::Exited { code })
}

/// Read a runtime's answer to a plane switch, if that is what this frame is.
///
/// `None` for anything else, including a frame from a runtime that speaks a
/// dialect this build has never seen: an unreadable answer is not an answer,
/// and the caller's timeout carries it to a rotation.
pub(crate) fn switch_answer(text: &str) -> Option<bool> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("t")?.as_str()? != SWITCH_RESULT {
        return None;
    }
    value.get("accepted")?.as_bool()
}

/// Read a credential result without accepting a malformed or uncorrelated frame.
pub(crate) fn credential_answer(text: &str) -> Option<(String, bool)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("t")?.as_str()? != CREDENTIAL_RESULT {
        return None;
    }
    Some((
        value.get("operation_id")?.as_str()?.to_string(),
        value.get("installed")?.as_bool()?,
    ))
}

/// A socket to a runtime's link.
pub(crate) type Link = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

impl HostedKernel {
    /// Turn a provider endpoint into the supervisor's link URL.
    ///
    /// The provider returns a host and path without a scheme, so the scheme is
    /// taken from the base URL it was reached on.
    ///
    /// The wire is asked for rather than assumed. A pod that predates lanes
    /// ignores the parameter and keeps speaking what it always did; one that
    /// does not takes it as permission to frame. Neither has to guess, which
    /// is what makes a half-rolled-out fleet safe.
    pub(crate) fn link_url(&self, endpoint: &str, session_id: &str) -> String {
        let scheme = if self.client.base_url().starts_with("https://") {
            "wss"
        } else {
            "ws"
        };
        let host_and_path = endpoint
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
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
    pub(crate) async fn connect(
        &self,
        handle: &RuntimeHandle,
        lanes: Option<&str>,
    ) -> Result<(Link, String), AdapterError> {
        let extra = lanes
            .map(|lanes| format!("&lanes={lanes}"))
            .unwrap_or_default();
        self.connect_with(handle, &extra).await
    }

    pub(crate) async fn connect_with(
        &self,
        handle: &RuntimeHandle,
        extra_query: &str,
    ) -> Result<(Link, String), AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        self.connect_at(handle, &endpoint.endpoint, extra_query)
            .await
    }

    pub(super) async fn connect_at(
        &self,
        handle: &RuntimeHandle,
        endpoint: &str,
        extra_query: &str,
    ) -> Result<(Link, String), AdapterError> {
        let mut url = self.link_url(endpoint, &handle.0);
        url.push_str(extra_query);
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

    /// Attach to a runtime's terminal.
    pub(crate) async fn attach_terminal(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        let (stream, url) = self.connect(handle, None).await?;
        tracing::info!(sandbox = %handle.0, %url, "attaching to sandbox terminal");
        let (sink, mut source) = stream.split();

        // Settled before anything is relayed, so both directions agree on how
        // a binary frame is read for the whole life of the link.
        let (framed, early) = Self::negotiate(&mut source).await;

        let (out_tx, out_rx) = mpsc::channel::<TerminalOutput>(OUTPUT_BUFFER);
        let sandbox = handle.0.clone();
        let sink = std::sync::Arc::new(tokio::sync::Mutex::new(sink));
        let lifecycle = self.link_lifecycle(&handle.0);
        let lifecycle_for_writer = lifecycle.clone();
        let in_tx = TerminalInputSender::spawn(INPUT_BUFFER, move |input| {
            let credential_operation = match &input {
                TerminalInput::Credential { operation_id, .. } => Some(operation_id.clone()),
                _ => None,
            };
            let message = Self::encode(input, framed);
            let sandbox = sandbox.clone();
            let sink = std::sync::Arc::clone(&sink);
            let lifecycle = lifecycle_for_writer.clone();
            async move {
                if let Err(error) = sink.lock().await.send(message).await {
                    tracing::warn!(%sandbox, %error, "sandbox terminal write failed");
                    if let Some(operation_id) = credential_operation {
                        lifecycle.fail_credential(&operation_id).await;
                    }
                    return Err(AdapterError::Unavailable(format!(
                        "sandbox terminal write failed: {error}"
                    )));
                }
                Ok(())
            }
        });

        let mut abort_on_drop = AbortLinkOnDrop::new(in_tx.clone());
        let whole_retirement = TaskRetirement::default();
        let whole_guard = whole_retirement.guard();
        let (installed, installation) = tokio::sync::oneshot::channel();

        let sandbox = handle.0.clone();
        let link_for_reader = in_tx.downgrade();
        let lifecycle_for_reader = lifecycle.clone();
        let link_for_cleanup = in_tx.downgrade();
        let writer_retired = in_tx.retirement();
        let writer_abort = in_tx.abort_handle();
        let lifecycle_for_cleanup = lifecycle.clone();
        let reader = tokio::spawn(async move {
            // Output that arrived while the wire was being settled. Sent
            // first, because it is the top of somebody's screen.
            if let Some(bytes) = early {
                if out_tx.send(TerminalOutput::Bytes(bytes)).await.is_err() {
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
                        if out_tx.send(TerminalOutput::Bytes(payload)).await.is_err() {
                            break; // consumer went away
                        }
                    }
                    Ok(Message::Text(text)) => {
                        if let Some((operation_id, installed)) = credential_answer(&text) {
                            if let Some(link) = link_for_reader.upgrade() {
                                lifecycle_for_reader
                                    .settle_credential(&link, &operation_id, installed)
                                    .await;
                            }
                        } else if let Some(accepted) = switch_answer(&text) {
                            if let Some(link) = link_for_reader.upgrade() {
                                lifecycle_for_reader.settle(&link, accepted).await;
                            }
                        } else if let Some(ending) = exit_report(&text) {
                            // The supervisor sends this only after it has
                            // stopped forwarding pty bytes. Sharing the output
                            // queue preserves that order into the session core.
                            let _ = out_tx.send(ending).await;
                            break;
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {} // other control frames are not used yet
                    Err(e) => {
                        tracing::warn!(%sandbox, error = %e, "sandbox terminal read failed");
                        break;
                    }
                }
            }
            tracing::info!(%sandbox, "sandbox output relay ended");
        });

        // A socket generation owns both halves. Whichever half ends first
        // retires the other, then removes only this exact registration.
        tokio::spawn(async move {
            let _whole_guard = whole_guard;
            let mut reader = reader;
            let writer_ended = writer_retired.clone();
            tokio::select! {
                _ = writer_ended.wait() => {
                    reader.abort();
                    let _ = reader.await;
                }
                _ = &mut reader => {
                    writer_abort.abort();
                    writer_retired.wait().await;
                }
            }
            if installation.await.is_ok() {
                if let Some(link) = link_for_cleanup.upgrade() {
                    lifecycle_for_cleanup.forget(&link).await;
                }
            }
        });

        if !lifecycle
            .register(in_tx.clone(), whole_retirement.clone(), installed)
            .await
        {
            in_tx.abort();
            whole_retirement.wait().await;
            return Err(AdapterError::Unavailable(format!(
                "runtime {} terminal link was superseded while opening",
                handle.0
            )));
        }

        abort_on_drop.disarm();
        Ok(TerminalChannel {
            output: out_rx,
            input: in_tx,
        })
    }

    /// Replace the credential a running runtime presents when calling out.
    ///
    /// The supervisor learns of one from a control frame on the link — which
    /// is where the frame has always gone, and why it keeps going there: a
    /// runtime already running was built against that dialect, and §5 does not
    /// permit breaking it to tidy up a name.
    pub(crate) async fn refresh_credential(
        &self,
        handle: &RuntimeHandle,
        credential: &str,
    ) -> Result<(), AdapterError> {
        let operation_id = uuid::Uuid::new_v4().simple().to_string();
        // Choose the socket generation and register its exact operation before
        // enqueueing. A replacement is therefore wholly before or after this
        // point, and even an immediate result cannot outrun its waiter.
        let link_and_answer = {
            let links = self.links.lock().await;
            match links.get(&handle.0) {
                Some(link) => {
                    let link = link.sender.clone();
                    let answer = self
                        .credential_waiters
                        .expect(&handle.0, &operation_id, &link)
                        .await;
                    Some((link, answer))
                }
                None => None,
            }
        };
        let Some((link, answer)) = link_and_answer else {
            return Err(AdapterError::Unavailable(format!(
                "runtime {} has no open link to carry a credential",
                handle.0
            )));
        };
        // Never logged with its value, here or anywhere below this line.
        if link
            .send(TerminalInput::Credential {
                operation_id: operation_id.clone(),
                value: credential.to_string(),
            })
            .await
            .is_err()
        {
            self.credential_waiters
                .forget_operation(&handle.0, &operation_id, &link)
                .await;
            // A dead writer wakes its generation supervisor, which retires the
            // reader before forgetting this exact registration. Queue pressure
            // is not a dead generation and deliberately leaves it registered.
            self.link_lifecycle(&handle.0)
                .forget_if_whole_retired(&link)
                .await;
            return Err(AdapterError::Unavailable(format!(
                "runtime {}'s link closed before the credential reached it",
                handle.0
            )));
        }
        match tokio::time::timeout(CREDENTIAL_DEADLINE, answer).await {
            Ok(Ok(true)) => {
                tracing::info!(sandbox = %handle.0, "credential installed by the runtime");
                Ok(())
            }
            Ok(Ok(false)) => Err(AdapterError::Unavailable(format!(
                "runtime {} did not install the credential",
                handle.0
            ))),
            Ok(Err(_)) => Err(AdapterError::Unavailable(format!(
                "runtime {}'s link ended before it acknowledged the credential",
                handle.0
            ))),
            Err(_) => {
                self.credential_waiters
                    .forget_operation(&handle.0, &operation_id, &link)
                    .await;
                Err(AdapterError::Unavailable(format!(
                    "runtime {} did not acknowledge the credential",
                    handle.0
                )))
            }
        }
    }

    /// Restart the supervised program under a different front-end command.
    ///
    /// The command must be one the pod was told at boot it may run; the
    /// supervisor refuses anything else by name.
    pub(crate) async fn switch_plane(
        &self,
        handle: &RuntimeHandle,
        command: &str,
    ) -> Result<(), AdapterError> {
        // Select the current generation and install its waiter while holding
        // the link registry lock. A replacement therefore happens wholly
        // before or after registration; an obsolete operation cannot pause
        // between the two and overwrite its successor's waiter.
        let link_and_answer = {
            let links = self.links.lock().await;
            match links.get(&handle.0) {
                Some(link) => {
                    let link = link.sender.clone();
                    let answer = self.switch_waiters.expect(&handle.0, &link).await;
                    Some((link, answer))
                }
                None => None,
            }
        };
        let Some((link, answer)) = link_and_answer else {
            return Err(AdapterError::Unavailable(format!(
                "runtime {} has no open link to switch plane",
                handle.0
            )));
        };
        if link
            .send(TerminalInput::SwitchPlane {
                command: command.to_string(),
            })
            .await
            .is_err()
        {
            self.link_lifecycle(&handle.0)
                .forget_if_whole_retired(&link)
                .await;
            return Err(AdapterError::Unavailable(format!(
                "runtime {}'s link closed before the switch reached it",
                handle.0
            )));
        }

        // Delivery is not performance. A supervisor that predates #217 has no
        // word for this frame and ignores it in silence, so waiting is the
        // only thing that tells the two apart — and the caller turns an error
        // here into a rotation, which is what such a runtime needs anyway.
        match tokio::time::timeout(SWITCH_DEADLINE, answer).await {
            Ok(Ok(true)) => {
                tracing::info!(sandbox = %handle.0, command, "the runtime switched plane in place");
                Ok(())
            }
            Ok(Ok(false)) => Err(AdapterError::Unsupported(format!(
                "runtime {} refused the command {command}",
                handle.0
            ))),
            // The link closed under the waiter.
            Ok(Err(_)) => Err(AdapterError::Unavailable(format!(
                "runtime {}'s link ended before it answered the switch",
                handle.0
            ))),
            Err(_) => {
                self.switch_waiters().forget(&handle.0, &link).await;
                Err(AdapterError::Unavailable(format!(
                    "runtime {} did not answer the switch",
                    handle.0
                )))
            }
        }
    }

    /// Learn which wire this pod speaks, from the first thing it says.
    ///
    /// A pod that knows lanes declares itself before anything else, so a
    /// declaration arrives immediately or not at all. A pod that does not
    /// simply starts relaying its terminal, which is itself the answer — and
    /// the bytes it sent while answering are handed back rather than dropped,
    /// because they are the first thing on somebody's screen.
    ///
    /// Waiting is bounded, and the bound is only ever paid by an older pod
    /// whose agent happens to be silent at startup. A newer one answers at
    /// once; an older one usually answers with output just as fast.
    pub(super) async fn negotiate<S>(source: &mut S) -> (bool, Option<Vec<u8>>)
    where
        S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        match tokio::time::timeout(DIALECT_DEADLINE, source.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                match supervisor::Ready::from_control(&text) {
                    Some(ready) => {
                        tracing::info!(
                            wire = ready.wire_version,
                            lanes = ?ready.lanes,
                            "the runtime declared its wire"
                        );
                        (true, None)
                    }
                    // Some other control frame. Nothing is inferred from it:
                    // the older wire is the reading a pod cannot misunderstand.
                    None => (false, None),
                }
            }
            // Terminal output before any declaration means there was never
            // going to be one.
            Ok(Some(Ok(Message::Binary(bytes)))) => (false, Some(bytes.to_vec())),
            Ok(Some(Ok(_))) => (false, None),
            Ok(Some(Err(_))) | Ok(None) => (false, None),
            Err(_) => {
                tracing::info!("the runtime declared no wire; using the one that predates lanes");
                (false, None)
            }
        }
    }

    /// Render one piece of terminal input for the wire this link speaks.
    ///
    /// Only the data lane is framed. Control travels as text and is identical
    /// on both dialects, which is what lets a credential reach a pod that
    /// predates lanes and one that does not without either being told which it
    /// is talking to.
    pub(super) fn encode(input: TerminalInput, framed: bool) -> Message {
        match input {
            TerminalInput::Bytes(b) => Message::Binary(match framed {
                true => supervisor::frame(supervisor::LANE_TERMINAL, &b),
                false => b,
            }),
            TerminalInput::Resize(size) => Message::Text(format!(
                r#"{{"t":"resize","cols":{},"rows":{}}}"#,
                size.cols, size.rows
            )),
            TerminalInput::Signal(sig) => {
                Message::Text(format!(r#"{{"t":"signal","sig":"{sig}"}}"#))
            }
            TerminalInput::Credential {
                operation_id,
                value,
            } => Message::Text(
                serde_json::json!({
                    "t": "credential",
                    "operation_id": operation_id,
                    "value": value,
                })
                .to_string(),
            ),
            TerminalInput::SwitchPlane { command } => Message::Text(
                serde_json::json!({ "t": "switch_plane", "command": command }).to_string(),
            ),
        }
    }
}
