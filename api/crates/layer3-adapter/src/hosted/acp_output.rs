//! Hold domain frames through native identity verification, preserving wire order.

use std::collections::VecDeque;

use futures_util::SinkExt as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::{Bridge, LANE_OUTPUT_ACK, MAX_FRAME_BYTES};
use crate::hosted::update_relay::UpdateRelay;
use crate::{AdapterError, ConversationUpdate, RuntimeOutputReceipt, RuntimeOutputSource};

const STARTUP_BYTES: usize = 8 * 1024 * 1024;
const HEADER_BYTES: usize = 24;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OutputCapability {
    version: u32,
    stream_id: uuid::Uuid,
}

impl OutputCapability {
    pub(super) fn validate(self) -> crate::Result<uuid::Uuid> {
        if self.version != 1 || self.stream_id.is_nil() {
            return Err(AdapterError::Rejected(
                "unsupported runtime output acknowledgement".into(),
            ));
        }
        Ok(self.stream_id)
    }
}

struct HeldOutput {
    cursor: u64,
    payload: Vec<u8>,
    /// Bootstrap RPCs must run now; their domain effect still owns this cursor.
    prepared: Option<Option<ConversationUpdate>>,
}

pub(super) struct ReliableOutput {
    stream: uuid::Uuid,
    last_received: Option<u64>,
    session: Option<String>,
    queued: VecDeque<HeldOutput>,
    queued_bytes: usize,
    /// Cursor of an identity-bearing terminal that could not be committed.
    /// Cumulative ACKs must not cross this frame.
    blocked_at: Option<u64>,
    acknowledgements: mpsc::Sender<RuntimeOutputSource>,
    durable_progress: tokio::sync::watch::Sender<u64>,
}

impl ReliableOutput {
    pub(super) fn new(
        stream: uuid::Uuid,
        acknowledgements: mpsc::Sender<RuntimeOutputSource>,
    ) -> Self {
        Self {
            stream,
            last_received: None,
            session: None,
            queued: VecDeque::new(),
            queued_bytes: 0,
            blocked_at: None,
            acknowledgements,
            durable_progress: tokio::sync::watch::channel(0).0,
        }
    }
}

impl Bridge {
    pub(in crate::hosted) async fn output_progress(
        &self,
    ) -> Option<tokio::sync::watch::Receiver<u64>> {
        let state = self.output.as_ref()?.lock().await;
        state.session.as_ref()?;
        Some(state.durable_progress.subscribe())
    }

    pub(super) async fn receive_output(
        &self,
        bytes: &[u8],
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        let fail = || AdapterError::Rejected("invalid retained runtime output position".into());
        let state = self.output.as_ref().ok_or_else(fail)?;
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_FRAME_BYTES + HEADER_BYTES {
            return Err(fail());
        }
        let stream = uuid::Uuid::from_slice(&bytes[..16]).map_err(|_| fail())?;
        let cursor = u64::from_be_bytes(bytes[16..24].try_into().map_err(|_| fail())?);
        let mut state = state.lock().await;
        if stream != state.stream
            || cursor == 0
            || state
                .last_received
                .is_some_and(|previous| previous.checked_add(1) != Some(cursor))
        {
            return Err(fail());
        }
        state.last_received = Some(cursor);
        let mut held = HeldOutput {
            cursor,
            payload: bytes[HEADER_BYTES..].to_vec(),
            prepared: None,
        };
        if state.session.is_none() {
            if state.queued_bytes.saturating_add(bytes.len()) > STARTUP_BYTES {
                return Err(AdapterError::Unavailable(
                    "retained runtime startup output exceeded its bound".into(),
                ));
            }
            if self.bootstrap_output(&held.payload).await {
                let effect = self.capture_output(&held.payload).await?;
                held.prepared = Some(match effect {
                    // Owner authentication must remain visible before session creation.
                    Some(notice @ ConversationUpdate::SignInRequired { .. }) => {
                        updates.send(notice);
                        None
                    }
                    other => other,
                });
            }
            state.queued_bytes += bytes.len();
            state.queued.push_back(held);
            return Ok(());
        }
        if self.unmatched_terminal(&held.payload).await {
            state.blocked_at = Some(cursor);
        }
        self.deliver_output(&state, held, updates).await
    }

