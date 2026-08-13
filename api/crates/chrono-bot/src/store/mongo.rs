//! Session records and event history, in MongoDB.
//!
//! Two collections. Sessions are keyed by their own id so an upsert is a
//! replace. Events carry a compound unique index on `(session_id, seq)`, which
//! is what makes the per-session sequence a guarantee rather than a hope: a
//! duplicate sequence is rejected by the database, not merely unlikely.

use futures_util::TryStreamExt;
use layer2_core::{
    store::{SessionRecord, SessionStore, StoreError},
    Event, SessionId, SubjectId,
};
use mongodb::bson::{doc, Document};
use mongodb::options::{IndexOptions, ReplaceOptions};
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};

const SESSIONS: &str = "sessions";
const EVENTS: &str = "session_events";

/// How a session record is shaped on the wire.
///
/// A distinct type from the core's record on purpose: the core owns its model,
/// and this owns how it is written. `_id` carries the session id so the
/// database enforces uniqueness rather than the application.
#[derive(Debug, Serialize, Deserialize)]
struct SessionDoc {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    kernel: String,
    status: String,
    recovery_state: String,
    stop_reason: Option<String>,
    columns: u32,
    rows: u32,
    last_seq: i64,
    created_at: String,
    runtime_id: Option<String>,
    context_ref: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EventDoc {
    session_id: String,
    seq: i64,
    /// The event kind, stored as the core serialises it so the vocabulary
    /// lives in one place rather than being mirrored here.
    kind: mongodb::bson::Bson,
}

/// A MongoDB-backed session store.
pub struct MongoSessionStore {
    sessions: Collection<SessionDoc>,
    events: Collection<EventDoc>,
}

impl MongoSessionStore {
    /// Connect and ensure the indexes the guarantees depend on exist.
    pub async fn connect(uri: &str, database: &str) -> Result<Self, StoreError> {
        let client = Client::with_uri_str(uri)
            .await
            .map_err(|e| StoreError::Unavailable(format!("connect: {e}")))?;
        let db = client.database(database);
        let store = Self {
            sessions: db.collection(SESSIONS),
            events: db.collection(EVENTS),
        };
        store.ensure_indexes().await?;
        tracing::info!(database, "session store ready");
        Ok(store)
    }

    async fn ensure_indexes(&self) -> Result<(), StoreError> {
        // Unique per (session, seq): the database is what makes the per-session
        // monotonic sequence a guarantee. Without it a racing append could
        // duplicate a cursor position and a resuming client would silently
        // skip or repeat history.
        let seq = IndexModel::builder()
            .keys(doc! { "session_id": 1, "seq": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        self.events
            .create_index(seq)
            .await
            .map_err(|e| StoreError::Unavailable(format!("event index: {e}")))?;

        let owner = IndexModel::builder()
            .keys(doc! { "owner": 1, "created_at": -1 })
            .build();
        self.sessions
            .create_index(owner)
            .await
            .map_err(|e| StoreError::Unavailable(format!("session index: {e}")))?;
        Ok(())
    }
}

fn to_doc(r: &SessionRecord) -> Result<SessionDoc, StoreError> {
    let s = &r.session;
    Ok(SessionDoc {
        id: s.id.0.clone(),
        owner: s.owner.0.clone(),
        kernel: s.kernel.0.clone(),
        status: serde_plain_string(&s.status)?,
        recovery_state: serde_plain_string(&s.recovery_state)?,
        stop_reason: match &s.stop_reason {
            Some(v) => Some(serde_plain_string(v)?),
            None => None,
        },
        columns: s.geometry.columns() as u32,
        rows: s.geometry.rows() as u32,
        last_seq: s.last_seq as i64,
        created_at: s.created_at.clone(),
        runtime_id: r.runtime_id.clone(),
        context_ref: r.context_ref.clone(),
    })
}

/// Render a core enum using its own serde representation.
///
/// Going through serde keeps the stored strings identical to the ones the
/// interfaces emit, so a value never means two different things depending on
/// which layer wrote it.
fn serde_plain_string<T: Serialize>(value: &T) -> Result<String, StoreError> {
    match serde_json::to_value(value).map_err(|e| StoreError::Malformed(e.to_string()))? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(StoreError::Malformed(format!(
            "expected a string representation, got {other}"
        ))),
    }
}

fn parse_plain<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|e| StoreError::Malformed(format!("{s:?}: {e}")))
}

