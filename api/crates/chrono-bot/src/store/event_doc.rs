//! How a durable event is written to, and read back from, MongoDB.
//!
//! Reading is deliberately asymmetric with writing. A writer only ever holds an
//! `Event`, so every document this control plane produces carries a
//! discriminator it owns. A reader has no such guarantee: during a rolling
//! deployment the collection holds whatever the newest control plane has
//! written, and the older one still serving traffic must be able to read past
//! it. Treating an unfamiliar discriminator as corruption would end the read,
//! and with it the stream a client is watching — the session would go blank
//! because a colleague's pod shipped first.
//!
//! So the two failures are kept apart. A document whose envelope is wrong is
//! damage and stays `Malformed`. A document whose envelope is right and whose
//! discriminator is unfamiliar is history from the future, and is carried back
//! intact for the caller to skip.

use layer2_core::{store::StoreError, Event, EventKind, SessionId, StoredEvent, StoredEventKind};
use mongodb::bson::{Bson, Document};
use serde::Serialize;

/// How an event is shaped on the wire.
///
/// A distinct type from the core's event on purpose: the core owns its
/// vocabulary, and this owns how it is written. `kind` is stored as the core
/// serialises it so the vocabulary lives in one place rather than being
/// mirrored here.
#[derive(Debug, Serialize)]
struct EventDoc {
    session_id: String,
    seq: i64,
    appended_at: String,
    kind: Bson,
}

/// Render an event for insertion.
pub(super) fn to_document(event: &Event) -> Result<Document, StoreError> {
    let doc = EventDoc {
        session_id: event.session_id.0.clone(),
        seq: event.seq as i64,
        appended_at: event.appended_at.clone(),
        kind: mongodb::bson::serialize_to_bson(&event.kind)
            .map_err(|error| StoreError::Malformed(error.to_string()))?,
    };
    mongodb::bson::serialize_to_document(&doc)
        .map_err(|error| StoreError::Malformed(error.to_string()))
}

/// Read one stored event back, tolerating a vocabulary this build predates.
pub(super) fn from_document(doc: Document) -> Result<StoredEvent, StoreError> {
    from_document_recognizing(doc, EventKind::recognizes_tag)
}

/// Read with the vocabulary a particular build knows.
///
/// Separate from the public reader so compatibility tests can exercise an
/// older build's exact decision without maintaining a second deserializer.
fn from_document_recognizing(
    mut doc: Document,
    recognizes_tag: impl Fn(&str) -> bool,
) -> Result<StoredEvent, StoreError> {
    let session_id = match doc.remove("session_id") {
        Some(Bson::String(value)) => value,
        _ => return Err(malformed("session_id must be a string")),
    };
    let seq = match doc.remove("seq") {
        Some(Bson::Int64(value)) => {
            u64::try_from(value).map_err(|_| malformed("seq must not be negative"))?
        }
        _ => return Err(malformed("seq must be a 64-bit integer")),
    };
    // Absent on a document written before this field existed, and read as
    // empty rather than as damage: a rolling deployment must read a
    // colleague's older event without failing the stream over a time it does
    // not have.
    let appended_at = match doc.remove("appended_at") {
        Some(Bson::String(value)) => value,
        _ => String::new(),
    };
    let mut payload = match doc.remove("kind") {
        Some(Bson::Document(value)) => value,
        _ => return Err(malformed("kind must be a document")),
    };
    let tag = payload
        .get_str("type")
        .map(str::to_owned)
        .map_err(|_| malformed("kind.type must be a string"))?;

    let kind = if recognizes_tag(&tag) {
        // Strict for everything this build owns: a payload we can name and
        // cannot read is damage, and hiding it here would let it spread.
        StoredEventKind::Known(
            mongodb::bson::deserialize_from_bson(Bson::Document(payload))
                .map_err(|error| malformed(error.to_string()))?,
        )
    } else {
        // The discriminator is carried separately, so it does not appear twice.
        payload.remove("type");
        tracing::warn!(
            session_id,
            seq,
            event_tag = tag,
            "read a durable event kind this build does not understand; skipping it"
        );
        StoredEventKind::Unknown {
            tag,
            payload: unreadable_payload(payload),
        }
    };

    Ok(StoredEvent {
        session_id: SessionId(session_id),
        seq,
        appended_at,
        kind,
    })
}

#[cfg(test)]
pub(super) fn from_document_before_context_compaction(
    doc: Document,
) -> Result<StoredEvent, StoreError> {
    from_document_recognizing(doc, |tag| {
        tag != "context_compacted" && EventKind::recognizes_tag(tag)
    })
}

/// Keep an unfamiliar payload in a shape the core can hold.
///
/// The core must not learn the storage backend's value type, so the payload
/// crosses as JSON. Nothing reads it — it exists so an operator diagnosing a
/// gap can see what was skipped — which is why a payload that will not convert
/// is reported and dropped rather than failing the read it is only annotating.
fn unreadable_payload(payload: Document) -> serde_json::Value {
    match mongodb::bson::deserialize_from_bson(Bson::Document(payload)) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                %error,
                "an unrecognised event's payload could not be rendered for diagnosis"
            );
            serde_json::Value::Null
        }
    }
}

fn malformed(message: impl Into<String>) -> StoreError {
    StoreError::Malformed(format!("event document: {}", message.into()))
}

#[cfg(test)]
#[path = "event_doc_tests.rs"]
mod event_doc_tests;
