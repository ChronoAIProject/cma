//! Driving this harness's own wire, and reading it as the contract.
//!
//! The other two kernels speak the Agent Client Protocol, so their client is
//! shared and most of the translation is the identity function. This one does
//! not. It serves three methods — `initialize`, `session/prompt`, `shutdown` —
//! and notifies a stream of its session log: `{seq, type, time, data}`, one
//! append-only record per thing that happened.
//!
//! Turning that into the contract's vocabulary is this file's whole job, and it
//! is why the contract is worth having. Nothing above Layer 3 learns that a
//! third protocol exists.
//!
//! ## What is carried, and what is deliberately dropped
//!
//! The log is richer than the contract, on purpose — it is the harness's own
//! durable record, including its request headers, its sandbox policy, and the
//! chunk-level replay of every stream. What crosses is what the contract has a
//! word for: messages, tool calls, turns and usage. Everything else is skipped
//! rather than approximated, and skipped silently rather than warned about: a
//! newer harness adding a record type must not fill a session's log with
//! complaints.
//!
//! ## Turn identity
//!
//! The prompt call returns as soon as the message is queued, not when the turn
//! ends — so unlike ACP, the answer to "did the turn finish" arrives on the
//! notification stream as `turn/end`. That is what closes the turn here.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::DshSandboxSpec;
use crate::supervisor;
use crate::{
    AdapterError, ContentBlock, ContentChunk, ConversationChannel, ConversationInput,
    ConversationInputSender, ConversationUpdate, PromptResponse, SessionUpdate, ToolCall,
    ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, TurnStopReason, UsageUpdate,
};

/// One operation may wait behind the writer, but a second is a backed-up link.
const INPUT_BUFFER: usize = 1;

/// How long to wait for the pod to say what it is.
const DECLARATION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long to wait for an answer to a request of ours.
///
/// Short, unlike the ACP client's: every method this wire offers returns
/// immediately. A prompt is acknowledged when it is queued, and the turn it
/// opens is reported on the notification stream instead.
const CALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// The socket half this client writes to.
type Sink = SplitSink<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, Message>;

/// A JSON-RPC call in flight, waiting for the harness to answer it.
type Pending =
    Arc<Mutex<std::collections::HashMap<i64, tokio::sync::oneshot::Sender<serde_json::Value>>>>;

/// The client's side of a conversation with this harness.
pub(super) struct Harness {
    sink: Arc<Mutex<Sink>>,
    pending: Pending,
    next_id: AtomicI64,
    /// The conversation's identifier on this wire.
    ///
    /// Chosen here rather than returned by the harness: its only session call
    /// takes an id and creates one under it. That is also what makes rejoining
    /// possible where resuming is not — the harness keys a live session on the
    /// id it was given, so a control plane that comes back with the same one
    /// finds the conversation still running in the process it left. What it
    /// cannot do is take one back off disk, which is why this kernel carries no
    /// conversation across pods.
    session: String,
    /// The running read of this harness's session log.
    ///
    /// Held rather than folded statelessly because one record's meaning
    /// depends on an earlier one: usage arrives without the size of the window
    /// it was drawn from, and the contract wants both together.
    folder: Arc<Mutex<Folder>>,
}

