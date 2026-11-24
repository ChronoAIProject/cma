use layer3_adapter::agent_capacity_recovery::ConfirmedAgentState;
use layer3_adapter::stale_turn::{
    SealedOutput, StaleTurnHold, StaleTurnSeal, StaleTurnTarget, SEAL_VERSION,
};
use layer3_adapter::RuntimeOutputSource;
use serde::Deserialize;

use super::input::QueuedStop;
use super::observation::NativeObservation;
use super::{Bridge, BridgeInput, OutputRetention, OUTPUT_ACKNOWLEDGED};
use crate::manager_ownership::OwnerSession;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub(crate) const FLAG: &str = "CHRONO_RUNTIME_STALE_TURN_SEAL_ENABLED";
const STOP: &str = "cma/codex/stop-exact-turn";
const FENCE: &str = "cma/codex/turn-stop-fence";
const RETAIN_FOR: Duration = Duration::from_secs(60 * 60);

pub(super) struct Stop {
    target: StaleTurnTarget,
    prompt: String,
    receipt: String,
    expires_at: Instant,
    cursor: Option<u64>,
    discarded: Arc<AtomicBool>,
}

impl Stop {
    fn matches(&self, target: &StaleTurnTarget, stream_id: uuid::Uuid) -> bool {
        if self.discarded.load(Ordering::Acquire)
            || self.expires_at <= Instant::now()
            || target.writer_epoch < self.target.writer_epoch
        {
            return false;
        }
        if let Some(expected_stream) = target.stream_id.as_deref() {
            if uuid::Uuid::parse_str(expected_stream).ok() != Some(stream_id) {
                return false;
            }
        } else if self.target.stream_id.is_some() {
            return false;
        }
        let mut current = target.clone();
        current.writer_epoch = self.target.writer_epoch;
        current.stream_id.clone_from(&self.target.stream_id);
        current == self.target
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Fence {
    version: u32,
    thread_id: String,
    turn_id: String,
    listener_incarnation: String,
    dispatch_sequence: u32,
    dispatch_key: String,
    receipt: String,
}

fn frame_method(frame: &[u8], method: &str) -> Option<serde_json::Value> {
    // The private methods are small. Leave ordinary frames on their existing path.
    if frame.len() > 4096
        || !frame
            .windows(method.len())
            .any(|window| window == method.as_bytes())
    {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(frame).ok()?;
    (value.get("method").and_then(serde_json::Value::as_str) == Some(method)).then_some(value)
}

#[cfg(test)]
#[path = "bridge_stale_turn_tests.rs"]
mod tests;

pub(super) fn is_untrusted_stop(frame: &[u8]) -> bool {
    use serde_json::Value;
    let is_stop = |request: &Value| request.get("method").and_then(Value::as_str) == Some(STOP);
    frame.split(|byte| *byte == b'\n').any(|line| {
        if !line.contains(&b'\\')
            && !line
                .windows(STOP.len())
                .any(|window| window == STOP.as_bytes())
        {
            return false;
        }
        serde_json::from_slice::<Value>(line).map_or(true, |value| {
            is_stop(&value)
                || value
                    .as_array()
                    .is_some_and(|batch| batch.iter().any(is_stop))
        })
    })
}

impl Bridge {
    pub(crate) async fn request_stale_turn_stop(
        &self,
        target: &StaleTurnTarget,
        authority: OwnerSession,
    ) -> Result<(), StaleTurnHold> {
        let state = self.stale_turn.as_ref().ok_or(StaleTurnHold::Unsupported)?;
        let observation = self
            .observation
            .as_ref()
            .ok_or(StaleTurnHold::Unsupported)?;
        if self.is_ended()
            || self.output_mode.load(Ordering::Acquire) != OUTPUT_ACKNOWLEDGED
            || target.stream_id.as_deref().is_some_and(|id| {
                uuid::Uuid::parse_str(id).ok() != Some(self.retained_output.stream_id())
            })
        {
            return Err(StaleTurnHold::Unconfirmed);
        }
        let mut stop = state.lock().await;
        let observed = observation.lock().await;
        let prompt = target.prompt();
        if observed.conversation_id.as_deref() != Some(target.conversation_id.as_str())
            || !matches!(&observed.native_state, Some(ConfirmedAgentState::Working { prompt_id }) if prompt_id == prompt.as_str())
            || self.retained_output.recovery_evidence().await.is_none()
        {
            return Err(StaleTurnHold::Unconfirmed);
        }
        if let Some(previous) = stop.as_ref().filter(|previous| {
            previous.prompt == prompt.as_str() && !previous.discarded.load(Ordering::Acquire)
        }) {
            return if previous.matches(target, self.retained_output.stream_id()) {
                Ok(())
            } else {
                Err(StaleTurnHold::Unconfirmed)
            };
        }
        let receipt = uuid::Uuid::new_v4().simple().to_string();
        let frame = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": STOP,
            "params": { "sessionId": target.conversation_id, "dispatchKey": prompt.as_str(), "receipt": receipt }
        })).map_err(|_| StaleTurnHold::Unconfirmed)?;

        // Arm before enqueue. The writer checks the owner again before writing.
        let queued = self
            .input
            .try_reserve()
            .map_err(|_| StaleTurnHold::Unconfirmed)?;
        let discarded = Arc::new(AtomicBool::new(false));
        *stop = Some(Stop {
            target: target.clone(),
            prompt: prompt.as_str().into(),
            receipt,
            expires_at: Instant::now() + RETAIN_FOR,
            cursor: None,
            discarded: Arc::clone(&discarded),
        });
        queued.send(BridgeInput {
            frame,
            authority: Some(authority),
            stop: Some(QueuedStop {
                output: Arc::clone(&self.retained_output),
                discarded,
            }),
        });
        Ok(())
    }

