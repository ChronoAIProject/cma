//! An AG-UI event and its SSE representation.

use serde::Serialize;

/// One AG-UI event, ready to travel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct AguiEvent {
    /// The event type, in the protocol's own spelling.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The run this belongs to, echoed on every event.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// The session, which this protocol calls a thread.
    #[serde(rename = "threadId")]
    pub thread_id: String,
    /// Everything else the event carries.
    #[serde(flatten)]
    pub body: serde_json::Value,
    /// When the durable event this was projected from was appended, RFC 3339.
    ///
    /// Absent for a liveness frame — which has no log position — and for an
    /// event written by a control plane that predates timestamps. A client
    /// renders no time rather than an invented one.
    #[serde(rename = "timestamp", skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// The durable sequence this was projected from, when there is one.
    ///
    /// Not part of the protocol's own payload: it rides as the SSE id, which
    /// is what a reconnecting client hands back. Skipped in the body so a
    /// client is never tempted to read it as protocol.
    ///
    /// `None` for every liveness frame and for every leading frame of a
    /// multi-frame durable projection. Only the projection's final frame is a
    /// safe resume point: receiving an earlier one does not mean the durable
    /// event arrived whole.
    #[serde(skip)]
    pub seq: Option<u64>,
    /// Whether this is liveness rather than a projection of durable history.
    ///
    /// Kept separate from `seq`: a leading durable projection frame also has
    /// no SSE id, but it must not be mistaken for the live lane. In particular,
    /// committed message content remains history even though its closing frame
    /// is the only one that advances the cursor.
    #[serde(rename = "cma:live", skip_serializing_if = "is_false")]
    pub live: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// Render one event as the frame that goes on the wire.
///
/// One function, used by the run stream and by the replay, so the two cannot
/// come to disagree about the shape a client has to read.
///
/// **The frame is deliberately unnamed.** An `EventSource` has no wildcard
/// listener: a frame carrying an `event:` name is delivered only to a listener
/// registered for that exact name, so naming them makes a kind the client has
/// never heard of unreceivable. This service's event set grows additively and
/// `docs/API_AGUI.md` requires an unrecognised kind to arrive anyway, so it can
/// be shown rather than dropped — which named frames make impossible.
///
/// Nothing is lost by it: the kind travels in the body as `type`, which is what
/// every client dispatches on and always has.
pub(super) fn sse_frame(event: &AguiEvent) -> axum::response::sse::Event {
    let frame = axum::response::sse::Event::default()
        .json_data(event)
        // Serialising cannot fail for a shape this crate builds, but a panic
        // here would take a live stream down; a client reading a run error it
        // did not expect is survivable and says something went wrong.
        .unwrap_or_else(|_| axum::response::sse::Event::default().data(r#"{"type":"RUN_ERROR"}"#));
    // Only where there is one. Liveness and leading durable projection frames
    // carry no cursor: neither is a point after which replay may safely begin.
    match event.cursor() {
        Some(id) => frame.id(id),
        None => frame,
    }
}

impl AguiEvent {
    /// The SSE id this frame travels under, if it may travel under one.
    ///
    /// One place, so the durable stream and the replay cannot disagree about
    /// it and neither can accidentally stamp a liveness frame.
    pub(super) fn cursor(&self) -> Option<String> {
        self.seq.map(|seq| seq.to_string())
    }
}
