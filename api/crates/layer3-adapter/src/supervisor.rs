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
        assert_eq!(WIRE_VERSION, 1);
    }
}