impl Harness {
    /// Open a conversation with the harness behind this socket.
    pub(super) async fn open(
        stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        cwd: String,
        spec: &DshSandboxSpec,
        rejoin: Option<String>,
        link: crate::hosted::LinkLifecycle,
        // Supplied by the caller and already being drained. This harness has
        // no sign-in of its own, but the contract is one contract: a kernel
        // that reported nothing during its open would be indistinguishable
        // from one whose open had not started.
        updates_tx: mpsc::Sender<crate::ConversationUpdate>,
    ) -> Result<(ConversationChannel, mpsc::Sender<crate::TerminalInput>), AdapterError> {
        let (sink, mut source) = stream.split();

        // The pod's declaration, read before anything else. A pod that
        // predates lanes never reaches here: the conversation lane is asked for
        // by name and an older pod does not serve one.
        match tokio::time::timeout(DECLARATION_DEADLINE, source.next()).await {
            Ok(Some(Ok(Message::Text(text))))
                if supervisor::Ready::from_control(&text).is_some() => {}
            _ => {
                return Err(AdapterError::Unavailable(
                    "this runtime did not declare a conversation lane".to_string(),
                ));
            }
        }

        let harness = Arc::new(Self {
            sink: Arc::new(Mutex::new(sink)),
            pending: Arc::new(Mutex::new(std::collections::HashMap::new())),
            next_id: AtomicI64::new(1),
            // A name to rejoin when a control plane is taking a live runtime
            // back over, and a fresh one otherwise. Reusing the name is the
            // whole mechanism: the harness hands back the session it already
            // holds under it rather than opening a second one beside it.
            session: rejoin.unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()),
            folder: Arc::new(Mutex::new(Folder::default())),
        });
        let control = crate::hosted::control_lane(Arc::clone(&harness.sink));
        link.register(control.clone()).await;

        let writer = Arc::clone(&harness);
        let (input, input_task) =
            ConversationInputSender::spawn_with_abort(INPUT_BUFFER, move |input| {
                let writer = Arc::clone(&writer);
                async move { writer.drive(input).await }
            });

