//! Driving this kernel's bridge over the Agent Client Protocol.
//!
//! A bridge in a pod is an ACP agent, whichever kernel is behind it. This is
//! the client half: it opens a session, prompts it, answers what it asks, and
//! turns everything it says into the contract's own vocabulary. Because the
//! contract *is* ACP, most of that translation is the identity function — what
//! is left is the handshake, the correlation of requests with answers, and the
//! deadlines that keep a silent bridge from stranding a session.
//!
//! Nothing here names a kernel. What differs between them is the command the
//! pod runs and the name of the mode a session is held to, and both arrive as
//! arguments — from the kernel's own directory, which is the only place either
//! is written down.
//!
//! ## What is deliberately not filtered
//!
//! Bridges relay their kernel's own startup warnings as assistant message
//! chunks — a model-metadata notice arrives looking exactly like something the
//! agent said. Nothing here strips it: this layer must not decide which of a
//! kernel's words are worth keeping, and a filter written against one release
//! is a filter that silently eats real output in the next.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::supervisor;
use crate::{
    AdapterError, ConversationChannel, ConversationInput, ConversationUpdate, PromptResponse,
};

use super::inbound::outcome_for;

/// The protocol version this client speaks.
const PROTOCOL_VERSION: u32 = 1;

/// How many messages either direction may queue before back pressure applies.
const BUFFER: usize = 256;

/// How long to wait for the pod to say what it is.
///
/// Answered immediately by any pod that serves a conversation lane at all, so
/// this is a bound on a failure rather than on the ordinary case.
const DECLARATION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long to wait for the bridge to answer a request of ours.
///
/// Generous, because a prompt turn is one of these and a turn takes as long as
/// the model takes. Bounded all the same: without it, a bridge that stops
/// answering leaves a session waiting for something that is not coming, with
/// nothing said about why.
const CALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(1800);

/// The socket half this client writes to.
type Sink = SplitSink<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, Message>;

/// A JSON-RPC call in flight, waiting for the bridge to answer it.
type Pending = Arc<Mutex<HashMap<i64, tokio::sync::oneshot::Sender<serde_json::Value>>>>;

/// The client's side of a conversation with this kernel's bridge.
pub struct Bridge {
    sink: Arc<Mutex<Sink>>,
    pub(super) pending: Pending,
    next_id: AtomicI64,
    /// The bridge's own identifier for this conversation, once one exists.
    session: Mutex<Option<String>>,
    /// Permission requests the bridge is blocked on, by the id we gave them.
    ///
    /// Held because the answer comes back minutes later, from a person, and
    /// has to be matched to the JSON-RPC request that is still waiting — and
    /// to the options that request offered, which only it knows.
    pub(super) blocked: Mutex<HashMap<String, Blocked>>,
}

/// A permission request the bridge is waiting on an answer to.
pub(super) struct Blocked {
    /// The JSON-RPC id to answer.
    pub(super) call: serde_json::Value,
    /// The options the bridge offered, which an intent is rendered into.
    pub(super) options: Vec<crate::PermissionOption>,
}

impl Bridge {
    /// Open a conversation with the bridge behind this socket.
    ///
    /// Everything the handshake needs happens before the channel is handed
    /// back, so a caller that receives one has a conversation that is ready to
    /// be prompted rather than one that may yet fail to start.
    pub async fn open(
        stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        cwd: String,
        mode: Option<String>,
    ) -> Result<ConversationChannel, AdapterError> {
        let (sink, mut source) = stream.split();

        // The pod's own declaration, read before anything else, because it
        // says whether this conversation is being taken over from a runtime
        // that has been replaced. A pod that predates lanes never reaches
        // here: the conversation lane is asked for by name and an older pod
        // does not serve one.
        let resuming = match tokio::time::timeout(DECLARATION_DEADLINE, source.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                supervisor::Ready::from_control(&text)
                    .ok_or_else(|| {
                        AdapterError::Unavailable(
                            "this runtime said something other than what it is".to_string(),
                        )
                    })?
                    .resuming
            }
            _ => {
                return Err(AdapterError::Unavailable(
                    "this runtime did not declare a conversation lane".to_string(),
                ))
            }
        };
        let (updates_tx, updates_rx) = mpsc::channel(BUFFER);
        let (input_tx, mut input_rx) = mpsc::channel(BUFFER);

