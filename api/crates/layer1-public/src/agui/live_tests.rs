//! What a client is handed while a turn is in flight.

use super::*;

#[test]
fn a_piece_of_a_message_carries_no_cursor() {
    // The rule the whole lane turns on. A client resumes on the SSE id; a
    // liveness frame is not in the log, so a frame given one would send a
    // reconnecting client to a position the log has never held, and every
    // event between there and the real position would be skipped.
    let frame = project_live(
        &LiveFrame::MessageDelta("thinking about".into()),
        "run_1",
        "sesn_1",
    )
    .expect("a message being typed has a shape here");

    assert_eq!(frame.kind, "TEXT_MESSAGE_CONTENT");
    assert!(
        frame.cursor().is_none(),
        "a liveness frame must never travel under an id"
    );
}

#[test]
fn a_liveness_frame_says_on_the_wire_that_it_has_no_cursor() {
    // A client cannot see the absence otherwise: `EventSource` reports
    // `lastEventId` as the last id it was *given*, so a frame sent without one
    // looks exactly like a frame repeating the previous id — and a client
    // deduplicating on that drops every liveness frame silently.
    let frame = project_live(&LiveFrame::MessageDelta("half".into()), "run_1", "sesn_1")
        .expect("has a shape");
    let body = serde_json::to_value(&frame).expect("serialises");

    assert_eq!(body["chrono:live"], true, "{body}");
    assert!(
        body.get("seq").is_none(),
        "and never as something that looks like a position: {body}"
    );
}

#[test]
fn a_durable_frame_does_not_claim_to_be_liveness() {
    let projected = super::super::project::project(
        &layer2_core::Event {
            session_id: layer2_core::SessionId("sesn_1".into()),
            seq: 4,
            kind: layer2_core::EventKind::TurnStarted,
        },
        "run_1",
        "sesn_1",
    );
    let body = serde_json::to_value(&projected.events[0]).expect("serialises");

    assert!(
        body.get("chrono:live").is_none(),
        "a frame with a cursor carries no marker at all: {body}"
    );
}

#[test]
fn the_in_flight_message_is_not_the_committed_one() {
    // They must be two identities. A client upserts by message id, so if the
    // in-flight pieces and the committed message shared one, the committed
    // message would be appended to what was already drawn and the turn would
    // read twice.
    let live = project_live(&LiveFrame::MessageDelta("half".into()), "run_1", "sesn_1")
        .expect("has a shape");
    let committed = super::super::project::project(
        &layer2_core::Event {
            session_id: layer2_core::SessionId("sesn_1".into()),
            seq: 4,
            kind: layer2_core::EventKind::MessageCommitted {
                content: "half a thought, finished".into(),
            },
        },
        "run_1",
        "sesn_1",
    );

    assert_ne!(
        live.body["messageId"], committed.events[0].body["messageId"],
        "the in-flight message and the committed one must be different identities"
    );
}

#[test]
fn successive_pieces_of_one_message_land_on_one_entry() {
    let first = project_live(&LiveFrame::MessageDelta("Hel".into()), "run_1", "sesn_1")
        .expect("has a shape");
    let second = project_live(&LiveFrame::MessageDelta("lo".into()), "run_1", "sesn_1")
        .expect("has a shape");

    assert_eq!(first.body["messageId"], second.body["messageId"]);
    assert_eq!(first.body["delta"], "Hel");
    assert_eq!(second.body["delta"], "lo");
}

#[test]
fn what_this_protocol_has_no_shape_for_becomes_nothing() {
    // A terminal's bytes are a terminal fact. A runtime exiting is already
    // recorded durably as a status change the run stream carries in its own
    // right, and saying it twice would end one run twice.
    assert!(project_live(&LiveFrame::TerminalOutput(vec![27, 91, 50, 74]), "r", "s").is_none());
    assert!(project_live(
        &LiveFrame::Exited {
            code: Some(0),
            reason: layer2_core::StopReason::UserTerminated,
        },
        "r",
        "s"
    )
    .is_none());
}
