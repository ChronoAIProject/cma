use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering};
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
pub(super) const TURN_DEADLINE: Duration = Duration::from_secs(60 * 60);
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

#[path = "acp_recovery.rs"]
mod recovery;

#[path = "acp_output.rs"]
mod output;

#[path = "acp_reader.rs"]
mod reader;

pub(super) struct PendingCall {
    pub(super) answer: tokio::sync::oneshot::Sender<serde_json::Value>,
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
}

pub(super) struct OpenedBridge {
    pub(super) channel: ConversationChannel,
    pub(super) bridge: Arc<Bridge>,
    pub(super) reader: tokio::task::JoinHandle<()>,
    pub(super) relay: tokio::task::JoinHandle<()>,
}

impl Bridge {
    #[cfg(test)]
    pub(super) async fn open(
        stream: Socket,
        expected_agent: &str,
        cwd: String,
        settings: HostedAgentOpenSettings,
        resume_conversation: Option<String>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<OpenedBridge> {
        Self::open_with_subscription(
            stream,
            expected_agent,
            cwd,
            (settings, None),
            resume_conversation,
            updates_tx,
        )
        .await
    }

    #[cfg(test)]
    pub(super) async fn open_with_subscription(
        stream: Socket,
        expected_agent: &str,
        cwd: String,
        (settings, subscription): (
            HostedAgentOpenSettings,
            Option<super::subscriptions::HostedSubscriptionBinding>,
        ),
        resume_conversation: Option<String>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<OpenedBridge> {
        Self::open_with_subscription_and_prompt(
            stream,
            expected_agent,
            cwd,
            (settings, subscription),
            resume_conversation,
            None,
            updates_tx,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn open_with_subscription_and_prompt(
        stream: Socket,
        expected_agent: &str,
        cwd: String,
        (settings, subscription): (
            HostedAgentOpenSettings,
            Option<super::subscriptions::HostedSubscriptionBinding>,
        ),
        resume_conversation: Option<String>,
        recovered_prompt: Option<PromptIdentity>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<OpenedBridge> {
        if recovered_prompt.is_some() && resume_conversation.is_none() {
            return Err(AdapterError::Rejected(
                "prompt recovery requires a held conversation".into(),
            ));
        }
        let (sink, mut source) = stream.split();
        let declaration = tokio::time::timeout(DECLARATION_DEADLINE, source.next())
            .await
            .map_err(|_| AdapterError::Unavailable("runtime sent no Ready declaration".into()))?
            .ok_or_else(|| AdapterError::Unavailable("runtime closed before Ready".into()))?
            .map_err(|error| AdapterError::Unavailable(format!("Ready read failed: {error}")))?;
        let output_stream = validate_ready(declaration, expected_agent)?;
        let (acknowledgements, acknowledged) = mpsc::channel(128);

        let bridge = Arc::new(Self {
            subscription,
            sink: Arc::new(Mutex::new(sink)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            request_namespace: uuid::Uuid::new_v4().to_string(),
            next_id: AtomicI64::new(1),
            recovery: Mutex::new(recovery::PromptRecovery::new(recovered_prompt)),
            conversation: Mutex::new(ConversationState::default()),
            output: output_stream
                .map(|stream| Mutex::new(output::ReliableOutput::new(stream, acknowledgements))),
            blocked: Mutex::new(HashMap::new()),
            answered: Arc::new(Mutex::new(HashMap::new())),
            owned_prompt_waiters: Arc::new(AtomicUsize::new(0)),
            orphaned_prompt_response: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            instructions_support: AtomicU8::new(0),
            context_refresh: settings.context_refresh.clone(),
        });
        let (updates, relay) = super::update_relay::spawn(updates_tx);
        let writer = Arc::clone(&bridge);
        let writer_updates = updates.clone();
        let input = ConversationInputSender::spawn(INPUT_BUFFER, move |input| {
            let writer = Arc::clone(&writer);
            let updates = writer_updates.clone();
            async move { writer.drive(input, &updates).await }
        })
        .with_settings_support(bridge.clone());

        let reader_task = reader::spawn(Arc::clone(&bridge), source, updates.clone(), acknowledged);

        if let Err(error) = bridge
            .handshake(resume_conversation, cwd, settings, &updates)
            .await
        {
            bridge.retire().await;
            reader_task.abort();
            let _ = reader_task.await;
            relay.abort();
            let _ = relay.await;
            return Err(error);
        }
        if reader_task.is_finished() {
            bridge.retire().await;
            let _ = reader_task.await;
            relay.abort();
            let _ = relay.await;
            return Err(AdapterError::Unavailable(
                "conversation link ended while opening".into(),
            ));
        }
        let conversation_id = bridge.session().await?;
        bridge.release_recovered_prompt(&updates).await;
        if let Err(error) = bridge.activate_output(&conversation_id, &updates).await {
            bridge.retire().await;
            reader_task.abort();
            relay.abort();
            return Err(error);
        }
        Ok(OpenedBridge {
            channel: ConversationChannel {
                input,
                conversation_id: Some(conversation_id),
            },
            bridge,
            reader: reader_task,
            relay,
        })
    }

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
        if session.is_empty() || session.len() > 1_024 || session.chars().any(char::is_control) {
            return Err(AdapterError::Rejected(
                "bridge returned an invalid conversation identifier".into(),
            ));
        }
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
                updates.send(update.without_prompt_evidence());
            }
        }
        if state.startup_lost > 0 {
            updates.send(ConversationUpdate::Lost {
                frames: state.startup_lost,
            });
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
                updates.send(self.scope_activity(update));
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

    pub(super) async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        self.call_within(method, params, CALL_DEADLINE).await
    }

    pub(super) async fn call_within(
        &self,
        method: &str,
        params: serde_json::Value,
        deadline: Duration,
    ) -> crate::Result<serde_json::Value> {
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

    pub(super) async fn begin(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> crate::Result<tokio::sync::oneshot::Receiver<serde_json::Value>> {
        self.begin_inner(method, params, false).await
    }

    pub(super) async fn begin_prompt(
        &self,
        params: serde_json::Value,
        prompt_id: Option<PromptIdentity>,
    ) -> crate::Result<tokio::sync::oneshot::Receiver<serde_json::Value>> {
        self.begin_with_id("session/prompt", params, true, prompt_id)
            .await
    }

    async fn begin_inner(
        &self,
        method: &str,
        params: serde_json::Value,
        prompt: bool,
    ) -> crate::Result<tokio::sync::oneshot::Receiver<serde_json::Value>> {
        self.begin_with_id(method, params, prompt, None).await
    }

    async fn begin_with_id(
        &self,
        method: &str,
        params: serde_json::Value,
        prompt: bool,
        prompt_id: Option<PromptIdentity>,
    ) -> crate::Result<tokio::sync::oneshot::Receiver<serde_json::Value>> {
        if self.retired.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "conversation link has retired".into(),
            ));
        }
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
        if let Err(error) = self.send(&frame).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        Ok(waiting)
    }

    pub(super) async fn send(&self, frame: &serde_json::Value) -> crate::Result<()> {
        if self.retired.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "conversation link has retired".into(),
            ));
        }
        let body = serde_json::to_vec(frame)
            .map_err(|_| AdapterError::Rejected("ACP frame could not be encoded".into()))?;
        if body.len() > MAX_FRAME_BYTES {
            return Err(AdapterError::Rejected(
                "ACP frame exceeds the size limit".into(),
            ));
        }
        let mut framed = Vec::with_capacity(body.len() + 1);
        framed.push(LANE_CONVERSATION);
        framed.extend_from_slice(&body);
        self.sink
            .lock()
            .await
            .send(Message::Binary(framed))
            .await
            .map_err(|error| {
                AdapterError::Unavailable(format!("conversation write failed: {error}"))
            })
    }

    pub(super) async fn retire(&self) {
        if !self.retired.swap(true, Ordering::SeqCst) {
            self.pending.lock().await.clear();
            self.blocked.lock().await.clear();
            let _ = self.sink.lock().await.close().await;
        }
    }

    async fn finish(&self) {
        self.retired.store(true, Ordering::SeqCst);
        self.pending.lock().await.clear();
        self.blocked.lock().await.clear();
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

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;