        let bridge = Arc::new(Self {
            sink: Arc::new(Mutex::new(sink)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicI64::new(1),
            session: Mutex::new(None),
            blocked: Mutex::new(HashMap::new()),
        });

        // Read before anything is sent: the handshake's own answers arrive on
        // this loop, so it has to be running to receive them.
        let reader = Arc::clone(&bridge);
        let updates = updates_tx.clone();
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
            tracing::info!("conversation link ended");
        });

        bridge.handshake(resuming, cwd, mode).await?;

        let writer = Arc::clone(&bridge);
        let updates = updates_tx;
        tokio::spawn(async move {
            while let Some(input) = input_rx.recv().await {
                writer.drive(input, &updates).await;
            }
            tracing::info!("conversation input ended");
        });

        Ok(ConversationChannel {
            updates: updates_rx,
            input: input_tx,
        })
    }

    /// Initialize, take or resume a session, and settle its mode.
    async fn handshake(
        &self,
        resuming: Option<String>,
        cwd: String,
        mode: Option<String>,
    ) -> Result<(), AdapterError> {
        self.call(
            "initialize",
            serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } },
            }),
        )
        .await?;

        let session = match resuming {
            // Carried from another pod. The rollout is already on this pod's
            // disk — the supervisor put it there — so loading by identifier is
            // what makes the session continue rather than start again.
            Some(conversation) => {
                let params = serde_json::json!({
                    "sessionId": conversation,
                    "cwd": cwd,
                    "mcpServers": [],
                });
                self.call("session/load", params).await.map_err(|e| {
                    // Loud: the runtime this conversation came from is about to
                    // be torn down, and reporting success here would lose it.
                    AdapterError::Rejected(format!("could not resume conversation: {e}"))
                })?;
                conversation
            }
            None => {
                let started = self
                    .call(
                        "session/new",
                        serde_json::json!({ "cwd": cwd, "mcpServers": [] }),
                    )
                    .await?;
                started
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        AdapterError::Rejected("the bridge named no session".to_string())
                    })?
                    .to_string()
            }
        };
        *self.session.lock().await = Some(session.clone());

        // Refused rather than defaulted, for the bridges that have modes: they
        // start every session in their most permissive one whatever the
        // kernel's own configuration says, so a session whose mode did not
        // take would be acting unattended while its owner had asked to be
        // asked. A bridge with no modes names none and this is skipped.
        if let Some(mode) = mode {
            self.call(
                "session/set_mode",
                serde_json::json!({ "sessionId": session, "modeId": mode }),
            )
            .await
            .map_err(|e| {
                AdapterError::Rejected(format!(
                    "this session could not be held to its permission mode ({mode}): {e}"
                ))
            })?;
            tracing::info!(%session, %mode, "conversation opened");
        } else {
            tracing::info!(%session, "conversation opened");
        }
        Ok(())
    }

    /// Act on something a caller sent toward the runtime.
    async fn drive(&self, input: ConversationInput, updates: &mpsc::Sender<ConversationUpdate>) {
        let Some(session) = self.session.lock().await.clone() else {
            return;
        };
        match input {
            ConversationInput::Prompt(content) => {
                // The turn's answer is the turn ending, so it is awaited on its
                // own task: a prompt takes as long as the model takes, and
                // blocking here would stop the caller cancelling it.
                let params = serde_json::json!({ "sessionId": session, "prompt": content });
                let updates = updates.clone();
                match self.begin(params) {
                    Ok(waiting) => {
                        tokio::spawn(async move {
                            let ended = match waiting.await {
                                Ok(result) => serde_json::from_value::<PromptResponse>(result)
                                    .unwrap_or_else(|_| {
                                        PromptResponse::new(crate::TurnStopReason::EndTurn)
                                    }),
                                // The bridge went away mid-turn. Reported as a
                                // cancellation rather than as an ending: the
                                // turn did not finish, and the core closes it
                                // with the reason the conversation stopped for.
                                Err(_) => PromptResponse::new(crate::TurnStopReason::Cancelled),
                            };
                            let _ = updates.send(ConversationUpdate::TurnEnded(ended)).await;
                        });
                    }
                    Err(e) => tracing::warn!(error = %e, "could not send a prompt"),
                }
            }
            ConversationInput::PermissionAnswer { id, decision } => {
                let held = self.blocked.lock().await.remove(&id);
                let Some(held) = held else {
                    // Answered after the bridge stopped waiting — a turn that
                    // was cancelled, or a runtime that went away. Nothing to
                    // send, and saying so beats a frame nothing will read.
                    tracing::info!(request = %id, "nothing is waiting for this answer any more");
                    return;
                };
                let outcome = outcome_for(decision, &held.options);
                let answer = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": held.call,
                    "result": { "outcome": outcome },
                });
                if let Err(e) = self.send(&answer).await {
                    tracing::warn!(error = %e, "could not answer a permission request");
                }
            }
            ConversationInput::Cancel => {
                let notice = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/cancel",
                    "params": { "sessionId": session },
                });
                if let Err(e) = self.send(&notice).await {
                    tracing::warn!(error = %e, "could not cancel the turn");
                }
            }
        }
    }

    /// Ask the bridge something and wait for its answer.
    async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AdapterError> {
        let waiting = self.begin(serde_json::json!({ "method": method, "params": params }))?;
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

    /// Send a request and hand back what will hold its answer.
    ///
    /// Registered before it is sent, because the answer can arrive on the read
    /// loop before this function returns.
    fn begin(
        &self,
        request: serde_json::Value,
    ) -> Result<tokio::sync::oneshot::Receiver<serde_json::Value>, AdapterError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (answer, waiting) = tokio::sync::oneshot::channel();
        let mut framed = request;
        framed["jsonrpc"] = serde_json::Value::String("2.0".to_string());
        framed["id"] = serde_json::Value::from(id);

        let pending = Arc::clone(&self.pending);
        let sink = Arc::clone(&self.sink);
        tokio::spawn(async move {
            pending.lock().await.insert(id, answer);
            let body = framed.to_string();
            let frame = supervisor::frame(supervisor::LANE_CONVERSATION, body.as_bytes());
            if sink
                .lock()
                .await
                .send(Message::Binary(frame))
                .await
                .is_err()
            {
                // Dropped from the map so the caller's wait ends now rather
                // than at the deadline: the link is gone and no answer is
                // coming.
                pending.lock().await.remove(&id);
            }
        });
        Ok(waiting)
    }

    /// Send one frame, expecting no answer.
    pub(super) async fn send(&self, frame: &serde_json::Value) -> Result<(), AdapterError> {
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

#[cfg(test)]
#[path = "acp_tests.rs"]
mod acp_tests;