    async fn unmatched_terminal(&self, payload: &[u8]) -> bool {
        let Ok(frame) = serde_json::from_slice::<serde_json::Value>(payload) else {
            return false;
        };
        let Some(id) = frame.get("id").and_then(serde_json::Value::as_str) else {
            return false;
        };
        if crate::PromptIdentity::parse(id).is_none()
            || frame.get("method").is_some()
            || (frame.get("error").is_none()
                && !frame
                    .get("result")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|result| result.contains_key("stopReason")))
        {
            return false;
        }
        if self.pending.lock().await.contains_key(id) {
            return false;
        }
        !self.recovery.lock().await.expects(id)
    }

    async fn bootstrap_output(&self, payload: &[u8]) -> bool {
        let Ok(frame) = serde_json::from_slice::<serde_json::Value>(payload) else {
            return false;
        };
        match frame.get("method").and_then(serde_json::Value::as_str) {
            Some("session/update" | "session/request_permission") => false,
            Some(_) => true,
            None => {
                let Some(id) = frame.get("id").and_then(serde_json::Value::as_str) else {
                    return false;
                };
                self.pending
                    .lock()
                    .await
                    .get(id)
                    .is_some_and(|waiting| !waiting.prompt)
            }
        }
    }

    async fn capture_output(&self, payload: &[u8]) -> crate::Result<Option<ConversationUpdate>> {
        let (capture, mut effects) = UpdateRelay::capture();
        self.receive(payload, &capture).await;
        let effect = effects.try_recv().ok();
        if effects.try_recv().is_ok() {
            return Err(AdapterError::Rejected(
                "one runtime frame produced multiple domain updates".into(),
            ));
        }
        Ok(effect)
    }

    pub(super) async fn activate_output(
        &self,
        session: &str,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        let Some(state) = &self.output else {
            return Ok(());
        };
        let mut state = state.lock().await;
        state.session = Some(session.to_owned());
        while let Some(held) = state.queued.pop_front() {
            if self.unmatched_terminal(&held.payload).await {
                state.blocked_at = Some(held.cursor);
            }
            self.deliver_output(&state, held, updates).await?;
        }
        state.queued_bytes = 0;
        Ok(())
    }

    async fn deliver_output(
        &self,
        state: &ReliableOutput,
        held: HeldOutput,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        if let Ok(frame) = serde_json::from_slice::<serde_json::Value>(&held.payload) {
            if matches!(
                frame.get("method").and_then(serde_json::Value::as_str),
                Some("session/update" | "session/request_permission")
            ) && frame
                .pointer("/params/sessionId")
                .and_then(serde_json::Value::as_str)
                != state.session.as_deref()
            {
                return Err(AdapterError::Rejected(
                    "retained output changed the original conversation identity".into(),
                ));
            }
        }
        let update = match held.prepared {
            Some(effect) => effect,
            None => self.capture_output(&held.payload).await?,
        };
        let source = RuntimeOutputSource {
            stream_id: state.stream.to_string(),
            cursor: held.cursor,
            native_conversation_id: state.session.clone().ok_or_else(|| {
                AdapterError::Unavailable("output has no verified conversation".into())
            })?,
            payload_sha256: format!("{:x}", Sha256::digest(&held.payload)),
        };
        updates.send(ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(source, state.acknowledgements.clone()),
            update: update.map(Box::new),
        });
        Ok(())
    }

    pub(super) async fn acknowledge_output(
        &self,
        source: RuntimeOutputSource,
    ) -> crate::Result<()> {
        let stream = uuid::Uuid::parse_str(&source.stream_id).map_err(|_| {
            AdapterError::Rejected("invalid output acknowledgement identity".into())
        })?;
        let state = self.output.as_ref().map(|state| state.lock());
        let mut state = match state {
            Some(state) => Some(state.await),
            None => None,
        };
        if let Some(blocked) = state.as_ref().and_then(|state| state.blocked_at) {
            if source.cursor > blocked {
                return Err(AdapterError::Unavailable(format!(
                    "runtime output terminal at cursor {blocked} is not durably resolved"
                )));
            }
            if source.cursor < blocked {
                return Err(AdapterError::Unavailable(
                    "runtime output acknowledgement is behind an unresolved terminal".into(),
                ));
            }
        }
        let mut bytes = Vec::with_capacity(HEADER_BYTES + 1);
        bytes.push(LANE_OUTPUT_ACK);
        bytes.extend_from_slice(stream.as_bytes());
        bytes.extend_from_slice(&source.cursor.to_be_bytes());
        self.sink
            .lock()
            .await
            .send(Message::Binary(bytes))
            .await
            .map_err(|error| {
                AdapterError::Unavailable(format!("runtime acknowledgement write failed: {error}"))
            })?;
        if let Some(state) = state.as_mut() {
            if state.blocked_at == Some(source.cursor) {
                state.blocked_at = None;
            }
            state.durable_progress.send_if_modified(|last| {
                if source.cursor <= *last {
                    return false;
                }
                *last = source.cursor;
                true
            });
        }
        Ok(())
    }
}
