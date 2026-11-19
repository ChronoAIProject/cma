//! Hold domain frames through native identity verification, preserving wire order.

use crate::ingress_budget::diagnostic::text;
use std::sync::Arc;

use crate::ingress_budget::{IngressBudget, IngressReservation};
#[cfg(test)]
use futures_util::SinkExt as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::Semaphore;
use tokio_tungstenite::tungstenite::Message;

use super::{Bridge, LANE_OUTPUT_ACK, MAX_FRAME_BYTES};
use crate::hosted::update_relay::UpdateRelay;
use crate::{AdapterError, ConversationUpdate, RuntimeOutputSource};

const STARTUP_BYTES: usize = 8 * 1024 * 1024;
const HEADER_BYTES: usize = 24;

#[path = "acp_output_memory.rs"]
mod memory;
use memory::OutputQueue;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OutputCapability {
    version: u32,
    stream_id: uuid::Uuid,
}

impl OutputCapability {
    pub(super) fn validate(self) -> crate::Result<uuid::Uuid> {
        if self.version != 1 || self.stream_id.is_nil() {
            return Err(AdapterError::Rejected(text(
                "unsupported runtime output acknowledgement",
            )));
        }
        Ok(self.stream_id)
    }
}

pub(super) struct HeldOutput {
    cursor: u64,
    payload: Vec<u8>,
    decoded: Option<crate::hosted::retained_json::JsonFrame>,
    /// Bootstrap RPCs must run now; their domain effect still owns this cursor.
    prepared: Option<Option<ConversationUpdate>>,
    sign_in: Option<crate::hosted::sign_in_retention::Admission>,
    observation: Option<crate::RuntimeOutputUpdate>,
    // Last field: release only after the payload and queued node are gone.
    _storage: Option<IngressReservation>,
}

impl HeldOutput {
    fn frame(&self) -> Option<std::borrow::Cow<'_, serde_json::Value>> {
        match &self.decoded {
            Some(frame) => Some(std::borrow::Cow::Borrowed(frame)),
            None => serde_json::from_slice(&self.payload)
                .ok()
                .map(std::borrow::Cow::Owned),
        }
    }
}

pub(super) struct ReliableOutput {
    stream: uuid::Uuid,
    last_received: Option<u64>,
    session: Option<String>,
    queued: OutputQueue,
    raw_budget: Option<Arc<Semaphore>>,
    aggregate: Option<IngressBudget>,
    json_pools: Option<crate::hosted::retained_json::JsonPools>,
    queued_bytes: usize,
    acknowledgements: super::ingress::AckSender,
    durable_progress: super::progress::ProgressSender<u64>,
    session_storage: Option<crate::ingress_budget::ReservationBundle>,
}

pub(super) struct OutputAckState {
    stream: uuid::Uuid,
    progress: super::progress::ProgressSender<u64>,
}

impl ReliableOutput {
    pub(super) fn new(
        stream: uuid::Uuid,
        acknowledgements: super::ingress::AckSender,
        bounded: bool,
        json_pools: Option<crate::hosted::retained_json::JsonPools>,
        aggregate: Option<IngressBudget>,
        policy: Option<&crate::ingress_budget::IngressOwnershipPolicy>,
    ) -> crate::Result<Self> {
        Ok(Self {
            stream,
            json_pools,
            aggregate,
            last_received: None,
            session: None,
            queued: OutputQueue::new(bounded),
            raw_budget: memory::process_budget(bounded),
            queued_bytes: 0,
            acknowledgements,
            durable_progress: super::progress::ProgressSender::new(0, policy)?,
            session_storage: None,
        })
    }
}

impl Bridge {
    // Capture before processing frames. Delivery may hold the output mutex
    // while waiting for a bounded relay; durable ACKs must not need that mutex.
    pub(super) async fn output_ack_state(&self) -> Option<OutputAckState> {
        let state = self.output.as_ref()?.lock().await;
        Some(OutputAckState {
            stream: state.stream,
            progress: state.durable_progress.clone(),
        })
    }

    pub(in crate::hosted) async fn output_progress(
        &self,
    ) -> Option<tokio::sync::watch::Receiver<u64>> {
        let state = self.output.as_ref()?.lock().await;
        state.session.as_ref()?;
        state.durable_progress.legacy_receiver()
    }

    pub(in crate::hosted) async fn output_observation(
        &self,
    ) -> Option<crate::ingress_budget::observation::DeliveryWatchReceiver<u64>> {
        let state = self.output.as_ref()?.lock().await;
        state.session.as_ref()?;
        Some(state.durable_progress.observe())
    }

