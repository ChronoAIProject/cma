//! The wire between the control plane and a bot pod's supervisor.
//!
//! Shared machinery, like `sandbox` and `terminal`: no kernel is named here,
//! because every kernel this service hosts is hosted by the same supervisor
//! and reached the same way.
//!
//! The definitions are stated twice — here and in `layer4-runtime` — and that
//! is deliberate rather than an oversight. The two are separately deployed
//! programs, and the control plane must not depend on the bot pod's crate
//! (CLAUDE.md §3). This is a protocol between them, in the same sense an HTTP
//! API is a protocol, and a protocol is written down on both sides.
//!
//! ## Two dialects
//!
//! Pods that predate lanes speak the wire that existed before: binary frames
//! are terminal bytes, and nothing declares anything. Under CLAUDE.md §5 they
//! keep running, unchanged, for as long as their sessions live — so the
//! control plane speaks whichever dialect the pod it reached understands, and
//! learns which that is from the pod's first word.
//!
//! Asking is safe in both directions. A pod that predates lanes ignores the
//! query that requests them, and answers nothing; one that does not sends its
//! declaration before anything else. Nothing is inferred from silence except
//! the older wire, which is the answer that was always true before.

use serde::Deserialize;

/// The wire this control plane speaks.
pub const WIRE_VERSION: u32 = 1;

/// Raw pseudo-terminal bytes.
pub const LANE_TERMINAL: u8 = 0x01;

/// One conversation frame, exactly as the kernel's bridge wrote it.
pub const LANE_CONVERSATION: u8 = 0x02;

/// A gap in the conversation, where frames were dropped rather than kept.
pub const LANE_CONVERSATION_GAP: u8 = 0x03;

/// One resource sample, exactly as the pod read it from its own cgroup.
pub const LANE_TELEMETRY: u8 = 0x04;

/// The word for that lane in an attach query and in a pod's declaration.
///
/// Named on both sides rather than written as a literal in each: the two ends
/// have to agree on one word, and a literal repeated in two crates is how they
/// stop agreeing - the argument `SWITCH_RESULT` already makes in
/// `hosted/link.rs:49-51` and in the pod's `link.rs:122-126`.
///
/// Never sent to a pod that has not declared it serves telemetry. A pod that
/// predates this reads any `lanes` value other than `conv` as the terminal
/// (`layer4-runtime/src/link.rs:79-83`, `:167-171`), so an ungated attach starts
/// a second agent process in a conversation-plane sandbox and hijacks the
/// screen of a terminal-plane one. Support is read from `/readyz` before
/// anything is opened, the way a prelude is (`hosted/mod.rs:438-451`).
pub const LANE_NAME_TELEMETRY: &str = "tel";

/// What a pod says about itself the moment a link opens.
#[derive(Debug, Clone, Deserialize)]
pub struct Ready {
    /// The wire the pod speaks.
    #[serde(rename = "wireVersion", default)]
    pub wire_version: u32,
    /// The lanes it serves, named.
    #[serde(default)]
    pub lanes: Vec<String>,
    /// The conversation the pod is taking over, when it was handed one.
    ///
    /// Read from the pod rather than remembered here. A control plane that
    /// restarted between the handover and taking the session back over
    /// remembers nothing, and a carried session would silently begin again.
    #[serde(default)]
    pub resuming: Option<String>,
}