        // Read before anything is sent: the handshake's own answer arrives on
        // this loop, so it has to be running to receive it.
        let reader = Arc::clone(&harness);
        let updates = updates_tx.clone();
        let control_for_reader = control.clone();
        let link_for_reader = link.clone();
        tokio::spawn(async move {
            while let Some(frame) = source.next().await {
                match frame {
                    Ok(Message::Binary(bytes)) => match supervisor::unframe(&bytes) {
                        Some((supervisor::LANE_CONVERSATION, payload)) => {
                            reader.receive(payload, &updates).await;
                        }
                        Some((supervisor::LANE_CONVERSATION_GAP, payload)) => {
                            let frames = serde_json::from_slice::<serde_json::Value>(payload)
                                .ok()
                                .and_then(|v| v.get("frames").and_then(serde_json::Value::as_u64))
                                .unwrap_or(0);
                            let _ = updates.send(ConversationUpdate::Lost { frames }).await;
                        }
                        _ => {}
                    },
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "conversation link read failed");
                        break;
                    }
                }
            }
            reader.pending.lock().await.clear();
            link_for_reader.forget(&control_for_reader).await;
            input_task.abort();
            tracing::info!("conversation link ended");
        });

        if let Err(error) = harness.handshake(cwd, spec).await {
            link.forget(&control).await;
            return Err(error);
        }

        let conversation_id = harness.session.clone();

        Ok((
            ConversationChannel {
                input,
                conversation_id: Some(conversation_id),
            },
            control,
        ))
    }

    /// Tell the harness which route and model this session runs on.
    ///
    /// The route is the one the image's profile declares. Naming a different
    /// one fails here rather than at the first turn, which is the difference
    /// between a session that will not start and one that looks started.
    async fn handshake(&self, cwd: String, spec: &DshSandboxSpec) -> Result<(), AdapterError> {
        self.call(
            "initialize",
            serde_json::json!({
                "cwd": cwd,
                "provider": spec.provider,
                "model": spec.deployment.upstream_model,
            }),
        )
        .await?;
        tracing::info!(session = %self.session, "conversation opened");
        Ok(())
    }

    /// Act on something a caller sent toward the runtime.
    async fn drive(&self, input: ConversationInput) -> Result<(), AdapterError> {
        match input {
            ConversationInput::Prompt(content) => {
                let params = serde_json::json!({
                    "sessionId": self.session,
                    "contentBlocks": content,
                });
                let waiting = self.begin("session/prompt", params).await?;
                tokio::spawn(async move {
                    if let Err(error) = Self::await_call("session/prompt", waiting).await {
                        tracing::warn!(%error, "the harness did not accept the prompt");
                    }
                });
                Ok(())
            }
            // Neither is reachable: this kernel declares no interrupt and no
            // permission channel, so the core refuses both by name before they
            // get here. Logged rather than dropped, because arriving anyway
            // would mean a capability check went missing upstream.
            ConversationInput::Cancel => Err(AdapterError::Unsupported(
                "this kernel cannot interrupt a turn".to_string(),
            )),
            ConversationInput::PermissionAnswer { .. } => Err(AdapterError::Unsupported(
                "this kernel asks for no permission".to_string(),
            )),
        }
    }

    /// Act on one frame the harness sent.
    async fn receive(&self, payload: &[u8], updates: &mpsc::Sender<ConversationUpdate>) {
        let Ok(frame) = serde_json::from_slice::<serde_json::Value>(payload) else {
            // Unreadable rather than unfamiliar. Logged without its content: a
            // conversation frame carries what somebody said to their agent.
            tracing::warn!(bytes = payload.len(), "a conversation frame was not json");
            return;
        };

        // An answer to something we asked.
        if let Some(id) = frame.get("id").and_then(serde_json::Value::as_i64) {
            if frame.get("method").is_none() {
                let waiting = self.pending.lock().await.remove(&id);
                if let Some(waiting) = waiting {
                    let _ = waiting.send(frame);
                }
                return;
            }
        }

        if frame.get("method").and_then(serde_json::Value::as_str) != Some("session.event") {
            // `session.status` and the subagent notices say nothing the
            // contract has a word for. Skipped rather than approximated.
            return;
        }
        let Some(event) = frame.pointer("/params/event") else {
            return;
        };
        for update in self.folder.lock().await.read(event) {
            if updates.send(update).await.is_err() {
                break;
            }
        }
    }

    /// Ask the harness something and wait for its answer.
    async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AdapterError> {
        let waiting = self.begin(method, params).await?;
        Self::await_call(method, waiting).await
    }

    /// Write a request and return the receiver that will carry its answer.
    async fn begin(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<tokio::sync::oneshot::Receiver<serde_json::Value>, AdapterError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (answer, waiting) = tokio::sync::oneshot::channel();
        self.pending.lock().await.insert(id, answer);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(e) = self.send(&request).await {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }
        Ok(waiting)
    }

    /// Wait for and classify the answer to one request already on the wire.
    async fn await_call(
        method: &str,
        waiting: tokio::sync::oneshot::Receiver<serde_json::Value>,
    ) -> Result<serde_json::Value, AdapterError> {
        let frame = tokio::time::timeout(CALL_DEADLINE, waiting)
            .await
            .map_err(|_| AdapterError::Unavailable(format!("{method} was never answered")))?
            .map_err(|_| AdapterError::Unavailable(format!("{method} lost its answer")))?;
        if let Some(error) = frame.get("error") {
            return Err(AdapterError::Rejected(format!("{method}: {error}")));
        }
        Ok(frame
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    /// Send one frame on the conversation lane.
    async fn send(&self, frame: &serde_json::Value) -> Result<(), AdapterError> {
        let body = frame.to_string();
        let framed = supervisor::frame(supervisor::LANE_CONVERSATION, body.as_bytes());
        self.sink
            .lock()
            .await
            .send(Message::Binary(framed))
            .await
            .map_err(|e| AdapterError::Unavailable(format!("conversation write failed: {e}")))
    }
}

/// The running read of one session's log.
///
/// A record's meaning can depend on an earlier one, so this carries the little
/// that must be remembered between them and nothing else.
#[derive(Debug, Default)]
pub(super) struct Folder {
    /// The context window the current route advertises, once it has said.
    ///
    /// Usage arrives without it — the harness reports what a step consumed,
    /// and how large the window is comes separately, when the route changes.
    window: u64,
}

impl Folder {
    /// Read one session-log record as the contract's own vocabulary.
    ///
    /// A record may say nothing the contract has a word for, or two things at
    /// once — an assembled message carries its own token accounting — so this
    /// yields a list rather than an option.
    pub(super) fn read(&mut self, event: &serde_json::Value) -> Vec<ConversationUpdate> {
        let kind = event.get("type").and_then(serde_json::Value::as_str);
        let data = event.get("data").unwrap_or(&serde_json::Value::Null);
        match kind {
            Some("assistant/chunk") => text_delta(data)
                .map(|text| {
                    vec![ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                        ContentChunk::new(ContentBlock::from(text)),
                    ))]
                })
                .unwrap_or_default(),
            Some("assistant/message") => {
                let mut folded = Vec::new();
                if let Some(usage) = data.get("usage") {
                    let window = self.window;
                    let used = usage
                        .get("inputTokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                        + usage
                            .get("outputTokens")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                    folded.push(ConversationUpdate::Said(SessionUpdate::UsageUpdate(
                        UsageUpdate::new(used, window),
                    )));
                }
                folded
            }
            Some("tool/call") => {
                let (Some(id), Some(name)) = (
                    data.get("callId").and_then(serde_json::Value::as_str),
                    data.get("name").and_then(serde_json::Value::as_str),
                ) else {
                    return Vec::new();
                };
                vec![ConversationUpdate::Said(SessionUpdate::ToolCall(
                    ToolCall::new(ToolCallId::new(id.to_string()), name.to_string()),
                ))]
            }
            Some("tool/result") => {
                let Some(id) = call_id_of(data) else {
                    return Vec::new();
                };
                // Failure is carried as a status rather than as prose: the
                // contract has a word for it and every interface renders it.
                let status = match data.get("error").is_some() {
                    true => ToolCallStatus::Failed,
                    false => ToolCallStatus::Completed,
                };
                let mut fields = ToolCallUpdateFields::default();
                fields.status = Some(status);
                vec![ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(
                    ToolCallUpdate::new(ToolCallId::new(id), fields),
                ))]
            }
            Some("request/context") => {
                if let Some(window) = data
                    .get("contextWindow")
                    .and_then(serde_json::Value::as_u64)
                {
                    self.window = window;
                }
                Vec::new()
            }
            Some("turn/end") => {
                vec![ConversationUpdate::TurnEnded(PromptResponse::new(
                    stop_reason(data),
                ))]
            }
            // Everything else is the harness's own record-keeping — request
            // headers, sandbox policy, inbox splices, titles. Skipped, and
            // skipped quietly: a newer harness adding a record must not fill a
            // session's log with complaints.
            _ => Vec::new(),
        }
    }
}