    pub(super) async fn receive_output(
        &self,
        bytes: &[u8],
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        self.check_owner().await?;
        let fail = || AdapterError::Rejected(text("invalid retained runtime output position"));
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
        if self.output_json_budget && state.json_pools.is_none() {
            super::super::output_json_budget::validate(&bytes[HEADER_BYTES..])
                .map_err(|error| error.adapter_error())?;
        }
        if state.session.is_none() && state.queued_bytes.saturating_add(bytes.len()) > STARTUP_BYTES
        {
            return Err(AdapterError::Unavailable(text(
                "retained runtime startup output exceeded its bound",
            )));
        }
        // Refuse before copying, queue allocation, accepted cursor, or effects.
        let storage = memory::reserve(
            state.raw_budget.as_ref(),
            bytes.len() - HEADER_BYTES,
            state.aggregate.as_ref(),
        )?;
        let decoded = state
            .json_pools
            .as_ref()
            .map(|pools| pools.decode(&bytes[HEADER_BYTES..]))
            .transpose()?;
        let sign_in = match (&state.json_pools, &decoded) {
            (Some(pools), Some(frame)) => pools.admit_sign_in(frame)?,
            _ => None,
        };
        let observation = match (&state.json_pools, &decoded) {
            (Some(pools), Some(frame)) => pools.admit_observation(frame)?,
            _ => None,
        };
        let decoded = decoded
            .map(|frame| {
                let prepared = self.prepare_update(&frame, observation.is_some())?;
                let terminal = self.prepare_terminal(&frame)?;
                let frame = frame
                    .with_prepared_update(prepared)
                    .with_prepared_terminal(terminal);
                self.prepare_delivery(frame)
            })
            .transpose()?;
        let mut held = HeldOutput {
            cursor,
            payload: bytes[HEADER_BYTES..].to_vec(),
            decoded,
            prepared: None,
            sign_in,
            observation,
            _storage: storage,
        };
        if self.ingress.is_none() {
            state.last_received = Some(cursor);
        }
        if state.session.is_none() {
            if self.bootstrap_output(&held).await {
                let effect = self.capture_output(&mut held).await?;
                held.prepared = Some(match effect {
                    // Owner authentication must remain visible before session creation.
                    Some(notice)
                        if matches!(notice.effect(), ConversationUpdate::SignInRequired { .. }) =>
                    {
                        updates.send(notice).await;
                        None
                    }
                    other => other,
                });
            }
            state.queued_bytes += bytes.len();
            state.queued.push_back(held);
            state.last_received = Some(cursor);
            return Ok(());
        }
        self.deliver_output(&state, held, updates).await?;
        state.last_received = Some(cursor);
        Ok(())
    }

