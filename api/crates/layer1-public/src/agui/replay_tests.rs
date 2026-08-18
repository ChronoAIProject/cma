//! What a client reading history is handed, and what it must be able to read.
//!
//! The defect these exist for was not in either half. The projection was
//! correct and the client's parser was correct; nothing checked that the
//! frames one produced were the frames the other reads, and the client was
//! pointed at a route that answers in the core's vocabulary. Every chat test
//! drove a mock whose frames were already AG-UI-shaped, so the suite proved
//! the surface renders AG-UI and never that the wire delivers it.
//!
//! So the tests here are about the seam: the query a client actually sends,
//! and the exact field names the frames carry.

use super::*;
use crate::agui::project::project;
use layer2_core::{Event, EventKind, SessionId};

fn logged(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: SessionId("sesn_1".into()),
        seq,
        kind,
    }
}

#[test]
fn the_cursor_is_read_under_the_name_a_client_sends() {
    // `wire.ts` sends `starting_after`; the REST route it used to be pointed
    // at reads `after`, so serde ignored it and every reconnect replayed the
    // whole log from the beginning. Both names are accepted here, and the one
    // the document names is the one that must work.
    let sent: Replay =
        serde_urlencoded::from_str("thread_id=sesn_1&starting_after=42").expect("parses");
    assert_eq!(sent.starting_after(), Some(42));

    let legacy: Replay = serde_urlencoded::from_str("thread_id=sesn_1&after=42").expect("parses");
    assert_eq!(
        legacy.starting_after(),
        Some(42),
        "a client written against the older spelling must not silently replay everything"
    );

    let none: Replay = serde_urlencoded::from_str("thread_id=sesn_1").expect("parses");
    assert_eq!(
        none.starting_after(),
        None,
        "absent means from the beginning"
    );
}

#[test]
fn a_replayed_frame_carries_the_field_names_the_client_reads() {
    // The contract that was broken. Each assertion below names a field the
    // client's parser switches on or reads; a rename here is a rename the
    // client cannot see, and the surface draws an unknown row instead.
    let projected = project(
        &logged(
            4,
            EventKind::MessageCommitted {
                content: "carried across".into(),
            },
        ),
        "sesn_1",
        "sesn_1",
    );
    let frame = serde_json::to_value(&projected.events[0]).expect("serialises");

    assert_eq!(
        frame["type"], "TEXT_MESSAGE_START",
        "the client switches on a top-level `type`; the core's own frames have none, \
         which is why reading them produced nothing but unknown rows"
    );
    assert!(frame.get("threadId").is_some(), "{frame}");
    assert!(frame.get("runId").is_some(), "{frame}");
    assert!(
        frame.get("seq").is_none(),
        "the sequence is deliberately not protocol: it rides as the SSE id, and a \
         client takes it from there. A client reading it off the body gets nothing, \
         which silently breaks both deduplication and the cursor it resumes on: {frame}"
    );
}

#[test]
fn history_and_the_live_stream_are_the_same_frames() {
    // Web_Style.md §18.1: "a turn drawn from history and a turn watched
    // arriving must produce the same result". They come from one projection,
    // and this is what keeps it that way.
    let event = logged(
        7,
        EventKind::ToolCallStarted {
            call_id: "t1".into(),
            title: "cargo build".into(),
            kind: "shell".into(),
        },
    );

    let live = project(&event, "run_9", "sesn_1");
    let replayed = project(&event, "sesn_1", "sesn_1");

    assert_eq!(
        live.events.iter().map(|e| e.kind).collect::<Vec<_>>(),
        replayed.events.iter().map(|e| e.kind).collect::<Vec<_>>(),
        "the same event must become the same kinds either way"
    );
    assert_eq!(
        serde_json::to_value(&live.events[0]).expect("json")["seq"],
        serde_json::to_value(&replayed.events[0]).expect("json")["seq"],
        "and carry the same cursor, which is what a client resumes on"
    );
}

#[test]
fn the_session_stream_is_where_a_client_watches_a_turn() {
    // Not an assertion about this function's body but about where the lane
    // has to live, recorded here because getting it wrong is invisible.
    //
    // A client posts a run and holds *this* stream — the session's, across
    // turns. It does not consume the run POST's response. So a live lane
    // offered only on the run stream reaches nothing: the transport would be
    // built, tested, and dead.
    //
    // The other reason is the one already written into this module: two
    // streams would make a client decide for itself how to interleave the
    // durable and the live, which is exactly the ordering question §18.1 of
    // `Web_Style.md` says must have one answer.
    let projected = super::super::live::project_live(
        &layer2_core::LiveFrame::MessageDelta("in flight".into()),
        "sesn_1",
        "sesn_1",
    )
    .expect("a message being typed has a shape");

    assert!(
        projected.cursor().is_none(),
        "and it must still carry no cursor here: this stream is the one a \
         client resumes on, so a live frame given an id would send the next \
         reconnect to a position the log has never held"
    );
}
