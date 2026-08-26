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
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;

use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::supervisor;
use crate::task_retirement::TaskRetirement;
use crate::{
    AdapterError, ConversationChannel, ConversationInputSender, ConversationUpdate, PromptResponse,
    TerminalInputSender,
};

/// The protocol version this client speaks.
pub(super) const PROTOCOL_VERSION: u32 = 1;

/// One operation may wait behind the writer, but a second is a backed-up link.
const INPUT_BUFFER: usize = 1;

/// How long to wait for the pod to say what it is.
///
/// Answered immediately by any pod that serves a conversation lane at all, so
/// this is a bound on a failure rather than on the ordinary case.
const DECLARATION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long to wait for the bridge to answer a question of ours.
///
/// Short, because every call that uses it is a question with a quick answer:
/// initialising, naming a conversation, setting a mode. It was thirty minutes
/// — a bound sized for a prompt, which no longer travels this path — and that
/// is how a pod whose agent never started held a session at `Attaching` for
/// half an hour with nothing said. A handshake that has not answered in this
/// long is not slow; it is not coming.
pub(super) const CALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a turn may take before it is declared lost.
///
/// Generous, because a turn takes as long as the model takes, and a person
/// watching an agent think must not have it cut off underneath them. Bounded
/// all the same: a prompt had no deadline at all, so an agent that stopped
/// answering left the turn `running` forever — indistinguishable, from the
/// outside, from one still working.
pub(super) const TURN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3600);

/// How long a sign-in may take.
///
/// Human-paced, not machine-paced: somebody has to open a page on another
/// device, sign in, and type a code. The ordinary call deadline is thirty
/// seconds and this timed out against it every time — the sign-in was working
/// and the wait was not.
///
/// A little longer than the code itself lives, so a sign-in that runs out is
/// reported by the kernel in its own words rather than by us giving up first.
/// Which of the two spoke is the difference between "your code expired" and
/// "something went wrong".
pub(super) const SIGN_IN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(16 * 60);

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
    pub(super) session: Mutex<Option<String>>,
    /// Permission requests the bridge is blocked on, by the id we gave them.
    ///
    /// Held because the answer comes back minutes later, from a person, and
    /// has to be matched to the JSON-RPC request that is still waiting — and
    /// to the options that request offered, which only it knows.
    pub(super) blocked: Mutex<HashMap<String, Blocked>>,
    /// Answers accepted in the active turn, including ones restored from a
    /// durable locator rather than observed on this socket.
    ///
    /// The whole response is retained so a duplicate delivery of the same
    /// JSON-RPC request can receive the identical answer without asking its
    /// owner a second time. Cleared at the prompt's terminal response so a
    /// later turn may safely reuse a JSON-RPC correlation id.
    pub(super) answered: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    /// How many prompt waiters this Bridge currently owns.
    ///
    /// More than one may exist while a runtime accepts steering. Counting them
    /// keeps one prompt's completion from making a later live permission request
    /// look like restart recovery merely because another prompt is still active.
    pub(super) owned_prompt_waiters: Arc<AtomicUsize>,
    /// A prompt begun on a replaced control plane whose terminal response no
    /// longer has a process-local waiter.
    ///
    /// Set only when a permission route had to be reconstructed from durable
    /// state. The next unmatched JSON-RPC response is then the blocked prompt
    /// continuing to its terminal response, and must reach the core instead of
    /// being discarded with the old connection's waiter.
    pub(super) orphaned_prompt_response: AtomicBool,
}

/// A permission request the bridge is waiting on an answer to.
pub(super) struct Blocked {
    /// The JSON-RPC id to answer.
    pub(super) call: serde_json::Value,
    /// The options the bridge offered, which an intent is rendered into.
    pub(super) options: Vec<crate::PermissionOption>,
    /// Whether a prompt begun on this Bridge owns the eventual terminal answer.
    ///
    /// Captured when the request arrives. A runtime can replay a blocked request
    /// onto a replacement socket, so presence in the live map alone does not
    /// prove that this process owns the corresponding prompt waiter.
    pub(super) owns_prompt_waiter: bool,
}

