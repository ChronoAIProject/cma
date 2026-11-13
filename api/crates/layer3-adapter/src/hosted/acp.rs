use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{stream::SplitSink, SinkExt as _, StreamExt as _};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

use super::update_relay::UpdateRelay;
use super::HostedAgentOpenSettings;
use crate::{
    AdapterError, ConversationChannel, ConversationInputSender, ConversationUpdate, PromptIdentity,
    PromptResponse,
};

pub(super) const PROTOCOL_VERSION: u32 = 1;
pub(super) const CALL_DEADLINE: Duration = Duration::from_secs(30);
pub(super) use crate::conversation_channel::RETAINED_HISTORY_IDLE_DEADLINE;
pub(super) const SIGN_IN_DEADLINE: Duration = Duration::from_secs(16 * 60);
const DECLARATION_DEADLINE: Duration = Duration::from_secs(10);
const INPUT_BUFFER: usize = 1;
const STARTUP_UPDATE_LIMIT: usize = 256;
pub(super) const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub(super) const LANE_CONVERSATION: u8 = 0x02;
pub(super) const LANE_CONVERSATION_GAP: u8 = 0x03;
const LANE_RETAINED_OUTPUT: u8 = 0x04;
const LANE_OUTPUT_ACK: u8 = 0x05;

pub(super) trait ConversationSocket:
    futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
    + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
    + Unpin
    + Send
{
}
impl<T> ConversationSocket for T where
    T: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin
        + Send
{
}
pub(super) type Socket = Box<dyn ConversationSocket>;
type Sink = SplitSink<Socket, Message>;
type Pending = Arc<Mutex<HashMap<String, PendingCall>>>;

#[path = "acp_declaration.rs"]
mod declaration;
use declaration::validate_ready;

#[path = "acp_open.rs"]
mod open;

#[path = "acp_calls.rs"]
mod calls;
pub(super) use calls::rpc_result;

#[path = "acp_recovery.rs"]
mod recovery;

#[path = "acp_output.rs"]
mod output;

#[path = "acp_reader.rs"]
mod reader;

pub(super) struct PendingCall {
    pub(super) answer: tokio::sync::oneshot::Sender<super::retained_json::JsonFrame>,
    pub(super) prompt: bool,
}

pub(super) struct Blocked {
    pub(super) call: serde_json::Value,
    pub(super) options: Vec<crate::PermissionOption>,
    pub(super) owns_prompt_waiter: bool,
}

#[derive(Default)]
struct ConversationState {
    active: Option<String>,
    startup: Vec<(String, ConversationUpdate)>,
    startup_lost: u64,
}

pub(super) struct Bridge {
    pub(super) json_pools: Option<super::retained_json::JsonPools>,
    pub(super) bounded_raw_output: bool,
    pub(super) permission_state: Option<Mutex<super::permission_state::PermissionState>>,
    output_json_budget: bool,
    shared_runtime_output: bool,
    pub(super) subscription: Option<super::subscriptions::HostedSubscriptionBinding>,
    sink: Arc<Mutex<Sink>>,
    pub(super) pending: Pending,
    request_namespace: String,
    next_id: AtomicI64,
    recovery: Mutex<recovery::PromptRecovery>,
    conversation: Mutex<ConversationState>,
    output: Option<Mutex<output::ReliableOutput>>,
    pub(super) blocked: Mutex<HashMap<String, Blocked>>,
    pub(super) answered: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    pub(super) owned_prompt_waiters: Arc<AtomicUsize>,
    pub(super) orphaned_prompt_response: AtomicBool,
    retired: AtomicBool,
    pub(super) instructions_support: AtomicU8,
    pub(super) context_refresh: Option<super::managed_context::HostedContextRefresh>,
    pub(super) context_goal: Mutex<Option<super::context_goal::ContextGoal>>,
    pub(super) recovering_context: AtomicU64,
    pub(super) retained_history_deadline: tokio::sync::watch::Sender<Option<tokio::time::Instant>>,
}

pub(super) struct OpenedBridge {
    pub(super) channel: ConversationChannel,
    pub(super) bridge: Arc<Bridge>,
    pub(super) reader: tokio::task::JoinHandle<()>,
    pub(super) relay: tokio::task::JoinHandle<()>,
}

struct OpeningTasks {
    bridge: Arc<Bridge>,
    reader: tokio::task::AbortHandle,
    relay: tokio::task::AbortHandle,
    armed: bool,
}

impl Drop for OpeningTasks {
    fn drop(&mut self) {
        if self.armed {
            // Do not suspend error publication on socket/task cleanup: a startup
            // recorder can otherwise see closed ACK ownership before the error.
            // The unpublished input drops too, releasing its idle writer.
            self.bridge.retired.store(true, Ordering::SeqCst);
            self.reader.abort();
            self.relay.abort();
        }
    }
}

