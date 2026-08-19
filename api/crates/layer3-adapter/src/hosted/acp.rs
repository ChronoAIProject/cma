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

/// How long to wait for the bridge to answer a question of ours.
///
/// Short, because every call that uses it is a question with a quick answer:
/// initialising, naming a conversation, setting a mode. It was thirty minutes
/// — a bound sized for a prompt, which no longer travels this path — and that
/// is how a pod whose agent never started held a session at `Attaching` for
/// half an hour with nothing said. A handshake that has not answered in this
/// long is not slow; it is not coming.
const CALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a turn may take before it is declared lost.
///
/// Generous, because a turn takes as long as the model takes, and a person
/// watching an agent think must not have it cut off underneath them. Bounded
/// all the same: a prompt had no deadline at all, so an agent that stopped
/// answering left the turn `running` forever — indistinguishable, from the
/// outside, from one still working.
const TURN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3600);

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
    /// Returns the conversation and the control sender for the same socket.
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
        switches: Option<(super::SwitchWaiters, String)>,
        // Whether this session spends its owner's own account with the model
        // vendor. It decides whether the agent has to be signed in before a
        // conversation can be opened at all — an unauthenticated kernel
        // refuses `session/new` outright.
        own_credential: bool,
    ) -> Result<(ConversationChannel, mpsc::Sender<crate::TerminalInput>), AdapterError> {
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
                    Ok(Message::Text(text)) => {
                        // A runtime's answer to a plane switch, and nothing
                        // else: the conversation itself travels framed on its
                        // own lane.
                        if let (Some(accepted), Some((waiters, runtime))) =
                            (super::link::switch_answer(&text), switches.as_ref())
                        {
                            waiters.settle(runtime, accepted).await;
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
            /*
             * Every call still waiting on this link is answered by dropping
             * its sender, which wakes the waiter at once with an error.
             *
             * Without this, a link that closed left its oneshots alive
             * forever: the handshake, a mode change, and above all a prompt
             * would await an answer that could no longer come, and the turn
             * sat "running" until somebody gave up. A dead link is an answer,
             * and it has to be delivered as one.
             */
            reader.pending.lock().await.clear();
            tracing::info!("conversation link ended");
        });

        // A conversation to rejoin wins over one the pod says it was handed.
        // They answer different questions — one is a control plane taking back
        // over a live runtime, the other a runtime taking over a session — and
        // when both are present the first is the more recent truth.
        bridge
            .handshake(rejoin, resuming, cwd, mode, own_credential)
            .await?;
        let conversation_id = bridge.session.lock().await.clone();

        let writer = Arc::clone(&bridge);
        let updates = updates_tx;
        tokio::spawn(async move {
            while let Some(input) = input_rx.recv().await {
                writer.drive(input, &updates).await;
            }
            tracing::info!("conversation input ended");
        });

        let control = super::control_lane(Arc::clone(&bridge.sink));

        Ok((
            ConversationChannel {
                updates: updates_rx,
                input: input_tx,
                conversation_id,
            },
            control,
        ))
    }

    /// Initialize, get hold of a session, and settle its mode.
    ///
    /// Three ways to have one, and they are genuinely different. A bridge that
    /// is still running holds its session in memory and is simply told which
    /// one this is. A bridge that has just started, on a pod handed a
    /// conversation, loads it from disk. Anything else opens a new one.
    ///
    /// Getting the first two the wrong way round fails, and was checked rather
    /// than reasoned about: a live bridge refuses to load a session it is
    /// already holding, because loading reads a file that a conversation with
    /// no committed turns has not written yet.
    async fn handshake(
        &self,
        rejoining: Option<String>,
        resuming: Option<String>,
        cwd: String,
        mode: Option<String>,
        own_credential: bool,
    ) -> Result<(), AdapterError> {
        let hello = self
            .call(
                "initialize",
                serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    // Declared so the bridge will offer a sign-in this client
                    // can actually complete. It advertises its device-code
                    // method only to a client that says it can put a URL in
                    // front of a person, and this client can: the address and
                    // the code travel up as an update and reach a surface. Say
                    // nothing here and the method is never offered, which is
                    // why an own-account session had no way to sign in.
                    "elicitation": { "url": {} },
                },
                }),
            )
            .await?;

        /*
         * A session on its owner's own account signs in before anything else.
         *
         * Found by running it: the conversation reached the bridge and died on
         * `session/new` with "Authentication required". An unauthenticated
         * kernel has no account to open a conversation against, so the sign-in
         * cannot wait until somebody sends a first message — there is nothing
         * to send it to.
         *
         * Skipped when the pod is already holding a conversation, which is a
         * runtime that signed in earlier and is being taken back over.
         */
        if own_credential && rejoining.is_none() {
            self.authenticate_owner(&hello).await?;
        }

        let session = match (rejoining, resuming) {
            // A conversation this bridge is still holding. Nothing is asked of
            // it beyond the identifier: the session is in the process's memory,
            // and asking it to load one would be asking it to read a file it
            // may not have written.
            (Some(conversation), _) => {
                tracing::info!(%conversation, "rejoining the conversation this runtime is holding");
                conversation
            }
            // Carried from another pod. The rollout is already on this pod's
            // disk — the supervisor put it there — so loading by identifier is
            // what makes the session continue rather than start again.
            (None, Some(conversation)) => {
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
            (None, None) => {
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
                // Not through `call`, which is otherwise the way to ask this
                // bridge something: that one waits under a deadline meant for a
                // question with a quick answer, and a prompt takes as long as
                // the model takes.
                match self.begin("session/prompt", params) {
                    Ok(waiting) => {
                        tokio::spawn(async move {
                            let answered = tokio::time::timeout(TURN_DEADLINE, waiting).await;
                            let outcome = match answered {
                                // Nothing at all, for an hour. Said rather
                                // than left running: a turn nobody will ever
                                // answer looks exactly like a turn in
                                // progress, and only one of them is worth
                                // waiting for.
                                Err(_) => Outcome::Failed {
                                    message: "the agent did not answer this turn".to_string(),
                                    capability: None,
                                },
                                Ok(Ok(frame)) => turn_outcome(frame),
                                // The bridge went away mid-turn. This is a
                                // failure, not an ending: the agent did not
                                // answer, and reporting it as a completed turn
                                // is what made a dead bridge look like an
                                // agent with nothing to say.
                                Ok(Err(_)) => Outcome::Failed {
                                    message: "the agent stopped answering before the turn finished"
                                        .to_string(),
                                    capability: None,
                                },
                            };
                            let _ = updates.send(outcome.into_update()).await;
                        });
                    }
                    // A prompt that could not even be sent is a failed turn,
                    // not a silent no-op: the caller is waiting on an answer
                    // that is never coming.
                    Err(e) => {
                        tracing::warn!(error = %e, "could not send a prompt");
                        let _ = updates
                            .send(ConversationUpdate::Failed {
                                message: format!("the prompt could not be sent to the agent: {e}"),
                                capability: None,
                            })
                            .await;
                    }
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

    /// Sign the agent in to its owner's own account, and wait for it.
    ///
    /// The method is chosen from what the bridge advertised rather than named
    /// here: which sign-ins a kernel offers is the kernel's business, and a
    /// client that hardcoded one would break the first time that changed. A
    /// bridge offering none is refused by name — §9 asks for an absence to be
    /// declared, and a session that cannot sign in cannot run on the account it
    /// was created for.
    ///
    /// This blocks until the sign-in finishes, which is the point: the address
    /// and the code have already reached a surface by then, carried up by the
    /// elicitation the bridge sends while it waits.
    async fn authenticate_owner(&self, hello: &serde_json::Value) -> Result<(), AdapterError> {
        let methods = hello
            .get("authMethods")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let chosen = methods
            .iter()
            .filter_map(|m| m.get("id").and_then(serde_json::Value::as_str))
            // A device code is the only kind completable from here: this pod
            // has no browser, and a method that opened one would hang.
            .find(|id| id.contains("device"))
            .ok_or_else(|| {
                AdapterError::Unsupported(
                    "this runtime offers no sign-in that can be completed away from the machine \
                     it runs on, so it cannot use its owner's own account"
                        .to_string(),
                )
            })?
            .to_string();

        tracing::info!(method = %chosen, "signing in to the owner's own account");
        self.call("authenticate", serde_json::json!({ "methodId": chosen }))
            .await
            .map_err(|e| {
                // Named rather than swallowed. A refused or expired sign-in is
                // the likeliest way this fails, and the person who has to act
                // on it is the one reading the session.
                AdapterError::Rejected(format!("the sign-in did not complete: {e}"))
            })?;
        Ok(())
    }

    /// Ask the bridge something and wait for its answer.
    async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AdapterError> {
        let waiting = self.begin(method, params)?;
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
    /// Takes the method rather than accepting a pre-built object, so a request
    /// without one cannot be constructed. It could before, and one was: the
    /// prompt passed its parameters straight in, and what went on the wire was
    /// a JSON-RPC request carrying an id and no method — undispatchable, so
    /// the agent was never asked anything and every conversation turn opened
    /// and hung.
    ///
    /// Registered before it is sent, because the answer can arrive on the read
    /// loop before this function returns.
    fn begin(
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

/// How a turn came out: finished, or stopped by something going wrong.
///
/// Kept apart from `PromptResponse` because the protocol has no word for
/// "the agent could not answer at all", and folding that into a stop reason
/// is what made every such turn read as an ordinary, empty completion.
enum Outcome {
    Ended(PromptResponse),
    Failed {
        message: String,
        capability: Option<String>,
    },
}

impl Outcome {
    fn into_update(self) -> ConversationUpdate {
        match self {
            Outcome::Ended(ended) => ConversationUpdate::TurnEnded(ended),
            Outcome::Failed {
                message,
                capability,
            } => ConversationUpdate::Failed {
                message,
                capability,
            },
        }
    }
}

/// Read how a turn ended out of the bridge's answer.
///
/// The answer is the whole JSON-RPC frame — `inbound` hands the receiver what
/// it read, and `call` is what usually unwraps `result`. This path did not, and
/// deserialized the envelope itself into a `PromptResponse`. That could only
/// fail, so every turn was reported as having ended normally: a refusal, a
/// context limit and a cancellation all arrived as `end_turn`, and so did an
/// error. A turn that failed looked exactly like a turn that finished.
///
/// Three cases, and each is said rather than flattened:
///
/// * an error — the turn did not finish, and reporting an ending would tell
///   the owner their agent had answered when it had not;
/// * a result that reads — the kernel's own stop reason, which is the whole
///   point of the field;
/// * a result that does not — recorded loudly and ended, because a turn that
///   is over must be closed either way or the session sits `running` forever.
fn turn_outcome(frame: serde_json::Value) -> Outcome {
    if let Some(error) = frame.get("error") {
        // The agent's own words for what went wrong — which is where "could
        // not reach the model", an upstream 401, and a refused tool all
        // arrive. Carried through rather than flattened to a stop reason,
        // because the message is the only part a person can act on.
        tracing::warn!(%error, "the bridge refused a prompt");
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the agent refused the prompt without saying why")
            .to_string();
        return Outcome::Failed {
            message,
            capability: error
                .get("data")
                .and_then(|d| d.get("capability"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        };
    }
    let Some(result) = frame.get("result") else {
        tracing::warn!(answer = %frame, "a prompt was answered with neither a result nor an error");
        return Outcome::Failed {
            message: "the agent answered with neither a result nor an error".to_string(),
            capability: None,
        };
    };
    match serde_json::from_value::<PromptResponse>(result.clone()) {
        Ok(ended) => Outcome::Ended(ended),
        Err(e) => {
            // A newer protocol naming a new way for a turn to finish must not
            // make a healthy session look broken, so this stays an ending.
            tracing::warn!(
                error = %e,
                answer = %result,
                "a turn ended in a way this build cannot read; treating it as finished"
            );
            Outcome::Ended(PromptResponse::new(crate::TurnStopReason::EndTurn))
        }
    }
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod acp_tests;
