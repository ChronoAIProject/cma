//! Session records and event history, in MongoDB.
//!
//! Two collections. Sessions are keyed by their own id so an upsert is a
//! replace. Events carry a compound unique index on `(session_id, seq)`, which
//! is what makes the per-session sequence a guarantee rather than a hope: a
//! duplicate sequence is rejected by the database, not merely unlikely.

use futures_util::TryStreamExt;
use layer2_core::{
    store::{SessionRecord, SessionStore, StoreError},
    Event, Rate, RateCardId, SessionId, SessionPricing, SessionStatus, StoredEvent, SubjectId,
};
use mongodb::bson::{doc, Document};
use mongodb::options::{IndexOptions, ReplaceOptions};
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};

use super::event_doc;

const SESSIONS: &str = "sessions";
const EVENTS: &str = "session_events";

/// A granted skill as it is stored.
///
/// Two shapes, because records written before versions were carried hold a
/// bare string. Reading accepts either; writing always produces the object, so
/// the old shape disappears as records are rewritten rather than needing a
/// migration nobody would run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum StoredSkill {
    Versioned {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// Written before this service recorded versions.
    Bare(String),
}

/// A pinned session price in BSON-compatible signed integers.
///
/// BSON has no unsigned 64-bit integer, so this mirror is explicit and every
/// conversion is checked. A price that cannot be represented is refused
/// rather than clamped into a different charge.
#[derive(Debug, Serialize, Deserialize)]
struct PricingDoc {
    rate_card: String,
    cpu_micros_per_cpu_hour: i64,
    memory_micros_per_gib_hour: i64,
    cpu_millis: i64,
    memory_mib: i64,
    micros_per_hour: i64,
    quoted_at: String,
}

impl StoredSkill {
    fn into_granted(self) -> layer2_core::GrantedSkill {
        match self {
            Self::Versioned { id, version } => layer2_core::GrantedSkill {
                id: layer2_core::SkillId::new(id),
                version,
            },
            // Not "version unknown to the registry" but "this record predates
            // us keeping it". Both render the same way, and neither invents one.
            Self::Bare(id) => layer2_core::GrantedSkill {
                id: layer2_core::SkillId::new(id),
                version: None,
            },
        }
    }
}

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
    /// The agent configuration, stored as the core serialises it.
    ///
    /// Optional because records written before options were persisted have no
    /// such field; those load on defaults rather than failing to load at all.
    #[serde(default)]
    options: Option<mongodb::bson::Bson>,
    /// The environment bundle the session was started with, if any.
    #[serde(default)]
    environment_profile: Option<String>,
    runtime_id: Option<String>,
    context_ref: Option<String>,
    /// Runtime version, rotation count and snapshot time.
    ///
    /// All optional so documents written before rotation existed still load. A
    /// session with no recorded version is treated as being on an older one,
    /// which is what brings it onto the current image.
    #[serde(default)]
    runtime_version: Option<String>,
    #[serde(default)]
    rotations: Option<i64>,
    #[serde(default)]
    rotated_at: Option<String>,
    /// What the session was granted when it was created.
    ///
    /// Optional because records written before capabilities existed have no
    /// such field; those load claiming nothing, which is settled the next time
    /// the session is placed.
    #[serde(default)]
    capabilities: Option<mongodb::bson::Bson>,
    /// What the session's credential may reach, decided when it was created.
    ///
    /// Optional because records written before the grant was recorded have no
    /// such field, and the absence is a different fact from an empty list: one
    /// says the grant was never written down and falls back to the profile,
    /// the other says the session may reach nothing.
    #[serde(default)]
    granted_services: Option<Vec<String>>,
    /// The skills the session was given. Same distinction as above: absent
    /// means the grant predates the field, empty means it was given none.
    /// Each granted skill and the version that arrived.
    ///
    /// Records written before versions were carried hold bare strings, so the
    /// read accepts either shape and treats a bare string as a skill whose
    /// version was never recorded. Absent still means the grant itself was
    /// never recorded, which is a third thing again.
    #[serde(default)]
    granted_skills: Option<Vec<StoredSkill>>,
    /// What this session was priced at. Absent means it was never priced,
    /// which is a different fact from a price of nothing.
    #[serde(default)]
    pricing: Option<PricingDoc>,
}

/// A MongoDB-backed session store.
///
/// Events are held as raw documents rather than as a typed collection: the
/// driver would otherwise decide for itself what an unreadable document means,
/// and telling a newer vocabulary apart from real corruption is exactly the
/// decision `event_doc` exists to make.
pub struct MongoSessionStore {
    sessions: Collection<SessionDoc>,
    events: Collection<Document>,
}