impl Bridge {
    pub(super) async fn session(&self) -> crate::Result<String> {
        self.conversation
            .lock()
            .await
            .active
            .clone()
            .ok_or_else(|| AdapterError::Unavailable("runtime holds no conversation".into()))
    }

    pub(super) async fn activate_session(
        &self,
        session: String,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        super::acp_session_identity::validate(&session)?;
        let mut state = self.conversation.lock().await;
        if state
            .active
            .as_ref()
            .is_some_and(|active| active != &session)
        {
            return Err(AdapterError::Rejected(
                "bridge changed the active conversation identifier".into(),
            ));
        }
        state.active = Some(session.clone());
        for (candidate, update) in std::mem::take(&mut state.startup) {
            if candidate == session {
                updates.send(update.without_prompt_evidence()).await;
            }
        }
        if state.startup_lost > 0 {
            updates
                .send(ConversationUpdate::Lost {
                    frames: state.startup_lost,
                })
                .await;
            state.startup_lost = 0;
        }
        Ok(())
    }

    pub(super) async fn route_update(
        &self,
        session: &str,
        update: ConversationUpdate,
        updates: &UpdateRelay,
    ) {
        let mut state = self.conversation.lock().await;
        match state.active.as_deref() {
            Some(active) if active == session => {
                drop(state);
                updates.send(self.scope_activity(update)).await;
            }
            Some(_) => tracing::warn!("discarding update for another conversation"),
            None if state.startup.len() < STARTUP_UPDATE_LIMIT => {
                state.startup.push((session.to_string(), update));
            }
            None => state.startup_lost = state.startup_lost.saturating_add(1),
        }
    }

    fn scope_activity(&self, update: ConversationUpdate) -> ConversationUpdate {
        let owns_prompt = self.owned_prompt_waiters.load(Ordering::SeqCst) > 0;
        super::acp_inbound::scope_activity(update, owns_prompt)
    }

    pub(super) async fn begin(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> crate::Result<tokio::sync::oneshot::Receiver<super::retained_json::JsonFrame>> {
        self.begin_inner(method, params, false).await
    }

    pub(super) async fn begin_prompt(
        &self,
        params: serde_json::Value,
        prompt_id: Option<PromptIdentity>,
    ) -> crate::Result<tokio::sync::oneshot::Receiver<super::retained_json::JsonFrame>> {
        self.begin_with_id("session/prompt", params, true, prompt_id)
            .await
    }

    async fn begin_inner(
        &self,
        method: &str,
        params: serde_json::Value,
        prompt: bool,
    ) -> crate::Result<tokio::sync::oneshot::Receiver<super::retained_json::JsonFrame>> {
        self.begin_with_id(method, params, prompt, None).await
    }

    async fn begin_with_id(
        &self,
        method: &str,
        params: serde_json::Value,
        prompt: bool,
        prompt_id: Option<PromptIdentity>,
    ) -> crate::Result<tokio::sync::oneshot::Receiver<super::retained_json::JsonFrame>> {
        if self.retired.load(Ordering::SeqCst) {
            return Err(write::retired_before_write(prompt));
        }
        let remembered_prompt = prompt_id.clone();
        let id = prompt_id.map_or_else(
            || {
                format!(
                    "cma-rpc-{}-{}",
                    self.request_namespace,
                    self.next_id.fetch_add(1, Ordering::SeqCst)
                )
            },
            |identity| identity.as_str().to_owned(),
        );
        let (answer, waiting) = tokio::sync::oneshot::channel();
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.pending
            .lock()
            .await
            .insert(id.clone(), PendingCall { answer, prompt });
        if prompt {
            if let Some(state) = &self.permission_state {
                state.lock().await.begin_prompt(&id);
            }
            if let Some(prompt_id) = remembered_prompt.clone() {
                self.remember_prompt(prompt_id).await;
            }
        }
        if let Err(error) = self.send_frame(&frame, prompt).await {
            self.pending.lock().await.remove(&id);
            if prompt {
                if let Some(prompt_id) = remembered_prompt {
                    self.clear_active_prompt_if(prompt_id.as_str()).await;
                }
            }
            return Err(error);
        }
        Ok(waiting)
    }
}

fn unframe(bytes: &[u8]) -> Option<(u8, &[u8])> {
    bytes.split_first().map(|(lane, payload)| (*lane, payload))
}

fn gap_count(payload: &[u8]) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(payload)
        .ok()?
        .get("frames")?
        .as_u64()
}

pub(super) enum Outcome {
    Ended(PromptResponse),
    Failed {
        message: String,
        capability: Option<String>,
    },
}

#[path = "acp_write.rs"]
mod write;

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;
