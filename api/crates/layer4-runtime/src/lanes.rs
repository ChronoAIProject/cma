//! The lanes this pod's link carries, and how they are told apart.
//!
//! One WebSocket, three kinds of traffic. Control is ready-and-capabilities,
//! resize and credential; conversation is whatever the kernel's bridge says,
//! carried verbatim; terminal is raw pseudo-terminal bytes. They share a
//! socket because a pod has one address and one authenticated path back to the
//! control plane, and they are kept distinct because the control plane has to
//! know what it is looking at without guessing.
//!
//! Control travels as text frames, which is what it already was: a JSON object
//! with a `t` tag. Everything else travels as binary with a one-byte lane
//! discriminator in front. That keeps a conversation frame opaque — this pod
//! never reads one, which is what stops a kernel version changing what a frame
//! looks like from breaking the pod hosting it — and it keeps terminal bytes
//! exactly as they were, one byte further along.
//!
//! ## Why a version, and why it is announced first
//!
//! A pod that predates all of this sends unprefixed binary and never sends a
//! ready frame. Under CLAUDE.md §5 those pods keep running, unchanged, through
//! this rollout and for as long as their sessions live. So the first thing a
//! new pod says is which wire it speaks; a control plane that hears nothing
//! concludes the old one, which is the answer that was always true before.

use serde::{Deserialize, Serialize};

/// The wire this build speaks.
///
/// Bumped when a lane's framing changes in a way an existing control plane
/// would misread. Adding a lane to the list below is not such a change: a
/// control plane ignores lanes it has no use for.
pub const WIRE_VERSION: u32 = 1;

/// Raw pseudo-terminal bytes.
pub const LANE_TERMINAL: u8 = 0x01;

/// One conversation frame, exactly as the kernel's bridge wrote it, with the
/// newline that delimited it removed.
pub const LANE_CONVERSATION: u8 = 0x02;

/// A gap in the conversation, where frames were dropped rather than kept.
///
/// Carried in the conversation's place rather than announced separately,
/// because where the gap falls is the useful part: the control plane records a
/// boundary its owner can be shown, instead of a total that says something is
/// missing without saying when.
pub const LANE_CONVERSATION_GAP: u8 = 0x03;

/// One resource sample this pod read from its own cgroup and interfaces.
///
/// Declared before anything sends one, so the number is agreed between the two
/// programs before either uses it.
pub const LANE_TELEMETRY: u8 = 0x04;

/// The word for that lane in an attach query and in this pod's declaration.
///
/// One word, agreed at both ends, for the same reason `SWITCH_RESULT`
/// (`link.rs:122-126`) is one word: a literal repeated in two crates is how they
/// stop agreeing.
pub const LANE_NAME_TELEMETRY: &str = "tel";

/// What this pod tells a control plane the moment one connects.
///
/// Sent before anything else, so a control plane can decide how to read every
/// frame that follows from the first one it receives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ready {
    /// Always `ready`; the tag every control frame carries.
    pub t: String,
    /// The wire this pod speaks.
    #[serde(rename = "wireVersion")]
    pub wire_version: u32,
    /// The lanes this pod serves, named.
    pub lanes: Vec<String>,
    /// The conversation this pod is taking over, when it was handed one.
    ///
    /// Said by the pod because the pod is what received it. A control plane
    /// that restarted between the handover and this moment remembers nothing,
    /// and a session carried onto a new runtime would silently begin again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resuming: Option<String>,
}

impl Ready {
    /// The declaration for a pod serving these lanes.
    pub fn declaring(lanes: &[&str]) -> Self {
        Self {
            t: "ready".to_string(),
            wire_version: WIRE_VERSION,
            lanes: lanes.iter().map(|l| l.to_string()).collect(),
            resuming: None,
        }
    }

    /// Say which conversation this pod is taking over.
    pub fn resuming(mut self, conversation: Option<String>) -> Self {
        self.resuming = conversation;
        self
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
///
/// An empty frame is not a lane-less payload — it is a frame with nothing in
/// it, which nothing here produces and nothing should act on.
pub fn unframe(bytes: &[u8]) -> Option<(u8, &[u8])> {
    bytes.split_first().map(|(lane, rest)| (*lane, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_survives_the_lane_it_travelled_on() {
        let carried = frame(LANE_CONVERSATION, br#"{"jsonrpc":"2.0","id":1}"#);
        let (lane, payload) = unframe(&carried).expect("a lane frame carries a lane");
        assert_eq!(lane, LANE_CONVERSATION);
        assert_eq!(payload, br#"{"jsonrpc":"2.0","id":1}"#);
    }

    #[test]
    fn a_resource_sample_survives_the_telemetry_lane() {
        let bytes = br#"{"sampledAtMs":42}"#;
        let carried = frame(LANE_TELEMETRY, bytes);
        let (lane, payload) = unframe(&carried).expect("a lane frame carries a lane");
        assert_eq!(lane, LANE_TELEMETRY);
        assert_eq!(payload, bytes);
    }

    #[test]
    fn terminal_bytes_are_unchanged_one_byte_along() {
        // The whole terminal plane rests on this: what the pty wrote is what
        // the client renders, byte for byte.
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
    fn a_lane_with_no_payload_is_still_that_lane() {
        let carried = frame(LANE_CONVERSATION_GAP, b"");
        let (lane, payload) = unframe(&carried).expect("carries a lane");
        assert_eq!(lane, LANE_CONVERSATION_GAP);
        assert!(payload.is_empty());
    }

    #[test]
    fn every_lane_has_a_distinct_discriminator() {
        let lanes = [
            LANE_TERMINAL,
            LANE_CONVERSATION,
            LANE_CONVERSATION_GAP,
            LANE_TELEMETRY,
        ];
        let mut sorted = lanes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), lanes.len(), "two lanes would be one lane");
    }

    #[test]
    fn the_lane_numbers_match_the_control_plane_that_reads_them() {
        // Stated twice by necessity - the control plane must not depend on this
        // crate - and pinned here as well as there. Distinctness alone is not
        // drift protection: renumbering a lane on one side only would leave
        // both suites green and the two programs disagreeing.
        assert_eq!(LANE_TERMINAL, 0x01);
        assert_eq!(LANE_CONVERSATION, 0x02);
        assert_eq!(LANE_CONVERSATION_GAP, 0x03);
        assert_eq!(LANE_TELEMETRY, 0x04);
        assert_eq!(LANE_NAME_TELEMETRY, "tel");
        assert_eq!(WIRE_VERSION, 1);
    }

    #[test]
    fn the_ready_frame_names_the_wire_and_its_lanes() {
        // Read by a control plane to decide how every later frame is framed,
        // so the field names are contract.
        let json = serde_json::to_value(Ready::declaring(&["ctl", "term"])).expect("serialises");
        assert_eq!(json["t"], "ready");
        assert_eq!(json["wireVersion"], WIRE_VERSION);
        assert_eq!(json["lanes"][0], "ctl");
        assert_eq!(json["lanes"][1], "term");
    }
}
