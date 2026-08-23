//! Resource use in the core's own vocabulary.

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
