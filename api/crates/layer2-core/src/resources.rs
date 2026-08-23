//! Resource use in the core's own vocabulary.

use std::sync::Arc;
use std::time::Instant;

use layer3_adapter::{AdapterError, KernelAdapter, RuntimeHandle};
use tokio::sync::mpsc;

use crate::ids::SessionId;
use crate::service::{SessionService, RESOURCE_BUFFER};

/// What a session is using.
///
/// Every field is optional because an unavailable level, an absent limit, and
/// a rate without a predecessor are all different from zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceReading {
    /// Processor use over `interval_ms`, in thousandths of one core.
    pub cpu_millis_used: Option<u32>,
    /// The processor allowance in thousandths of one core.
    pub cpu_millis_limit: Option<u32>,
    /// Memory in use at the instant of the reading, in bytes.
    pub memory_bytes_used: Option<u64>,
    /// The memory allowance in bytes.
    pub memory_bytes_limit: Option<u64>,
    /// The highest recorded memory use, in bytes.
    pub memory_bytes_peak: Option<u64>,
    /// Thousandths of the interval in which every task was memory-stalled.
    pub stall_permille: Option<u16>,
    /// Times this session's memory was about to exceed its ceiling.
    pub ceiling_hits: Option<u64>,
    /// Allocations refused since the session started.
    pub allocation_failures: Option<u64>,
    /// Processes killed for exceeding the memory allowance.
    pub kills: Option<u64>,
    /// Bytes received since the session started.
    pub received_bytes: Option<u64>,
    /// Bytes sent since the session started.
    pub sent_bytes: Option<u64>,
    /// Received bytes per second over `interval_ms`.
    pub received_bytes_per_second: Option<u64>,
    /// Sent bytes per second over `interval_ms`.
    pub sent_bytes_per_second: Option<u64>,
    /// The interval over which the three rates were derived, in milliseconds.
    pub interval_ms: Option<u64>,
}

/// The most recent reading a session produced, and when it reached the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldReading {
    pub reading: ResourceReading,
    /// RFC 3339 in UTC, used only as the moment shown to a reader.
    pub observed_at: String,
}

/// What is known about a session's resource use right now.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // The public contract carries HeldReading whole.
pub enum ResourceStatus {
    /// A reading was taken: the newest one and when it arrived.
    Held(HeldReading),
    /// Reading was started and nothing has arrived yet.
    Awaiting,
    /// This session cannot report what it is using.
    Unreported,
}

/// Translate a lower-layer sample into the core's vocabulary.
///
/// This is a field-for-field move: it computes nothing and preserves every
/// absence.
pub(crate) fn reading_from(sample: layer3_adapter::ResourceSample) -> ResourceReading {
    ResourceReading {
        cpu_millis_used: sample.cpu_millis_used,
        cpu_millis_limit: sample.cpu_millis_limit,
        memory_bytes_used: sample.memory_bytes_used,
        memory_bytes_limit: sample.memory_bytes_limit,
        memory_bytes_peak: sample.memory_bytes_peak,
        stall_permille: sample.stall_permille,
        ceiling_hits: sample.ceiling_hits,
        allocation_failures: sample.allocation_failures,
        kills: sample.kills,
        received_bytes: sample.received_bytes,
        sent_bytes: sample.sent_bytes,
        received_bytes_per_second: sample.received_bytes_per_second,
        sent_bytes_per_second: sample.sent_bytes_per_second,
        interval_ms: sample.interval_ms,
    }
}

/// Reduce one reading to the memory fields consumed by the pressure fold.
#[allow(clippy::question_mark)] // The absence guard is the fold's first rule.
pub(crate) fn pressure_sample(
    reading: &ResourceReading,
    at: Instant,
) -> Option<crate::pressure::PressureSample> {
    let Some(used) = reading.memory_bytes_used else {
        return None;
    };
    let (limit_bytes, occupancy_permille) = match reading.memory_bytes_limit {
        Some(limit) if limit > 0 => {
            let occupancy =
                (u128::from(used) * 1_000 / u128::from(limit)).min(u128::from(u16::MAX)) as u16;
            (Some(limit), Some(occupancy))
        }
        _ => (None, None),
    };
    Some(crate::pressure::PressureSample {
        occupancy_permille,
        used_bytes: used,
        limit_bytes,
        at,
    })
}

/// Watch a session for as long as it is this process's to watch.
///
/// Each reading takes the entries write lock once for a field assignment. No
/// I/O occurs and no await is held under that guard.
pub(crate) fn spawn_resource_watch(
    service: Arc<SessionService>,
    id: SessionId,
    adapter: Arc<dyn KernelAdapter>,
    handle: RuntimeHandle,
) {
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::channel(RESOURCE_BUFFER);
        match adapter.watch_resources(&handle, tx).await {
            Ok(()) => {}
            Err(AdapterError::Unsupported(reason)) => {
                if let Some(entry) = service.entries.write().await.get_mut(&id) {
                    entry.readings_unreported = true;
                }
                tracing::info!(
                    session_id = %id,
                    %reason,
                    "this session does not report what it is using"
                );
                return;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "could not start watching what this session is using"
                );
                return;
            }
        }

        while let Some(sample) = rx.recv().await {
            let reading = reading_from(sample);
            let pressure = pressure_sample(&reading, Instant::now());
            let resources = {
                let mut entries = service.entries.write().await;
                let Some(entry) = entries.get_mut(&id) else {
                    break;
                };
                entry.last_reading = Some(HeldReading {
                    reading: reading.clone(),
                    observed_at: crate::clock::now_rfc3339(),
                });
                entry.resources.clone()
            };
            let _ = resources.send(reading);
            if let Some(sample) = pressure {
                service.note_memory_pressure(&id, sample).await;
            }
        }

        tracing::info!(session_id = %id, "resource reading watch ended");
    });
}

impl SessionService {
    /// The last reading held for a session, if any was ever held.
    ///
    /// There is no caller because reconciliation establishes facts across all
    /// owners without a request on the line. `None` remains an honest absence;
    /// it is never replaced with an all-zero reading.
    ///
    /// Read this before removing the live entry. The hold exists only in this
    /// process and deliberately disappears with it.
    #[allow(dead_code)] // Consumed by the reconciliation event that follows this issue.
    pub(crate) async fn held_reading(&self, id: &SessionId) -> Option<HeldReading> {
        self.entries
            .read()
            .await
            .get(id)
            .and_then(|entry| entry.last_reading.clone())
    }
}
