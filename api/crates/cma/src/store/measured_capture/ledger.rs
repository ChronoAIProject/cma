//! Append-only finalized measured batches, derived from one source snapshot.
use super::{
    preview::{custody_samples, MAX_ROWS},
    CaptureError, MeasuredCapture,
};
use crate::store::{durability::majority_collection, migrations::measured_batches, sandbox_usage};
use layer2_core::{
    sandbox_metrics::measured_evidence::batch::{
        CorrectionReference, EvidenceScope, FrozenEvidence,
    },
    sandbox_usage::UsageEvent,
};
use mongodb::{
    bson::{doc, Document},
    options::ReadConcern,
    ClientSession, Collection, Database,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const MAX_HISTORY: usize = 4096;
const MAX_REVISIONS: usize = 128;
const MAX_BATCH_PAYLOAD: usize = 8 * 1024 * 1024;
const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;
#[path = "ledger/period.rs"]
mod period;
pub(crate) use period::{ExactPeriodReadRequest, PeriodReadRequest, SelectedWindow};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FreezeRequest {
    pub scope: EvidenceScope,
    pub source_cutoff_ms: u64,
    pub correction_of: Option<CorrectionReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadRequest {
    pub scope: EvidenceScope,
    pub revision: u64,
}

pub(crate) struct MeasuredLedger {
    database: Database,
    rows: Collection<Document>,
}

impl MeasuredLedger {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            database: database.clone(),
            rows: majority_collection(database, measured_batches::COLLECTION),
        }
    }

    pub(super) async fn freeze(
        &self,
        request: FreezeRequest,
    ) -> Result<FrozenEvidence, CaptureError> {
        self.freeze_with_history_limit(request, MAX_HISTORY_BYTES)
            .await
    }

    async fn freeze_with_history_limit(
        &self,
        request: FreezeRequest,
        history_limit: usize,
    ) -> Result<FrozenEvidence, CaptureError> {
        let stream_id = request
            .scope
            .stream_id()
            .map_err(|_| CaptureError::Corrupt)?;
        let mut session = self
            .database
            .client()
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let (history, history_bytes) = self
            .history(&stream_id, &request.scope, &mut session)
            .await?;
        let target = match &request.correction_of {
            None => 1,
            Some(reference) => history
                .iter()
                .find(|entry| &entry.reference() == reference)
                .ok_or(CaptureError::Unavailable)?
                .revision()
                .checked_add(1)
                .ok_or(CaptureError::Unavailable)?,
        };
        if let Some(stored) = history.iter().find(|entry| entry.revision() == target) {
            if stored.matches_request(
                &request.scope,
                request.source_cutoff_ms,
                request.correction_of.as_ref(),
            ) {
                return Ok(stored.clone());
            }
            return Err(CaptureError::Unavailable);
        }
        let previous = history.last();
        if previous.map_or(Some(1), |entry| entry.revision().checked_add(1)) != Some(target)
            || history.len() >= MAX_REVISIONS
        {
            return Err(CaptureError::Unavailable);
        }
        let capture = MeasuredCapture::new(&self.database);
        let positions = capture.read_positions(&mut session, MAX_ROWS).await?;
        let samples = custody_samples(&positions)?;
        let events = self
            .allocation(&request.scope, request.source_cutoff_ms, &mut session)
            .await?;
        if session.snapshot_time().is_none() {
            return Err(CaptureError::Unavailable);
        }
        let evidence = FrozenEvidence::freeze(
            request.scope,
            request.source_cutoff_ms,
            &samples,
            &events,
            previous,
        )
        .map_err(|_| CaptureError::Unavailable)?;
        let expected = encode(&evidence)?;
        // Preserve replay of every retained revision even after this append.
        checked_history_bytes(
            history_bytes,
            expected
                .get_str("payload")
                .map_err(|_| CaptureError::Corrupt)?
                .len(),
            history_limit,
        )?;
        // The unique immutable stream/revision key serializes concurrent writers.
        // A failed acknowledgement never permits a successor or replaces bytes.
        match self.rows.insert_one(expected.clone()).await {
            Ok(_) => Ok(evidence),
            Err(_) => {
                let retained = self
                    .rows
                    .find_one(doc! {"_id":evidence.evidence_id()})
                    .await
                    .map_err(|_| CaptureError::Unavailable)?;
                if retained.as_ref() == Some(&expected) {
                    Ok(evidence)
                } else {
                    Err(CaptureError::Unavailable)
                }
            }
        }
    }

    pub(super) async fn read(&self, request: ReadRequest) -> Result<FrozenEvidence, CaptureError> {
        let stream_id = request
            .scope
            .stream_id()
            .map_err(|_| CaptureError::Corrupt)?;
        let mut session = self
            .database
            .client()
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let (history, _) = self
            .history(&stream_id, &request.scope, &mut session)
            .await?;
        history
            .into_iter()
            .find(|entry| entry.revision() == request.revision)
            .ok_or(CaptureError::Unavailable)
    }

    async fn history(
        &self,
        stream_id: &str,
        scope: &EvidenceScope,
        session: &mut ClientSession,
    ) -> Result<(Vec<FrozenEvidence>, usize), CaptureError> {
        let mut cursor = self
            .rows
            .find(doc! {"stream_id":stream_id})
            .sort(doc! {"revision":1})
            .limit((MAX_REVISIONS + 1) as i64)
            .max_time(Duration::from_secs(10))
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let mut history = Vec::new();
        let mut bytes = 0;
        while let Some(row) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(|_| CaptureError::Unavailable)?
        {
            bytes = checked_history_bytes(
                bytes,
                row.get_str("payload")
                    .map_err(|_| CaptureError::Corrupt)?
                    .len(),
                MAX_HISTORY_BYTES,
            )?;
            let entry = decode(&row)?;
            if entry.scope() != scope || history.len() >= MAX_REVISIONS {
                return Err(CaptureError::Corrupt);
            }
            if let Some(previous) = history.last() {
                entry
                    .validate_successor(previous)
                    .map_err(|_| CaptureError::Corrupt)?;
            } else if entry.revision() != 1 {
                return Err(CaptureError::Corrupt);
            }
            history.push(entry);
        }
        Ok((history, bytes))
    }

    async fn allocation(
        &self,
        scope: &EvidenceScope,
        cutoff_ms: u64,
        session: &mut ClientSession,
    ) -> Result<Vec<UsageEvent>, CaptureError> {
        use layer2_core::sandbox_metrics::measured_evidence::coverage::SUPPORT_WINDOW_MS;
        let state = self
            .database
            .collection::<Document>(sandbox_usage::STATES)
            .find_one(doc! {"_id":&scope.sandbox_id})
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|_| CaptureError::Unavailable)?
            .ok_or(CaptureError::Unavailable)?;
        let state = sandbox_usage::read_state(&state).map_err(|_| CaptureError::Corrupt)?;
        if state.owner.as_str() != scope.owner_id
            || state.sandbox_id.as_str() != scope.sandbox_id
            || state.revision == 0
        {
            return Err(CaptureError::Unavailable);
        }
        let last = self.event(scope, state.revision, session).await?;
        if last.ended_at != state.last_observed_at || last.source_seq != state.source_seq {
            return Err(CaptureError::Corrupt);
        }
        let start_ms = scope.from_ms.saturating_sub(SUPPORT_WINDOW_MS);
        let end_ms = scope.to_ms.saturating_add(SUPPORT_WINDOW_MS).min(cutoff_ms);
        if last
            .ended_at
            .checked_mul(1000)
            .ok_or(CaptureError::Corrupt)?
            <= start_ms
        {
            return Ok(Vec::new());
        }
        // Lifecycle revisions are contiguous, with monotonic ended_at. Locate a
        // bounded range by exact immutable event IDs, independently of secondary
        // query fields. Include the predecessor to prove the left boundary.
        let mut low = 1;
        let mut high = state.revision;
        while low < high {
            let middle = low + (high - low) / 2;
            let row = self.event(scope, middle, session).await?;
            if row
                .ended_at
                .checked_mul(1000)
                .ok_or(CaptureError::Corrupt)?
                <= start_ms
            {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let start = low.saturating_sub(1).max(1);
        let mut events = Vec::new();
        for revision in start..=state.revision {
            if events.len() >= MAX_HISTORY {
                return Err(CaptureError::Unavailable);
            }
            let row = self.event(scope, revision, session).await?;
            let reached = row
                .ended_at
                .checked_mul(1000)
                .ok_or(CaptureError::Corrupt)?
                >= end_ms;
            events.push(row);
            if reached {
                break;
            }
        }
        if events.first().is_none_or(|row| {
            row.revision > 1
                && row
                    .ended_at
                    .checked_mul(1000)
                    .is_none_or(|end| end > start_ms)
        }) {
            return Err(CaptureError::Corrupt);
        }
        Ok(events)
    }

    async fn event(
        &self,
        scope: &EvidenceScope,
        revision: u64,
        session: &mut ClientSession,
    ) -> Result<UsageEvent, CaptureError> {
        let id = format!("cma-usage:{}:{}", scope.sandbox_id, revision);
        let row = self
            .database
            .collection::<Document>(sandbox_usage::EVENTS)
            .find_one(doc! {"_id":id})
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|_| CaptureError::Unavailable)?
            .ok_or(CaptureError::Unavailable)?;
        let event = sandbox_usage::read_event(&row).map_err(|_| CaptureError::Corrupt)?;
        if event.revision != revision
            || event.owner.as_str() != scope.owner_id
            || event.sandbox_id.as_str() != scope.sandbox_id
        {
            return Err(CaptureError::Corrupt);
        }
        Ok(event)
    }
}