/// One reading of what a sandbox is using, as its supervisor read it.
///
/// Counters and levels, and nothing derived. The supervisor reports what it
/// can see at one instant; turning two of these into a rate is this layer's
/// work, because only a link that has stayed open holds a predecessor from
/// the same sandbox.
///
/// Every field but `uptime_ms` is optional. An absence means the pod could not
/// read that figure, which is different from reading a zero.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceFrame {
    /// Milliseconds since the supervisor started, from its monotonic clock.
    pub uptime_ms: u64,
    /// Processor time consumed since the sandbox started, in microseconds.
    #[serde(default)]
    pub cpu_usage_usec: Option<u64>,
    /// The processor allowance in thousandths of one core, where one is set.
    #[serde(default)]
    pub cpu_millis_limit: Option<u32>,
    /// Memory in use at the instant of the sample, in bytes.
    #[serde(default)]
    pub memory_bytes_used: Option<u64>,
    /// The memory allowance in bytes, where one is set.
    #[serde(default)]
    pub memory_bytes_limit: Option<u64>,
    /// The most memory in use at any instant since the sandbox started.
    #[serde(default)]
    pub memory_bytes_peak: Option<u64>,
    /// Thousandths of the interval in which every task was memory-stalled.
    #[serde(default)]
    pub stall_permille: Option<u16>,
    /// Times memory usage reached the configured ceiling.
    #[serde(default)]
    pub ceiling_hits: Option<u64>,
    /// Allocations refused since the sandbox started.
    #[serde(default)]
    pub allocation_failures: Option<u64>,
    /// Processes killed for exceeding the memory allowance.
    #[serde(default)]
    pub kills: Option<u64>,
    /// Bytes received across readable non-loopback interfaces.
    #[serde(default)]
    pub received_bytes: Option<u64>,
    /// Bytes sent across readable non-loopback interfaces.
    #[serde(default)]
    pub sent_bytes: Option<u64>,
}

impl Ready {
    /// Read a pod's declaration out of a control frame, if that is what it is.
    ///
    /// Anything else — an unfamiliar control frame, a frame that is not one at
    /// all — is not a declaration and is not read as one. A pod is understood
    /// to speak the older wire until it says otherwise in the one shape that
    /// says so.
    pub fn from_control(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        if value.get("t").and_then(|t| t.as_str()) != Some("ready") {
            return None;
        }
        let ready: Self = serde_json::from_value(value).ok()?;
        (ready.wire_version >= WIRE_VERSION).then_some(ready)
    }

    /// Whether this pod serves a named lane.
    pub fn serves(&self, lane: &str) -> bool {
        self.lanes.iter().any(|l| l == lane)
    }
}

/// Put a payload on a lane.
pub fn frame(lane: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 1);
    out.push(lane);
    out.extend_from_slice(payload);
    out
}