    async fn bootstrap_output(&self, held: &HeldOutput) -> bool {
        let Some(frame) = held.frame() else {
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

    async fn capture_output(
        &self,
        held: &mut HeldOutput,
    ) -> crate::Result<Option<ConversationUpdate>> {
        if let Some(policy) = &self.ingress {
            let (capture, effect) = UpdateRelay::capture_admitted(policy)?;
            let frame = held.decoded.as_ref().ok_or_else(|| {
                AdapterError::Rejected(text("admitted output requires retained JSON"))
            })?;
            self.receive_admitted_frame(
                frame.clone(),
                &capture,
                held.sign_in.take(),
                held.observation.take(),
                frame.prepared_update(),
            )
            .await?;
            return effect.take();
        }
        let (capture, mut effects) = UpdateRelay::capture();
        let sign_in = held.sign_in.take();
        let observation = held.observation.take();
        match &held.decoded {
            Some(frame) => {
                self.receive_admitted_frame(
                    frame.clone(),
                    &capture,
                    sign_in,
                    observation,
                    frame.prepared_update(),
                )
                .await?
            }
            None => self.receive(&held.payload, &capture).await?,
        }
        let effect = effects.try_recv().ok();
        if effects.try_recv().is_ok() {
            return Err(AdapterError::Rejected(text(
                "one runtime frame produced multiple domain updates",
            )));
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
        let storage = self
            .ingress
            .as_ref()
            .map(|policy| {
                policy.reserve(
                    crate::ingress_budget::IngressClass::Data,
                    crate::ingress_budget::AllocationPlan::new(
                        "output conversation identity",
                        &[session.len()],
                    )?,
                )
            })
            .transpose()?;
        state.session = Some(session.to_owned());
        state.session_storage = storage;
        while let Some(held) = state.queued.pop_front() {
            state.queued_bytes -= held.payload.len() + HEADER_BYTES;
            self.deliver_output(&state, held, updates).await?;
        }
        state.queued_bytes = 0;
        Ok(())
    }

    async fn deliver_output(
        &self,
        state: &ReliableOutput,
        mut held: HeldOutput,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        if let Some(frame) = held.frame() {
            if matches!(
                frame.get("method").and_then(serde_json::Value::as_str),
                Some("session/update" | "session/request_permission")
            ) && frame
                .pointer("/params/sessionId")
                .and_then(serde_json::Value::as_str)
                != state.session.as_deref()
            {
                return Err(AdapterError::Rejected(text(
                    "retained output changed the original conversation identity",
                )));
            }
        }
        let update = match held.prepared.take() {
            Some(effect) => effect,
            None => self.capture_output(&mut held).await?,
        };
        let delivery_storage = held
            .decoded
            .as_ref()
            .and_then(|frame| frame.delivery_storage());
        if self.ingress.is_some() && delivery_storage.is_none() {
            return Err(AdapterError::Rejected(text(
                "runtime delivery has no admitted owner",
            )));
        }
        let source = RuntimeOutputSource {
            stream_id: state.stream.to_string(),
            cursor: held.cursor,
            native_conversation_id: state.session.clone().ok_or_else(|| {
                AdapterError::Unavailable(text("output has no verified conversation"))
            })?,
            payload_sha256: format!("{:x}", Sha256::digest(&held.payload)),
        };
        let delivery = ConversationUpdate::RuntimeOutput {
            receipt: state.acknowledgements.receipt(source),
            update: update.map(|update| match update {
                // Transfer the already admitted owner without another box
                // or an independent clone policy around its string payload.
                ConversationUpdate::RetainedEffect(effect) => effect,
                update => crate::RuntimeOutputUpdate::new(update, self.shared_runtime_output),
            }),
        };
        let delivery = match delivery_storage {
            Some(storage) => ConversationUpdate::RetainedEffect(
                crate::RuntimeOutputUpdate::from_admitted(storage.initialize(|| delivery)),
            ),
            None => delivery,
        };
        updates.send(delivery).await;
        Ok(())
    }

    pub(super) async fn acknowledge_output(
        &self,
        source: impl std::borrow::Borrow<RuntimeOutputSource>,
        state: Option<&OutputAckState>,
    ) -> crate::Result<()> {
        let source = source.borrow();
        let stream = uuid::Uuid::parse_str(&source.stream_id)
            .map_err(|_| AdapterError::Rejected(text("invalid output acknowledgement identity")))?;
        let state = state
            .filter(|state| state.stream == stream && source.cursor > 0)
            .ok_or_else(|| {
                AdapterError::Rejected(text("output acknowledgement changed its stream"))
            })?;
        let scope = self
            .ingress
            .as_ref()
            .map(|policy| {
                let plan = crate::ingress_budget::AllocationPlan::new(
                    "acknowledgement write",
                    &[
                        HEADER_BYTES + 1,
                        crate::ingress_budget::sizing::shared_owner_bytes::<Message>(),
                    ],
                )?;
                policy
                    .reserve(crate::ingress_budget::IngressClass::Control, plan)
                    .map(crate::ingress_budget::IngressWorkScope::new)
            })
            .transpose()?;
        let write = async {
            let mut bytes = Vec::with_capacity(HEADER_BYTES + 1);
            bytes.push(LANE_OUTPUT_ACK);
            bytes.extend_from_slice(stream.as_bytes());
            bytes.extend_from_slice(&source.cursor.to_be_bytes());
            self.write_owned(Message::Binary(bytes)).await
        };
        match scope {
            Some(scope) => scope.run_io(write).await?,
            None => write.await?,
        }
        state.progress.send_if_modified(|last| {
            if source.cursor <= *last {
                return false;
            }
            *last = source.cursor;
            true
        });
        Ok(())
    }
}

#[cfg(test)]
#[path = "acp_output_budget_tests.rs"]
mod budget_tests;

#[cfg(test)]
#[path = "acp_output_json_retention_tests.rs"]
mod json_retention_tests;

#[cfg(test)]
#[path = "acp_output_coordinator_tests.rs"]
mod coordinator_tests;

#[cfg(test)]
#[path = "acp_sign_in_retention_tests.rs"]
mod sign_in_retention_tests;

#[cfg(test)]
#[path = "acp_context_goal_ownership_tests.rs"]
mod context_goal_ownership_tests;

#[cfg(test)]
#[path = "acp_observation_retention_tests.rs"]
mod observation_retention_tests;

#[cfg(test)]
#[path = "acp_session_identity_startup_tests.rs"]
mod session_identity_startup_tests;

#[cfg(test)]
#[path = "acp_correlation_flow_tests.rs"]
mod correlation_flow_tests;