/// The text a streamed chunk added, if it added any.
///
/// The harness streams block starts, ends, usage and a finish marker down the
/// same channel as text. Only the text is liveness the contract has a word for.
fn text_delta(data: &serde_json::Value) -> Option<String> {
    let chunk = data.get("chunk")?;
    if chunk.get("type").and_then(serde_json::Value::as_str)? != "text-delta" {
        return None;
    }
    Some(chunk.get("text")?.as_str()?.to_string())
}

/// The call a tool result belongs to.
///
/// Carried inside the result message rather than beside it, which is where the
/// harness puts it.
fn call_id_of(data: &serde_json::Value) -> Option<String> {
    data.get("message")?
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|block| block.get("id")?.as_str().map(str::to_string))
}

/// Why the turn ended, in the contract's words.
///
/// The harness's reasons are richer than the contract's and several collapse
/// onto the same one. Where they do, the safer reading wins: a turn that
/// stopped because something went wrong is not reported as one that finished.
fn stop_reason(data: &serde_json::Value) -> TurnStopReason {
    match data
        .get("reason")
        .and_then(|r| r.get("kind"))
        .and_then(serde_json::Value::as_str)
    {
        Some("completed") => TurnStopReason::EndTurn,
        Some("max-tokens") => TurnStopReason::MaxTokens,
        // A turn a persistence backend closed on reload, and one a
        // cancellation stopped, are both turns that did not finish.
        Some("aborted") | Some("interrupted") => TurnStopReason::Cancelled,
        // Blocked, errored, and anything a newer harness invents. Refused is
        // the reading that cannot overstate what happened.
        _ => TurnStopReason::Refusal,
    }
}

#[cfg(test)]
#[path = "translate_tests.rs"]
mod translate_tests;
