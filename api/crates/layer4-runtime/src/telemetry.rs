//! Serve this pod's resource readings on their own supervisor lane.

use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};

use crate::lanes;
use crate::link::Sink;
use crate::AppState;

/// The narrowest cadence this pod will serve.
///
/// The interval crosses a trust boundary as a query parameter, so it is
/// clamped rather than trusted: at 1 ms a caller could spend this pod's
/// processor on reading its own cgroup, which is the opposite of what a health
/// signal is for.
pub(crate) const MIN_INTERVAL_MS: u64 = 250;
/// The widest cadence this pod will serve.
pub(crate) const MAX_INTERVAL_MS: u64 = 60_000;

pub(crate) fn clamp_interval_ms(requested: u64) -> u64 {
    requested.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS)
}

/// Send resource readings until either side of the link closes.
pub(crate) async fn relay_telemetry(
    mut sink: Sink,
    mut source: futures_util::stream::SplitStream<WebSocket>,
    state: AppState,
    id: String,
    interval_ms: u64,
) -> std::io::Result<()> {
    let served_interval_ms = clamp_interval_ms(interval_ms);
    if served_interval_ms != interval_ms {
        tracing::debug!(runtime = %id, requested = interval_ms, served = served_interval_ms, "resource cadence clamped");
    }
    tracing::info!(runtime = %id, interval_ms = served_interval_ms, "resource telemetry attached");

    let mut interval = tokio::time::interval(Duration::from_millis(served_interval_ms));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let sample = state.resources.sample(Instant::now());
                let payload = match serde_json::to_vec(&sample) {
                    Ok(payload) => payload,
                    Err(error) => {
                        tracing::warn!(runtime = %id, %error, "resource sample cannot be serialised");
                        continue;
                    }
                };
                let framed = lanes::frame(lanes::LANE_TELEMETRY, &payload);
                if sink.send(Message::Binary(framed.into())).await.is_err() {
                    break;
                }
            }
            frame = source.next() => match frame {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => {
                    tracing::warn!(runtime = %id, %error, "resource telemetry read failed");
                    break;
                }
                Some(Ok(_)) => {
                    tracing::debug!(runtime = %id, "inbound frame ignored on a resource telemetry link");
                }
            },
        }
    }
    tracing::info!(runtime = %id, "resource telemetry detached");
    Ok(())
}
