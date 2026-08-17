//! What a link says about itself, and how a frame is read on it.
//!
//! The wire is the contract between a pod and a control plane that may have
//! been deployed at a different time. Everything here is about the two of them
//! agreeing without having to guess.

use super::*;

/// A link opened with these query values, as the control plane would.
fn params(wire: Option<u32>, lanes: Option<&str>) -> AttachParams {
    AttachParams {
        cols: Some(120),
        rows: Some(40),
        wire,
        lanes: lanes.map(str::to_string),
    }
}

#[test]
fn a_control_plane_that_says_nothing_gets_the_wire_it_had() {
    // The §5 half that is easy to forget: a control plane deployed before
    // lanes existed opens links with no `wire`, and must keep getting
    // unprefixed binary. A pod that framed anyway would write a discriminator
    // into somebody's terminal as a typed character.
    let p = params(None, None);
    assert!(!p.framed());
    assert!(!p.conversational());
}

#[test]
fn asking_for_the_lane_wire_is_what_turns_framing_on() {
    let p = params(Some(1), None);
    assert!(p.framed());
    assert!(
        !p.conversational(),
        "the terminal is still the default lane"
    );
}

#[test]
fn a_wire_older_than_this_pod_speaks_is_treated_as_the_legacy_one() {
    // Conservative on purpose: the only safe reading of a version this pod
    // does not implement is the framing that predates versions.
    assert!(!params(Some(0), None).framed());
}

#[test]
fn the_conversation_lane_is_asked_for_by_name() {
    let p = params(Some(1), Some("conv"));
    assert!(p.framed());
    assert!(p.conversational());
}

#[test]
fn an_unfamiliar_lane_name_is_not_the_conversation() {
    // Nothing is inferred from a name this pod does not serve; it drives the
    // terminal, which is what a link has always been for.
    assert!(!params(Some(1), Some("something-else")).conversational());
}

#[test]
fn a_gap_says_how_many_frames_fell_in_it() {
    // The control plane records a boundary its owner can be shown, so the
    // count has to survive the crossing.
    let Message::Binary(bytes) = gap(17) else {
        panic!("a gap travels as binary, on its own lane");
    };
    let (lane, payload) = lanes::unframe(&bytes).expect("carries a lane");
    assert_eq!(lane, LANE_CONVERSATION_GAP);
    let parsed: serde_json::Value = serde_json::from_slice(payload).expect("the marker is json");
    assert_eq!(parsed["frames"], 17);
}

#[test]
fn a_conversation_frame_crosses_on_its_own_lane_untouched() {
    let said = br#"{"jsonrpc":"2.0","method":"session/update"}"#;
    let Message::Binary(bytes) = outbound(Outbound::Frame(said.to_vec())) else {
        panic!("a conversation frame travels as binary");
    };
    let (lane, payload) = lanes::unframe(&bytes).expect("carries a lane");
    assert_eq!(lane, LANE_CONVERSATION);
    assert_eq!(payload, said, "a frame must not be reshaped in transit");
}

#[test]
fn the_control_vocabulary_still_parses_from_its_tag() {
    // Unchanged from the single-lane wire, which is what lets a pod that
    // predates lanes and one that does not accept the same control frames.
    let credential: ControlFrame =
        serde_json::from_str(r#"{"t":"credential","value":"x"}"#).expect("credential parses");
    assert!(matches!(credential, ControlFrame::Credential { .. }));
    let resize: ControlFrame =
        serde_json::from_str(r#"{"t":"resize","cols":80,"rows":24}"#).expect("resize parses");
    assert!(matches!(
        resize,
        ControlFrame::Resize { cols: 80, rows: 24 }
    ));
    let signal: ControlFrame =
        serde_json::from_str(r#"{"t":"signal","sig":"INT"}"#).expect("signal parses");
    assert!(matches!(signal, ControlFrame::Signal { .. }));
    assert!(serde_json::from_str::<ControlFrame>(r#"{"t":"nope"}"#).is_err());
}
