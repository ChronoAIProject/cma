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
    /// **`None` for a liveness frame, and the option is the guard.** What is
    /// happening on a session — a message being typed — is not in the durable
    /// log and cannot be resumed from. A frame like that given an id becomes a
    /// cursor: a client that reconnects on it asks the log to continue from a
    /// number the log has never heard of, and everything between there and the
    /// real position is skipped. Making the field an `Option` is what stops
    /// that being a rule somebody has to remember.
    ///
    /// Its **absence** is stated on the wire as `cma:live`, because a
    /// client cannot otherwise see it. `EventSource` reports `lastEventId` as
    /// the last id it was *given*, so a frame sent without one is
    /// indistinguishable from a frame repeating the previous id — and a client
    /// deduplicating on that would silently drop every liveness frame.
    ///
    /// This is not the thing the paragraph above forbids. A cursor is a
    /// position a client could wrongly resume from; this says there is no
    /// position, which is the opposite hazard. It is written from this one
    /// field, so the two cannot come to disagree.
    #[serde(
        rename = "cma:live",
        serialize_with = "mark_live",
        skip_serializing_if = "Option::is_some"
    )]
    pub seq: Option<u64>,
}

/// Write the marker that says this frame carries no cursor.
///
/// Only ever reached for a frame whose sequence is `None` — the field is
/// skipped otherwise — so it writes the one value it can mean.
fn mark_live<S>(_seq: &Option<u64>, out: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    out.serialize_bool(true)
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
    // Only where there is one. A liveness frame carries no cursor, and
    // stamping it with an id would make a client reconnect from a position the
    // log has never held.
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
