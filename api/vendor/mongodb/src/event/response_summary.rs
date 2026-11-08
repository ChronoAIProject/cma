//! Optional content-free response accounting without owned BSON conversion.
use crate::bson::{RawBsonRef, RawDocument};

/// Numeric observations of one command response. Bytes are uncompressed BSON,
/// excluding transport framing. Cursor documents include the entire received
/// batch, even when the application subsequently abandons the cursor.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct ResponseSummary {
    /// Whether the server reported a successful command.
    pub succeeded: bool,
    /// Encoded length of the complete BSON response; zero on transport failure.
    pub reply_bytes: u64,
    /// Number of documents in firstBatch, nextBatch, or findAndModify's value.
    pub documents: u64,
    /// Sum of encoded document lengths, without array or reply envelopes.
    pub document_bytes: u64,
    /// False when a result batch cannot be accounted for as BSON documents.
    pub complete: bool,
}

/// A synchronous, non-blocking observer. Implementations must not panic.
/// The class token lets callers apply a fixed label allow-list before I/O;
/// the driver never retains command documents or names for this observer.
pub trait ResponseObserver: std::fmt::Debug + Send + Sync {
    /// Map command and optional collection names to a caller-owned class token.
    /// No filter, pipeline, document, database or identity is passed here.
    fn classify(&self, operation: &str, collection: Option<&str>) -> u32;
    /// Record numeric metadata, including failed responses and retries.
    fn observe(&self, class: u32, summary: ResponseSummary);
}

pub(crate) fn collection<'a>(operation: &str, command: &'a RawDocument) -> Option<&'a str> {
    let field = match operation {
        "getMore" => "collection",
        "find" | "aggregate" | "findAndModify" | "distinct" | "count" => operation,
        _ => return None,
    };
    command.get_str(field).ok()
}

impl ResponseSummary {
    /// Summarize borrowed raw BSON without decoding or copying document contents.
    pub fn from_reply(reply: &RawDocument) -> Self {
        let mut summary = Self {
            succeeded: match reply.get("ok") {
                Ok(Some(RawBsonRef::Int32(1) | RawBsonRef::Int64(1))) => true,
                Ok(Some(RawBsonRef::Double(value))) => value == 1.0,
                _ => false,
            },
            reply_bytes: reply.as_bytes().len() as u64,
            complete: true,
            ..Self::default()
        };
        if let Ok(cursor) = reply.get_document("cursor") {
            let batch = cursor.get_array("firstBatch").or_else(|_| cursor.get_array("nextBatch"));
            match batch {
                Ok(batch) => {
                    for entry in batch {
                        match entry {
                            Ok(RawBsonRef::Document(document)) => summary.document(document),
                            _ => { summary.complete = false; break; }
                        }
                    }
                }
                Err(_) => summary.complete = false,
            }
        } else if !matches!(reply.get("cursor"), Ok(None)) {
            summary.complete = false;
        } else {
            match reply.get("value") {
                Ok(Some(RawBsonRef::Document(value))) => summary.document(value),
                Ok(None | Some(RawBsonRef::Null)) => {},
                _ => summary.complete = false,
            }
        }
        summary
    }

    fn document(&mut self, document: &RawDocument) {
        self.documents = self.documents.saturating_add(1);
        self.document_bytes = self.document_bytes.saturating_add(document.as_bytes().len() as u64);
    }
}
