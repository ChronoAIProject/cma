//! Projecting and merging a session's live resource readings.
//!
//! Readings have their own broadcast lane in the core so a gauge cannot use
//! the lag budget reserved for message deltas. Both AG-UI streams merge that
//! lane through this module, which keeps their drop and closure semantics the
//! same.

use std::time::Duration;

use layer2_core::{LiveFrame, ResourceReading};
use tokio::sync::broadcast::{error::RecvError, Receiver};

use super::project::AguiEvent;

/// How long either stream waits before polling the durable log again.
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Which live lane woke a stream, or that neither did before its next poll.
#[derive(Debug, Clone)]
pub(super) enum Woke {
    Live(LiveFrame),
    Reading(ResourceReading),
    Idle,
}

/// Wait for either live lane without letting one starve the other.
///
/// Falling behind loses only liveness, so it is logged and the next available
/// frame is read. A closed lane is removed while the other remains usable.
pub(super) async fn next_frame(
    live: &mut Option<Receiver<LiveFrame>>,
    resources: &mut Option<Receiver<ResourceReading>>,
) -> Woke {
    loop {
        let has_live = live.is_some();
        let has_resources = resources.is_some();
        if !has_live && !has_resources {
            tokio::time::sleep(POLL_INTERVAL).await;
            return Woke::Idle;
        }

        tokio::select! {
            received = receive(live), if has_live => match received {
                Ok(frame) => return Woke::Live(frame),
                Err(RecvError::Lagged(frames)) => {
                    tracing::debug!(frames, lane = "conversation", "an AG-UI watcher fell behind on liveness");
                }
                Err(RecvError::Closed) => *live = None,
            },
            received = receive(resources), if has_resources => match received {
                Ok(reading) => return Woke::Reading(reading),
                Err(RecvError::Lagged(frames)) => {
                    tracing::debug!(frames, lane = "resources", "an AG-UI watcher fell behind on liveness");
                }
                Err(RecvError::Closed) => *resources = None,
            },
        }
    }
}

/// Receive from a lane whose `tokio::select!` guard established its presence.
async fn receive<T: Clone>(receiver: &mut Option<Receiver<T>>) -> Result<T, RecvError> {
    receiver
        .as_mut()
        .expect("the select guard established this receiver")
        .recv()
        .await
}

/// Project one reading as a state patch.
///
/// A reading supersedes its predecessor whole. One replace is atomic, heals
/// after a dropped frame on the next reading, and remains idempotent when a
/// frame is replayed. A set of field patches could tear across a drop.
///
/// `STATE_SNAPSHOT` is not offered, so this path must be reconstructible from
/// replaces alone. Every absence is an explicit `null`: an unread figure, an
/// absent ceiling, and a rate without a predecessor are none of them zero.
pub(super) fn project_reading(
    reading: &ResourceReading,
    run_id: &str,
    thread_id: &str,
) -> AguiEvent {
    AguiEvent {
        kind: "STATE_DELTA",
        run_id: run_id.to_string(),
        thread_id: thread_id.to_string(),
        body: serde_json::json!({ "delta": [{
            "op": "replace",
            "path": "/resources",
            "value": {
                "cpuMillisUsed": reading.cpu_millis_used,
                "cpuMillisLimit": reading.cpu_millis_limit,
                "memoryBytesUsed": reading.memory_bytes_used,
                "memoryBytesLimit": reading.memory_bytes_limit,
                "memoryBytesPeak": reading.memory_bytes_peak,
                "stallPermille": reading.stall_permille,
                "ceilingHits": reading.ceiling_hits,
                "allocationFailures": reading.allocation_failures,
                "kills": reading.kills,
                "receivedBytes": reading.received_bytes,
                "sentBytes": reading.sent_bytes,
                "receivedBytesPerSecond": reading.received_bytes_per_second,
                "sentBytesPerSecond": reading.sent_bytes_per_second,
                "intervalMs": reading.interval_ms,
            },
        }] }),
        timestamp: None,
        // A gauge is liveness, not a position in the durable log. Giving it a
        // sequence would make a reconnect resume from a cursor the log lacks.
        seq: None,
        live: true,
    }
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod resources_tests;
