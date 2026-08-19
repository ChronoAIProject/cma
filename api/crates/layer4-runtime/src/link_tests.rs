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

#[test]
fn a_switch_names_the_program_to_restart() {
    // The command is the kernel's own front-end name, carried verbatim; the
    // allowlist check happens when it is acted on, not when it is parsed.
    let switch: ControlFrame =
        serde_json::from_str(r#"{"t":"switch_plane","command":"codex-acp"}"#)
            .expect("a switch parses");
    assert!(matches!(
        switch,
        ControlFrame::SwitchPlane { command } if command == "codex-acp"
    ));
}

#[tokio::test]
async fn a_switch_is_answered_either_way() {
    // #289: the frame reaching the socket says nothing about the program
    // being swapped, so the supervisor answers — and the control plane
    // records an in-place switch only on a positive answer.
    let state = AppState::for_test(&["codex-acp", "codex"]);
    let (replies, mut answers) = mpsc::channel::<String>(CONTROL_REPLIES);

    control(
        r#"{"t":"switch_plane","command":"codex-acp"}"#,
        "sbx_1",
        &state,
        None,
        &replies,
    )
    .await;
    // Asserted on the serialised frame rather than on a Rust value: the two
    // ends agree on what goes over the wire. Key order is not part of that
    // agreement — a workspace build turns on serde_json's preserve_order and
    // a crate build does not — so the frame is parsed and its fields read.
    let accepted: serde_json::Value =
        serde_json::from_str(&answers.recv().await.expect("the switch was answered"))
            .expect("the answer is JSON");
    assert_eq!(accepted["t"], "switch_plane_result");
    assert_eq!(accepted["command"], "codex-acp");
    assert_eq!(accepted["accepted"], true);

    // A command this pod was never told it may run is refused by name, and
    // the refusal is on the wire rather than only in a log line.
    control(
        r#"{"t":"switch_plane","command":"rm"}"#,
        "sbx_1",
        &state,
        None,
        &replies,
    )
    .await;
    let refused: serde_json::Value =
        serde_json::from_str(&answers.recv().await.expect("the refusal was answered"))
            .expect("the refusal is JSON");
    assert_eq!(refused["t"], "switch_plane_result");
    assert_eq!(refused["command"], "rm");
    assert_eq!(refused["accepted"], false);
}