fn checked_history_bytes(
    retained: usize,
    incoming: usize,
    limit: usize,
) -> Result<usize, CaptureError> {
    retained
        .checked_add(incoming)
        .filter(|total| *total <= limit)
        .ok_or(CaptureError::Unavailable)
}

fn encode(evidence: &FrozenEvidence) -> Result<Document, CaptureError> {
    let payload = serde_json::to_string(evidence).map_err(|_| CaptureError::Corrupt)?;
    if payload.len() > MAX_BATCH_PAYLOAD {
        return Err(CaptureError::Unavailable);
    }
    let scope = evidence.scope();
    let mut document = doc! {
        "_id": evidence.evidence_id(),
        "schema_version": 1_i64,
        "stream_id": scope.stream_id().map_err(|_| CaptureError::Corrupt)?,
        "revision": i64::try_from(evidence.revision()).map_err(|_| CaptureError::Corrupt)?,
        "evidence_sha256": evidence.evidence_sha256(),
        "payload": payload,
    };
    if evidence.revision() == 1 {
        // One immutable initial row claims every allocation second. The unique
        // multikey index rejects overlapping windows atomically, even when two
        // clients independently compute their batches at different snapshots.
        let seconds = (scope.from_ms / 1000..scope.to_ms / 1000)
            .map(|second| i64::try_from(second).map_err(|_| CaptureError::Corrupt))
            .collect::<Result<Vec<_>, _>>()?;
        document.insert("sandbox_id", &scope.sandbox_id);
        document.insert("window_id", &scope.window_id);
        document.insert("window_seconds", seconds);
    }
    Ok(document)
}
fn decode(row: &Document) -> Result<FrozenEvidence, CaptureError> {
    let payload = row.get_str("payload").map_err(|_| CaptureError::Corrupt)?;
    if payload.len() > MAX_BATCH_PAYLOAD {
        return Err(CaptureError::Corrupt);
    }
    let evidence = FrozenEvidence::decode(payload.as_bytes()).map_err(|_| CaptureError::Corrupt)?;
    if encode(&evidence)? != *row {
        return Err(CaptureError::Corrupt);
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests;