    pub(crate) async fn observe_stale_turn_stop(
        &self,
        target: &StaleTurnTarget,
    ) -> Result<StaleTurnSeal, StaleTurnHold> {
        let state = self.stale_turn.as_ref().ok_or(StaleTurnHold::Unsupported)?;
        let stop = state.lock().await;
        let stop = stop
            .as_ref()
            .filter(|stop| stop.matches(target, self.retained_output.stream_id()))
            .ok_or(StaleTurnHold::Unconfirmed)?;
        let cursor = stop.cursor.ok_or(StaleTurnHold::Unconfirmed)?;
        let observation = self
            .observation
            .as_ref()
            .ok_or(StaleTurnHold::Unsupported)?;
        let observation = observation.lock().await;
        if self.output_mode.load(Ordering::Acquire) != OUTPUT_ACKNOWLEDGED
            || observation.pending_output
            || observation.conversation_id.as_deref() != Some(target.conversation_id.as_str())
            || observation.native_state != Some(ConfirmedAgentState::Idle)
        {
            return Err(StaleTurnHold::Unconfirmed);
        }
        let evidence = self
            .retained_output
            .recovery_evidence()
            .await
            .ok_or(StaleTurnHold::Unconfirmed)?;
        if evidence.boundary.acknowledged_through != cursor
            || evidence.boundary.observed_through != cursor
        {
            return Err(StaleTurnHold::Unconfirmed);
        }
        let stream_id = evidence.boundary.stream_id;
        let output = if cursor == 0 {
            SealedOutput::Empty { stream_id }
        } else {
            let digest = evidence
                .acknowledged_payload_sha256
                .ok_or(StaleTurnHold::Unconfirmed)?;
            SealedOutput::Through {
                source: RuntimeOutputSource {
                    stream_id,
                    cursor,
                    native_conversation_id: target.conversation_id.clone(),
                    payload_sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
                },
            }
        };
        Ok(StaleTurnSeal {
            version: SEAL_VERSION,
            target: target.clone(),
            prompt_id: stop.prompt.clone(),
            stop_receipt_id: stop.receipt.clone(),
            output,
        })
    }
}

pub(super) async fn observe_fence(
    state: &tokio::sync::Mutex<Option<Stop>>,
    output: &OutputRetention,
    observation: &tokio::sync::Mutex<NativeObservation>,
    frame: &[u8],
) -> bool {
    let Some(value) = frame_method(frame, FENCE) else {
        return false;
    };
    if value.get("id").is_some()
        || value.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0")
    {
        return true;
    }
    let Ok(fence) =
        serde_json::from_value::<Fence>(value.get("params").cloned().unwrap_or_default())
    else {
        return true;
    };
    let mut state = state.lock().await;
    if let Some(stop) = state.as_mut().filter(|stop| {
        stop.cursor.is_none()
            && !stop.discarded.load(Ordering::Acquire)
            && stop.expires_at > Instant::now()
            && fence.version == SEAL_VERSION
            && fence.thread_id == stop.target.conversation_id
            && fence.dispatch_key == stop.prompt
            && fence.receipt == stop.receipt
            && fence.dispatch_sequence != 0
            && !fence.turn_id.is_empty()
            && fence.turn_id.len() <= 128
            && !fence.listener_incarnation.is_empty()
            && fence.listener_incarnation.len() <= 128
    }) {
        let mut observation = observation.lock().await;
        if observation.conversation_id.as_deref() == Some(stop.target.conversation_id.as_str())
            && matches!(&observation.native_state, Some(ConfirmedAgentState::Working { prompt_id }) if prompt_id == &stop.prompt)
        {
            if let Some(evidence) = output.recovery_evidence().await {
                stop.cursor = Some(evidence.boundary.observed_through);
                observation.native_state = Some(ConfirmedAgentState::Idle);
                observation.revision = observation.revision.saturating_add(1);
            }
        }
    }
    true
}