/// Read a lane frame back, or `None` if it carries no lane at all.
pub fn unframe(bytes: &[u8]) -> Option<(u8, &[u8])> {
    bytes.split_first().map(|(lane, rest)| (*lane, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declaration_is_recognised_and_read() {
        let ready = Ready::from_control(r#"{"t":"ready","wireVersion":1,"lanes":["ctl","conv"]}"#)
            .expect("a pod that speaks lanes says so in this shape");
        assert_eq!(ready.wire_version, 1);
        assert!(ready.serves("conv"));
        assert!(!ready.serves("term"));
    }

    #[test]
    fn a_pod_that_does_not_declare_telemetry_does_not_serve_it() {
        let ready = Ready::from_control(r#"{"t":"ready","wireVersion":1,"lanes":["term"]}"#)
            .expect("a pod that speaks lanes says so in this shape");
        assert!(!ready.serves(LANE_NAME_TELEMETRY));
    }

    #[test]
    fn a_pod_that_says_nothing_of_the_kind_is_not_declaring() {
        // Everything else on the control lane is somebody else's business, and
        // reading one of those as a declaration would switch the wire under a
        // pod that never agreed to it.
        assert!(Ready::from_control(r#"{"t":"pong"}"#).is_none());
        assert!(Ready::from_control("not json at all").is_none());
        assert!(Ready::from_control(r#"{"wireVersion":1}"#).is_none());
    }

    #[test]
    fn a_wire_older_than_this_one_is_not_taken_as_agreement() {
        // Conservative in the one direction that is safe: the framing that
        // predates versions is the only reading a pod cannot misunderstand.
        assert!(Ready::from_control(r#"{"t":"ready","wireVersion":0,"lanes":[]}"#).is_none());
    }

    #[test]
    fn a_payload_survives_the_lane_it_travelled_on() {
        let bytes = b"\x1b[2;1Hsecond\x00\xff";
        let carried = frame(LANE_TERMINAL, bytes);
        let (lane, payload) = unframe(&carried).expect("carries a lane");
        assert_eq!(lane, LANE_TERMINAL);
        assert_eq!(payload, bytes);
    }

    #[test]
    fn an_empty_frame_carries_nothing_rather_than_lane_zero() {
        assert!(unframe(&[]).is_none());
    }

    #[test]
    fn the_lane_numbers_match_the_pod_that_serves_them() {
        // Stated twice by necessity — the control plane must not depend on the
        // bot pod's crate — so the values are pinned here against drift. They
        // are a protocol, and a protocol that disagrees with itself is worse
        // than no protocol at all.
        assert_eq!(LANE_TERMINAL, 0x01);
        assert_eq!(LANE_CONVERSATION, 0x02);
        assert_eq!(LANE_CONVERSATION_GAP, 0x03);
        assert_eq!(LANE_TELEMETRY, 0x04);
        assert_eq!(LANE_NAME_TELEMETRY, "tel");
        assert_eq!(WIRE_VERSION, 1);
    }

    #[test]
    fn a_resource_frame_may_carry_only_its_clock() {
        let frame: ResourceFrame =
            serde_json::from_str(r#"{"uptimeMs":17}"#).expect("the clock is required");
        assert_eq!(frame.uptime_ms, 17);
        assert!(frame.cpu_usage_usec.is_none());
        assert!(frame.cpu_millis_limit.is_none());
        assert!(frame.memory_bytes_used.is_none());
        assert!(frame.memory_bytes_limit.is_none());
        assert!(frame.memory_bytes_peak.is_none());
        assert!(frame.stall_permille.is_none());
        assert!(frame.ceiling_hits.is_none());
        assert!(frame.allocation_failures.is_none());
        assert!(frame.kills.is_none());
        assert!(frame.received_bytes.is_none());
        assert!(frame.sent_bytes.is_none());
    }

    #[test]
    fn a_newer_resource_frame_does_not_hide_fields_this_build_knows() {
        let frame: ResourceFrame = serde_json::from_str(
            r#"{"uptimeMs":17,"aCounterThisBuildDoesNotKnow":91,"memoryBytesUsed":23}"#,
        )
        .expect("unknown fields are forward-compatible");
        assert_eq!(frame.uptime_ms, 17);
        assert_eq!(frame.memory_bytes_used, Some(23));
    }

    #[test]
    fn every_resource_frame_wire_name_reaches_its_field() {
        let frame: ResourceFrame = serde_json::from_value(serde_json::json!({
            "uptimeMs": 2_000,
            "cpuUsageUsec": 12_000_000,
            "cpuMillisLimit": 1_000,
            "memoryBytesUsed": 402_653_184,
            "memoryBytesLimit": 1_073_741_824_u64,
            "memoryBytesPeak": 530_579_456,
            "stallPermille": 15,
            "ceilingHits": 3,
            "allocationFailures": 1,
            "kills": 2,
            "receivedBytes": 8_388_608,
            "sentBytes": 2_097_152,
        }))
        .expect("all twelve contract fields deserialize");

        assert_eq!(frame.uptime_ms, 2_000);
        assert_eq!(frame.cpu_usage_usec, Some(12_000_000));
        assert_eq!(frame.cpu_millis_limit, Some(1_000));
        assert_eq!(frame.memory_bytes_used, Some(402_653_184));
        assert_eq!(frame.memory_bytes_limit, Some(1_073_741_824));
        assert_eq!(frame.memory_bytes_peak, Some(530_579_456));
        assert_eq!(frame.stall_permille, Some(15));
        assert_eq!(frame.ceiling_hits, Some(3));
        assert_eq!(frame.allocation_failures, Some(1));
        assert_eq!(frame.kills, Some(2));
        assert_eq!(frame.received_bytes, Some(8_388_608));
        assert_eq!(frame.sent_bytes, Some(2_097_152));
    }
}