impl MongoSessionStore {
    /// Connect and ensure the indexes the guarantees depend on exist.
    /// Open the store over an existing connection, ensuring its indexes exist.
    ///
    /// Takes a client rather than a URI so one connection serves every store
    /// this deployment keeps in the same database. See `super::connect`.
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
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

/// The duplicate-key code, which is the unique index on `(session_id, seq)`
/// doing its job.
const DUPLICATE_KEY: i32 = 11000;

/// Read a failed insert as a sequence collision, if that is what it was.
///
/// `None` for anything else, so a store that is merely unreachable is still
/// reported as unreachable. Only the compound unique index can produce this
/// code on this collection, so a duplicate key here means one thing.
///
/// The batch is inserted ordered, so the first failing position is also the
/// number of events the database accepted before it stopped — which is what a
/// caller re-numbering the remainder needs, and the reason this returns a
/// count rather than a bare marker.
fn sequence_taken(error: &mongodb::error::Error, total: usize) -> Option<StoreError> {
    let mongodb::error::ErrorKind::InsertMany(ref failed) = *error.kind else {
        return None;
    };
    let written = failed
        .write_errors
        .as_ref()?
        .iter()
        .filter(|w| w.code == DUPLICATE_KEY)
        .map(|w| w.index)
        .min()?;
    Some(StoreError::SequenceTaken { written, total })
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
        options: Some(
            mongodb::bson::serialize_to_bson(&s.options)
                .map_err(|e| StoreError::Malformed(e.to_string()))?,
        ),
        environment_profile: s.environment_profile.as_ref().map(|p| p.0.clone()),
        runtime_id: r.runtime_id.clone(),
        context_ref: r.context_ref.clone(),
        runtime_version: s.runtime_version.clone(),
        rotations: Some(i64::from(s.rotations)),
        rotated_at: s.rotated_at.clone(),
        capabilities: Some(
            mongodb::bson::serialize_to_bson(&s.capabilities)
                .map_err(|e| StoreError::Malformed(e.to_string()))?,
        ),
        granted_services: s
            .granted_services
            .as_ref()
            .map(|services| services.iter().map(|id| id.0.clone()).collect()),
        granted_skills: s.granted_skills.as_ref().map(|skills| {
            skills
                .iter()
                .map(|granted| StoredSkill::Versioned {
                    id: granted.id.0.clone(),
                    version: granted.version.clone(),
                })
                .collect()
        }),
        pricing: s.pricing.as_ref().map(pricing_to_doc).transpose()?,
    })
}

fn pricing_to_doc(pricing: &SessionPricing) -> Result<PricingDoc, StoreError> {
    Ok(PricingDoc {
        rate_card: pricing.rate_card.0.clone(),
        cpu_micros_per_cpu_hour: price_to_i64(pricing.rate.cpu_micros_per_cpu_hour)?,
        memory_micros_per_gib_hour: price_to_i64(pricing.rate.memory_micros_per_gib_hour)?,
        cpu_millis: capacity_to_i64(pricing.cpu_millis, "CPU millicores")?,
        memory_mib: capacity_to_i64(pricing.memory_mib, "memory MiB")?,
        micros_per_hour: price_to_i64(pricing.micros_per_hour)?,
        quoted_at: pricing.quoted_at.clone(),
    })
}

fn price_to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| {
        StoreError::Malformed(format!("a price of {value} micro-USD cannot be stored"))
    })
}

fn capacity_to_i64(value: u32, unit: &str) -> Result<i64, StoreError> {
    i64::try_from(u64::from(value)).map_err(|_| {
        StoreError::Malformed(format!(
            "a session capacity of {value} {unit} cannot be stored"
        ))
    })
}

fn pricing_from_doc(pricing: PricingDoc) -> Result<SessionPricing, StoreError> {
    Ok(SessionPricing {
        rate_card: RateCardId(pricing.rate_card),
        rate: Rate {
            cpu_micros_per_cpu_hour: stored_price(pricing.cpu_micros_per_cpu_hour)?,
            memory_micros_per_gib_hour: stored_price(pricing.memory_micros_per_gib_hour)?,
        },
        cpu_millis: stored_capacity(pricing.cpu_millis, "CPU millicores")?,
        memory_mib: stored_capacity(pricing.memory_mib, "memory MiB")?,
        micros_per_hour: stored_price(pricing.micros_per_hour)?,
        quoted_at: pricing.quoted_at,
    })
}

fn stored_price(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| {
        StoreError::Malformed(format!("a stored price of {value} micro-USD is invalid"))
    })
}

fn stored_capacity(value: i64, unit: &str) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| {
        StoreError::Malformed(format!(
            "a stored session capacity of {value} {unit} is invalid"
        ))
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
            runtime_version: d.runtime_version,
            rotations: d.rotations.unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
            rotated_at: d.rotated_at,
            capabilities: match d.capabilities {
                Some(v) => mongodb::bson::deserialize_from_bson(v)
                    .map_err(|e| StoreError::Malformed(e.to_string()))?,
                None => Default::default(),
            },
            environment_profile: d.environment_profile.map(layer2_core::EnvironmentProfileId),
            granted_services: d
                .granted_services
                .map(|services| services.into_iter().map(layer2_core::ServiceId).collect()),
            granted_skills: d
                .granted_skills
                .map(|skills| skills.into_iter().map(StoredSkill::into_granted).collect()),
            pricing: d.pricing.map(pricing_from_doc).transpose()?,
            options: match d.options {
                Some(v) => mongodb::bson::deserialize_from_bson(v)
                    .map_err(|e| StoreError::Malformed(e.to_string()))?,
                None => Default::default(),
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

    async fn count_live_for(&self, owner: &SubjectId) -> Result<u32, StoreError> {
        let statuses = SessionStatus::WANTING_RUNTIME
            .iter()
            .map(serde_plain_string)
            .collect::<Result<Vec<_>, _>>()?;
        let count = self
            .sessions
            .count_documents(doc! { "owner": &owner.0, "status": { "$in": statuses } })
            .await
            .map_err(|e| StoreError::Unavailable(format!("count live sessions: {e}")))?;
        u32::try_from(count)
            .map_err(|_| StoreError::Malformed("live session count exceeds u32".into()))
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
        let docs: Result<Vec<Document>, StoreError> =
            events.iter().map(event_doc::to_document).collect();
        let total = events.len();
        self.events.insert_many(docs?).await.map_err(|e| {
            sequence_taken(&e, total)
                .unwrap_or_else(|| StoreError::Unavailable(format!("append events: {e}")))
        })?;
        Ok(())
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
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
            out.push(event_doc::from_document(d)?);
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
#[path = "mongo_tests.rs"]
mod tests;