impl Bridge {
    /// Open a conversation with the bridge behind this socket.
    ///
    /// Everything the handshake needs happens before the channel is handed
    /// back, so a caller that receives one has a conversation that is ready to
    /// be prompted rather than one that may yet fail to start.
    /// Returns the conversation and the control sender for the same socket.
    ///
    /// The update sink comes from the caller for that same reason read the
    /// other way round. The handshake can stop and wait — an own-account
    /// session waits there for its owner to sign in, which is a person's worth
    /// of time — and what it is waiting for has to be shown to that person
    /// while it waits. A sink created here and returned at the end would carry
    /// the request only after it had been answered, which is to say never.
    ///
    /// The second half is not optional bookkeeping. A credential reaches a
    /// runtime as a control frame on a link the adapter has recorded, and
    /// until now the only recorded links were terminal attachments — so a
    /// conversation session could never be given its owner's credential, and
    /// on a conversation-only kernel there was no other path at all.
    pub async fn open(
        stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        cwd: String,
        mode: Option<String>,
        rejoin: Option<String>,
        // Where this link's answers to a plane switch are delivered, and the
        // runtime they are about. A conversation link is the only link a
        // conversation-only kernel has, so without this such a session could
        // never confirm a switch and every one of them would rotate (#289).
        link: Option<super::LinkLifecycle>,
        // Whether this session spends its owner's own account with the model
        // vendor. It decides whether the agent has to be signed in before a
        // conversation can be opened at all — an unauthenticated kernel
        // refuses `session/new` outright.
        own_credential: bool,
        // Already being drained by the caller when this is called. See above.
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, TerminalInputSender), AdapterError> {
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
        let bridge = Arc::new(Self {
            sink: Arc::new(Mutex::new(sink)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicI64::new(1),
            session: Mutex::new(None),
            blocked: Mutex::new(HashMap::new()),
            answered: Arc::new(Mutex::new(HashMap::new())),
            owned_prompt_waiters: Arc::new(AtomicUsize::new(0)),
            orphaned_prompt_response: AtomicBool::new(false),
        });
        // This sender's channel identity is the generation of this socket.
        // Keep it beside the reader so a late answer from this socket cannot
        // settle a waiter installed by a replacement connection.
        let control = super::control_lane(Arc::clone(&bridge.sink), link.clone());
        let mut abort_on_drop = super::AbortLinkOnDrop::new(control.clone());

        let writer = Arc::clone(&bridge);
        let writer_updates = updates_tx.clone();
        let input = ConversationInputSender::spawn(INPUT_BUFFER, move |input| {
            let writer = Arc::clone(&writer);
            let updates = writer_updates.clone();
            async move { writer.drive(input, &updates).await }
        });

        // Read before anything is sent: the handshake's own answers arrive on
        // this loop, so it has to be running to receive them.
        let reader = Arc::clone(&bridge);
        let updates = updates_tx.clone();
        let control_for_reader = control.downgrade();
        let link_for_reader = link.clone();
        let reader_task = tokio::spawn(async move {
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
                    Ok(Message::Text(text)) => {
                        // Runtime control answers; the conversation itself
                        // travels framed on its own lane.
                        if let Some(lifecycle) = link_for_reader.as_ref() {
                            if let Some((operation_id, installed)) =
                                super::link::credential_answer(&text)
                            {
                                if let Some(control) = control_for_reader.upgrade() {
                                    lifecycle
                                        .settle_credential(&control, &operation_id, installed)
                                        .await;
                                }
                            } else if let Some(accepted) = super::link::switch_answer(&text) {
                                if let Some(control) = control_for_reader.upgrade() {
                                    lifecycle.settle(&control, accepted).await;
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "conversation link read failed");
                        break;
                    }
                }
            }
        });

        let whole_retirement = TaskRetirement::default();
        let whole_guard = whole_retirement.guard();
        let (installed, installation) = if link.is_some() {
            let (installed, installation) = tokio::sync::oneshot::channel();
            (Some(installed), Some(installation))
        } else {
            (None, None)
        };
        let cleanup_bridge = Arc::clone(&bridge);
        let input_retired = input.retirement();
        let input_abort = input.abort_handle();
        let control_retired = control.retirement();
        let control_abort = control.abort_handle();
        let control_for_cleanup = control.downgrade();
        let link_for_cleanup = link.clone();
        let cleanup_task = tokio::spawn(async move {
            let _whole_guard = whole_guard;
            let mut reader = reader_task;
            let input_ended = input_retired.clone();
            let control_ended = control_retired.clone();
            tokio::select! {
                _ = input_ended.wait() => {
                    reader.abort();
                    let _ = reader.await;
                    control_abort.abort();
                    control_retired.wait().await;
                }
                _ = control_ended.wait() => {
                    reader.abort();
                    let _ = reader.await;
                    input_abort.abort();
                    input_retired.wait().await;
                }
                _ = &mut reader => {
                    input_abort.abort();
                    control_abort.abort();
                    input_retired.wait().await;
                    control_retired.wait().await;
                }
            }
            // Dropping every waiter makes a dead link an immediate answer.
            cleanup_bridge.pending.lock().await.clear();
            if let (Some(lifecycle), Some(installation)) = (link_for_cleanup.as_ref(), installation)
            {
                if installation.await.is_ok() {
                    if let Some(control) = control_for_cleanup.upgrade() {
                        lifecycle.forget(&control).await;
                    }
                }
            }
            tracing::info!("conversation link ended");
        });

        if let Some(lifecycle) = link.as_ref() {
            if !lifecycle
                .register(
                    control.clone(),
                    whole_retirement.clone(),
                    installed.expect("a registered link has an installation barrier"),
                )
                .await
            {
                control.abort();
                let _ = cleanup_task.await;
                let _ = bridge.sink.lock().await.close().await;
                return Err(AdapterError::Unavailable(
                    "the conversation link was superseded while opening".into(),
                ));
            }
        }

        // A conversation to rejoin wins over one the pod says it was handed.
        // They answer different questions — one is a control plane taking back
        // over a live runtime, the other a runtime taking over a session — and
        // when both are present the first is the more recent truth.
        let handshake = bridge
            .handshake(rejoin, resuming, cwd, mode, own_credential)
            .await;
        if let Err(error) = handshake {
            // The runtime may reject the handshake while deliberately keeping
            // its socket open. Stop every detached owner before returning, or
            // repeated readoption attempts accumulate sockets and relay tasks.
            control.abort();
            let _ = cleanup_task.await;
            let _ = bridge.sink.lock().await.close().await;
            return Err(error);
        }
        let conversation_id = bridge.session.lock().await.clone();
        if let Some(lifecycle) = link.as_ref() {
            if !lifecycle.is_current(&control).await {
                control.abort();
                let _ = cleanup_task.await;
                let _ = bridge.sink.lock().await.close().await;
                return Err(AdapterError::Unavailable(
                    "the conversation link ended or was superseded while opening".into(),
                ));
            }
        }

        abort_on_drop.disarm();
        Ok((
            ConversationChannel {
                input,
                conversation_id,
            },
            control,
        ))
    }

    /// Ask the bridge something and wait for its answer.
    pub(super) async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AdapterError> {
        self.call_within(method, params, CALL_DEADLINE).await
    }

    /// The same, for something that legitimately takes longer than a call.
    pub(super) async fn call_within(
        &self,
        method: &str,
        params: serde_json::Value,
        deadline: std::time::Duration,
    ) -> Result<serde_json::Value, AdapterError> {
        let waiting = self.begin(method, params).await?;
        let frame = tokio::time::timeout(deadline, waiting)
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
    /// Takes the method rather than accepting a pre-built object, so a request
    /// without one cannot be constructed. It could before, and one was: the
    /// prompt passed its parameters straight in, and what went on the wire was
    /// a JSON-RPC request carrying an id and no method — undispatchable, so
    /// the agent was never asked anything and every conversation turn opened
    /// and hung.
    ///
    /// Registered before it is sent, because the answer can arrive on the read
    /// loop before this function returns.
    pub(super) async fn begin(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<tokio::sync::oneshot::Receiver<serde_json::Value>, AdapterError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (answer, waiting) = tokio::sync::oneshot::channel();
        let framed = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        self.pending.lock().await.insert(id, answer);
        if let Err(error) = self.send(&framed).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
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

/// How a turn came out: finished, or stopped by something going wrong.
///
/// Kept apart from `PromptResponse` because the protocol has no word for
/// "the agent could not answer at all", and folding that into a stop reason
/// is what made every such turn read as an ordinary, empty completion.
pub(super) enum Outcome {
    Ended(PromptResponse),
    Failed {
        message: String,
        capability: Option<String>,
    },
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod acp_tests;
