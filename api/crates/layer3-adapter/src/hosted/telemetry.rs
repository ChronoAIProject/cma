//! Watching resource counters from a hosted runtime.
//!
//! A watch is opened only after the pod declares support on readiness. Rates
//! are derived here from consecutive frames on one link, so a predecessor can
//! never survive the sandbox that produced it.

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::{HostedKernel, READY_PATH, SUPERVISOR_PORT};
use crate::supervisor;
use crate::{AdapterError, ResourceSample, RuntimeHandle};

impl HostedKernel {
    /// Whether this pod's supervisor can report what its sandbox is using.
    ///
    /// Discovered before a link is opened. A pod that predates telemetry
    /// treats an unfamiliar attachment as a terminal and may resize or start
    /// its live pty, so attempting the socket is not a safe capability probe.
    async fn reports_resources(&self, endpoint: &str) -> bool {
        let Ok(body) = self.client.get_from_sandbox(endpoint, READY_PATH).await else {
            return false;
        };
        serde_json::from_slice::<serde_json::Value>(body.unwrap_or_default().as_slice())
            .ok()
            .and_then(|value| value.get("resources").and_then(serde_json::Value::as_bool))
            .unwrap_or(false)
    }

    /// Start relaying this runtime's resource readings to a caller.
    pub async fn watch_resources(
        &self,
        handle: &RuntimeHandle,
        readings: mpsc::Sender<ResourceSample>,
    ) -> Result<(), AdapterError> {
        let Some(interval_ms) = self.shape.telemetry_interval_ms else {
            return Err(AdapterError::Unsupported(
                "this deployment does not ask its runtimes for readings".to_string(),
            ));
        };

        // Resolve once and use the same endpoint for discovery and attach. A
        // reschedule between two resolutions could prove one pod and open the
        // socket on another.
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        if !self.reports_resources(&endpoint.endpoint).await {
            return Err(AdapterError::Unsupported(format!(
                "runtime {} cannot report what it is using",
                handle.0
            )));
        }

        let extra_query = format!("&telemetry={interval_ms}");
        let (stream, url) = self
            .connect_at(handle, &endpoint.endpoint, &extra_query)
            .await?;
        let (mut sink, mut source) = stream.split();
        let (framed, early) = Self::negotiate(&mut source).await;
        if !framed {
            if let Some(bytes) = early {
                tracing::warn!(
                    sandbox = %handle.0,
                    bytes = bytes.len(),
                    "terminal output arrived on a requested telemetry link"
                );
            }
            let _ = sink.close().await;
            return Err(AdapterError::Unsupported(format!(
                "runtime {} negotiated an unframed link that cannot carry telemetry",
                handle.0
            )));
        }
        drop(sink);

        tracing::info!(
            sandbox = %handle.0,
            interval_ms,
            %url,
            "watching sandbox resources"
        );
        let sandbox = handle.0.clone();
        tokio::spawn(async move {
            // Link-local by construction. This predecessor is dropped with
            // the socket and can never be differenced against a new sandbox.
            let mut previous: Option<supervisor::ResourceFrame> = None;
            while let Some(frame) = source.next().await {
                match frame {
                    Ok(Message::Binary(bytes)) => match supervisor::unframe(&bytes) {
                        Some((supervisor::LANE_TELEMETRY, payload)) => {
                            let Ok(now) =
                                serde_json::from_slice::<supervisor::ResourceFrame>(payload)
                            else {
                                tracing::warn!(%sandbox, "a reading this build cannot read");
                                continue;
                            };
                            let sample = derive_sample(previous.as_ref(), &now);
                            previous = Some(now);
                            if readings.send(sample).await.is_err() {
                                break;
                            }
                        }
                        Some((lane, _)) => {
                            tracing::warn!(
                                %sandbox,
                                lane,
                                "frame on a lane this attachment does not carry"
                            );
                        }
                        None => {}
                    },
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%sandbox, %error, "reading link read failed");
                        break;
                    }
                }
            }
            tracing::info!(%sandbox, "reading relay ended");
        });
        Ok(())
    }
}

/// Turn a frame into a sample, deriving rates against one predecessor.
///
/// This is the only place a rate is derived. A counter remains useful after a
/// dropped frame; a quotient does not, and deriving at both ends would let two
/// programs silently disagree about the interval.
pub(super) fn derive_sample(
    previous: Option<&supervisor::ResourceFrame>,
    now: &supervisor::ResourceFrame,
) -> ResourceSample {
    let mut sample = ResourceSample {
        cpu_millis_used: None,
        cpu_millis_limit: now.cpu_millis_limit,
        memory_bytes_used: now.memory_bytes_used,
        memory_bytes_limit: now.memory_bytes_limit,
        memory_bytes_peak: now.memory_bytes_peak,
        stall_permille: now.stall_permille,
        ceiling_hits: now.ceiling_hits,
        allocation_failures: now.allocation_failures,
        kills: now.kills,
        received_bytes: now.received_bytes,
        sent_bytes: now.sent_bytes,
        received_bytes_per_second: None,
        sent_bytes_per_second: None,
        interval_ms: None,
    };

    let Some(previous) = previous else {
        return sample;
    };
    let Some(elapsed_ms) = now
        .uptime_ms
        .checked_sub(previous.uptime_ms)
        .filter(|elapsed| *elapsed > 0)
    else {
        tracing::debug!(
            previous_uptime_ms = previous.uptime_ms,
            uptime_ms = now.uptime_ms,
            "resource frame had no forward interval"
        );
        return sample;
    };
    sample.interval_ms = Some(elapsed_ms);

    sample.cpu_millis_used = counter_delta(
        previous.cpu_usage_usec,
        now.cpu_usage_usec,
        "cpu_usage_usec",
    )
    .map(|used| u32::try_from(used / elapsed_ms).unwrap_or(u32::MAX));
    sample.received_bytes_per_second = counter_delta(
        previous.received_bytes,
        now.received_bytes,
        "received_bytes",
    )
    .map(|bytes| bytes.saturating_mul(1_000) / elapsed_ms);
    sample.sent_bytes_per_second = counter_delta(previous.sent_bytes, now.sent_bytes, "sent_bytes")
        .map(|bytes| bytes.saturating_mul(1_000) / elapsed_ms);
    sample
}

fn counter_delta(previous: Option<u64>, now: Option<u64>, counter: &'static str) -> Option<u64> {
    let (Some(previous), Some(now)) = (previous, now) else {
        return None;
    };
    match now.checked_sub(previous) {
        Some(delta) => Some(delta),
        None => {
            tracing::debug!(counter, previous, now, "resource counter went backwards");
            None
        }
    }
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod telemetry_tests;
