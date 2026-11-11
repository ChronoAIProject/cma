//! Frozen format-2 keyset discovery. Candidates grant no execution authority.
use std::time::Duration;

use futures_util::TryStreamExt;
use layer2_core::sandbox::agent_capacity_recovery::{ports::RecoveryError, state::RecoveryRequest};
use mongodb::{
    bson::{self, doc, Document},
    options::{Collation, Hint},
};

use super::records;
mod agent;
use crate::store::sandbox_mongo::MongoSandboxStore;
pub(super) use agent::page as for_agent;

pub(crate) const PAGE_SIZE: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stream {
    Admitted,
    ExpiredLease,
}

#[derive(Debug, Clone)]
pub(crate) struct Cursor {
    stream: Stream,
    at: String,
    key: String,
}

pub(crate) struct Page {
    pub requests: Vec<RecoveryRequest>,
    pub next: Option<Cursor>,
}

impl Stream {
    fn fields(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Admitted => ("admitted", "work.ready_at", "agent_capacity_recovery_due"),
            Self::ExpiredLease => (
                "attaching",
                "work.lease.expires_at",
                "agent_capacity_recovery_expired",
            ),
        }
    }
}

pub(super) async fn page(
    store: &MongoSandboxStore,
    stream: Stream,
    after: Option<&Cursor>,
    now: &str,
) -> Result<Page, RecoveryError> {
    let (phase, field, hint) = stream.fields();
    let filter = filter(stream, after, now)?;
    let mut rows = records::collection(store)
        .find(filter)
        .sort(doc! {field: 1, "_id": 1})
        .hint(Hint::Name(hint.into()))
        .collation(Collation::builder().locale("simple").build())
        .projection(doc! {"_id": 1, "format_version": 1, "agent_id": 1,
        "work.request": 1, "work.revision": 1, "work.phase": 1,
        "work.ready_at": 1, "work.lease": 1})
        .limit(i64::from(PAGE_SIZE))
        .batch_size(PAGE_SIZE)
        .max_time(Duration::from_millis(1000))
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let mut requests = Vec::with_capacity(PAGE_SIZE as usize);
    let mut next = None;
    while let Some(row) = rows
        .try_next()
        .await
        .map_err(|_| RecoveryError::Unavailable)?
    {
        if requests.len() == PAGE_SIZE as usize {
            return Err(RecoveryError::InvalidEvidence);
        }
        let (request, cursor) = candidate(&row, stream, phase, now)?;
        if next.as_ref().or(after).is_some_and(|previous: &Cursor| {
            (cursor.at.as_str(), cursor.key.as_str())
                <= (previous.at.as_str(), previous.key.as_str())
        }) {
            return Err(RecoveryError::InvalidEvidence);
        }
        requests.push(request);
        next = Some(cursor);
    }
    if requests.len() < PAGE_SIZE as usize {
        next = None;
    }
    Ok(Page { requests, next })
}

fn filter(stream: Stream, after: Option<&Cursor>, now: &str) -> Result<Document, RecoveryError> {
    if layer2_core::seconds_from_rfc3339(now).is_none() {
        return Err(RecoveryError::InvalidEvidence);
    }
    let (phase, field, _) = stream.fields();
    let mut filter = doc! {"format_version": 2_i64, "work.phase": phase,
    field: {"$lte": now}};
    if let Some(after) = after {
        if after.stream != stream
            || layer2_core::seconds_from_rfc3339(&after.at).is_none()
            || !after.key.starts_with("capacity-recovery:")
            || after.key.len() != 82
            || !after.key[18..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(RecoveryError::InvalidEvidence);
        }
        // Each seek carries its equality prefix and due-time bound. Factoring
        // these outside $or lets Mongo filter the cursor after fetching earlier
        // pages, so examined work grows with history despite the result limit.
        filter = doc! {"$or": [
            {"format_version": 2_i64, "work.phase": phase,
                field: {"$gt": &after.at, "$lte": now}},
            {"format_version": 2_i64, "work.phase": phase,
                field: {"$eq": &after.at, "$lte": now}, "_id": {"$gt": &after.key}},
        ]};
    }
    Ok(filter)
}

fn candidate(
    row: &Document,
    stream: Stream,
    phase: &str,
    now: &str,
) -> Result<(RecoveryRequest, Cursor), RecoveryError> {
    let invalid = |_| RecoveryError::InvalidEvidence;
    let work = row.get_document("work").map_err(invalid)?;
    let request: RecoveryRequest =
        bson::deserialize_from_document(work.get_document("request").map_err(invalid)?.clone())
            .map_err(|_| RecoveryError::InvalidEvidence)?;
    request.validate()?;
    let key = request.key()?;
    let at = match stream {
        Stream::Admitted => work.get_str("ready_at").map_err(invalid)?,
        Stream::ExpiredLease => work
            .get_document("lease")
            .map_err(invalid)?
            .get_str("expires_at")
            .map_err(invalid)?,
    };
    if row.get_i64("format_version").ok() != Some(2)
        || row.get_str("_id").ok() != Some(key.as_str())
        || row.get_str("agent_id").ok() != Some(request.agent_id.as_str())
        || work.get_str("phase").ok() != Some(phase)
        || !work.get_i64("revision").is_ok_and(|revision| revision >= 1)
        || layer2_core::seconds_from_rfc3339(at).is_none()
        || at > now
    {
        return Err(RecoveryError::InvalidEvidence);
    }
    Ok((
        request,
        Cursor {
            stream,
            at: at.into(),
            key,
        },
    ))
}

#[cfg(test)]
mod tests;