fn from_doc(d: SessionDoc) -> Result<SessionRecord, StoreError> {
    use layer2_core::{Session, TerminalGeometry};
    let geometry = TerminalGeometry::new(d.columns as u16, d.rows as u16)
        .map_err(|e| StoreError::Malformed(e.to_string()))?;
    Ok(SessionRecord {
        session: Session {
            id: SessionId(d.id),
            owner: SubjectId(d.owner),
            kernel: layer2_core::KernelId::new(d.kernel),
            status: parse_plain(&d.status)?,
            recovery_state: parse_plain(&d.recovery_state)?,
            stop_reason: match d.stop_reason {
                Some(v) => Some(parse_plain(&v)?),
                None => None,
            },
            geometry,
            last_seq: d.last_seq.max(0) as u64,
            created_at: d.created_at,
        },
        runtime_id: d.runtime_id,
        context_ref: d.context_ref,
    })
}

#[async_trait::async_trait]
impl SessionStore for MongoSessionStore {
    async fn upsert(&self, record: &SessionRecord) -> Result<(), StoreError> {
        let doc = to_doc(record)?;
        self.sessions
            .replace_one(doc! { "_id": &doc.id }, &doc)
            .with_options(ReplaceOptions::builder().upsert(true).build())
            .await
            .map_err(|e| StoreError::Unavailable(format!("upsert: {e}")))?;
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        let found = self
            .sessions
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("get: {e}")))?;
        found.map(from_doc).transpose()
    }

    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<SessionRecord>, StoreError> {
        self.query(doc! { "owner": &owner.0 }).await
    }

    async fn list_all(&self) -> Result<Vec<SessionRecord>, StoreError> {
        self.query(doc! {}).await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.sessions
            .delete_one(doc! { "_id": &id.0 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("delete session: {e}")))?;
        // History goes with it: keeping orphaned events would leave a record of
        // work whose session the owner asked to be removed.
        self.events
            .delete_many(doc! { "session_id": &id.0 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("delete events: {e}")))?;
        Ok(())
    }

    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        let docs: Result<Vec<EventDoc>, StoreError> = events
            .iter()
            .map(|e| {
                Ok(EventDoc {
                    session_id: e.session_id.0.clone(),
                    seq: e.seq as i64,
                    kind: mongodb::bson::serialize_to_bson(&e.kind)
                        .map_err(|err| StoreError::Malformed(err.to_string()))?,
                })
            })
            .collect();
        self.events
            .insert_many(docs?)
            .await
            .map_err(|e| StoreError::Unavailable(format!("append events: {e}")))?;
        Ok(())
    }

    async fn events_since(&self, id: &SessionId, after: u64) -> Result<Vec<Event>, StoreError> {
        let mut cursor = self
            .events
            .find(doc! { "session_id": &id.0, "seq": { "$gt": after as i64 } })
            .sort(doc! { "seq": 1 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("events: {e}")))?;
        let mut out = Vec::new();
        while let Some(d) = cursor
            .try_next()
            .await
            .map_err(|e| StoreError::Unavailable(format!("events cursor: {e}")))?
        {
            out.push(Event {
                session_id: SessionId(d.session_id),
                seq: d.seq.max(0) as u64,
                kind: mongodb::bson::deserialize_from_bson(d.kind)
                    .map_err(|e| StoreError::Malformed(e.to_string()))?,
            });
        }
        Ok(out)
    }
}

impl MongoSessionStore {
    async fn query(&self, filter: Document) -> Result<Vec<SessionRecord>, StoreError> {
        let mut cursor = self
            .sessions
            .find(filter)
            .sort(doc! { "created_at": -1 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("list: {e}")))?;
        let mut out = Vec::new();
        while let Some(d) = cursor
            .try_next()
            .await
            .map_err(|e| StoreError::Unavailable(format!("list cursor: {e}")))?
        {
            out.push(from_doc(d)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{RecoveryState, SessionStatus, StopReason};

    #[test]
    fn enums_round_trip_through_their_serde_representation() {
        // The stored string must be the same one the interfaces emit, or a
        // value would mean different things depending on which layer wrote it.
        assert_eq!(serde_plain_string(&SessionStatus::Ready).unwrap(), "ready");
        assert_eq!(
            serde_plain_string(&RecoveryState::Recovering).unwrap(),
            "recovering"
        );
        assert_eq!(
            serde_plain_string(&StopReason::RequiresAction).unwrap(),
            "requires_action"
        );

        let s: SessionStatus = parse_plain("terminated").unwrap();
        assert_eq!(s, SessionStatus::Terminated);
        let r: RecoveryState = parse_plain("degraded").unwrap();
        assert_eq!(r, RecoveryState::Degraded);
    }

    #[test]
    fn an_unknown_stored_value_is_rejected_rather_than_guessed() {
        assert!(parse_plain::<SessionStatus>("not-a-status").is_err());
    }
}
