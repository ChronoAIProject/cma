//! Independent custody for measured observations and conflicting attributions.
use std::time::Duration;

use layer2_core::sandbox_metrics::measured_evidence::{
    EvidenceError, MeasuredObservation, RetainedObservation,
};
use mongodb::{
    bson::{doc, Document, Timestamp},
    Client, ClientSession, Collection, Database,
};

use super::durability::majority_collection;
use super::migrations::measured_custody::COLLECTION;
use super::sandbox_mongo::{
    commit_transaction, is_confirmed_body_retry, start_transaction, CommitOutcome,
};
use super::transaction_terminal::GuardedTerminal;

pub(crate) mod command;
mod indexed;
pub(crate) mod ledger;
mod preview;
const SCHEMA_VERSION: i64 = 1;
const ATTEMPTS: usize = 3;
const INDEXED_ATTEMPTS: usize = 32;
const DEADLINE: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Capture {
    Inserted,
    Replay,
    Disputed,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CaptureError {
    #[error("measured observation custody is unavailable or unresolved")]
    Unavailable,
    #[error("retained measured observation fails integrity validation")]
    Corrupt,
}

enum TransactionError {
    Store(CaptureError),
    Driver(mongodb::error::Error),
}

impl From<CaptureError> for TransactionError {
    fn from(error: CaptureError) -> Self {
        Self::Store(error)
    }
}

impl From<mongodb::error::Error> for TransactionError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

#[derive(Clone)]
pub(crate) struct MeasuredCapture {
    client: Client,
    rows: Collection<Document>,
    indexed: Option<indexed::IndexedCustody>,
}

impl MeasuredCapture {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            client: database.client().clone(),
            rows: majority_collection(database, COLLECTION),
            indexed: None,
        }
    }

    fn with_indexed(mut self, indexed: indexed::IndexedCustody) -> Self {
        self.indexed = Some(indexed);
        self
    }

    pub(crate) async fn capture(
        &self,
        observation: &MeasuredObservation,
    ) -> Result<Capture, CaptureError> {
        let digest = observation
            .evidence_sha256()
            .map_err(|_| CaptureError::Corrupt)?;
        let payload = serde_json::to_string(observation).map_err(|_| CaptureError::Corrupt)?;
        tokio::time::timeout(DEADLINE, self.persist(observation, &digest, &payload))
            .await
            .map_err(|_| CaptureError::Unavailable)?
    }

    async fn persist(
        &self,
        observation: &MeasuredObservation,
        digest: &str,
        payload: &str,
    ) -> Result<Capture, CaptureError> {
        let physical = observation.physical_sample_id();
        // Indexed writers may compete on a shard or capacity record. Retry
        // confirmed aborted transactions only, within the capture deadline.
        let attempts = if self.indexed.is_some() {
            INDEXED_ATTEMPTS
        } else {
            ATTEMPTS
        };
        for attempt in 0..attempts {
            if self.indexed.is_some() {
                super::contention::backoff(attempt).await;
            }
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| CaptureError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CaptureError::Unavailable)?;
            let (result, expected) =
                match self.apply(observation, digest, payload, &mut session).await {
                    Ok(result) => result,
                    Err(error) => {
                        let _ = session.abort_guarded().await;
                        match error {
                            TransactionError::Driver(error) if is_confirmed_body_retry(&error) => {
                                continue
                            }
                            TransactionError::Driver(_) => return Err(CaptureError::Unavailable),
                            TransactionError::Store(error) => return Err(error),
                        }
                    }
                };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Failed(_) => return Err(CaptureError::Unavailable),
                CommitOutcome::Unknown(_) => {
                    let at = session.operation_time().ok_or(CaptureError::Unavailable)?;
                    return self.reconcile(physical, result, &expected, at).await;
                }
            }
        }
        Err(CaptureError::Unavailable)
    }

    async fn apply(
        &self,
        observation: &MeasuredObservation,
        digest: &str,
        payload: &str,
        session: &mut ClientSession,
    ) -> Result<(Capture, Document), TransactionError> {
        let physical = observation.physical_sample_id();
        let id = sample_id(physical);
        let Some(primary) = self
            .rows
            .find_one(doc! {"_id": &id})
            .session(&mut *session)
            .await?
        else {
            let expected = encode(&id, physical, "sample", digest, payload);
            self.rows
                .insert_one(expected.clone())
                .session(&mut *session)
                .await?;
            if let Some(indexed) = &self.indexed {
                indexed.append(&expected, true, session).await?;
            }
            return Ok((Capture::Inserted, expected));
        };
        let initial = decode(&primary, physical, false)?;
        if let Some(indexed) = &self.indexed {
            indexed.append(&primary, false, session).await?;
        }
        let dispute = self
            .first_conflict(physical, &initial, session, false)
            .await?;
        if let (Some(indexed), Some(dispute)) = (&self.indexed, &dispute) {
            indexed.append(dispute, false, session).await?;
        }
        match initial.verify_replay(observation) {
            Ok(()) => {
                Ok(dispute.map_or((Capture::Replay, primary), |row| (Capture::Disputed, row)))
            }
            Err(EvidenceError::ConflictingObservation) => {
                let identity = observation
                    .attribution_id()
                    .map_err(|_| CaptureError::Corrupt)?;
                let id = conflict_id(physical, &identity);
                if let Some(current) = self
                    .rows
                    .find_one(doc! {"_id": &id})
                    .session(&mut *session)
                    .await?
                {
                    decode(&current, physical, true)?
                        .verify_replay(observation)
                        .map_err(|_| CaptureError::Corrupt)?;
                    if let Some(indexed) = &self.indexed {
                        indexed.append(&current, false, session).await?;
                    }
                    return Ok((Capture::Disputed, current));
                }
                let record = encode(&id, physical, "conflict", digest, payload);
                self.rows
                    .insert_one(record.clone())
                    .session(&mut *session)
                    .await?;
                if let Some(indexed) = &self.indexed {
                    indexed.append(&record, true, session).await?;
                }
                Ok((Capture::Disputed, record))
            }
            Err(_) => Err(CaptureError::Corrupt.into()),
        }
    }

    async fn first_conflict(
        &self,
        physical: &str,
        initial: &RetainedObservation,
        session: &mut ClientSession,
        snapshot: bool,
    ) -> Result<Option<Document>, TransactionError> {
        let mut query = self
            .rows
            .find_one(doc! {"physical_sample_id": physical, "kind": "conflict"})
            .sort(doc! {"_id": 1});
        if snapshot {
            query = query.read_concern(mongodb::options::ReadConcern::snapshot());
        }
        let conflict = query.session(session).await?;
        if let Some(row) = conflict.as_ref() {
            let evidence = decode(row, physical, true)?;
            if initial.verify_retained_replay(&evidence)
                != Err(EvidenceError::ConflictingObservation)
            {
                return Err(CaptureError::Corrupt.into());
            }
        }
        Ok(conflict)
    }

    async fn reconcile(
        &self,
        physical: &str,
        expected_result: Capture,
        expected: &Document,
        at: Timestamp,
    ) -> Result<Capture, CaptureError> {
        let id = expected.get_str("_id").map_err(|_| CaptureError::Corrupt)?;
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .snapshot_time(at)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let stored = self
            .rows
            .find_one(doc! {"_id": id})
            .read_concern(mongodb::options::ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        if stored.as_ref() != Some(expected) {
            return Err(CaptureError::Unavailable);
        }
        if let Some(indexed) = &self.indexed {
            indexed.verify_reconciled(expected, &mut session).await?;
        }
        let primary = self
            .rows
            .find_one(doc! {"_id": sample_id(physical)})
            .read_concern(mongodb::options::ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|_| CaptureError::Unavailable)?
            .ok_or(CaptureError::Corrupt)?;
        let initial = decode(&primary, physical, false)?;
        if let Some(indexed) = &self.indexed {
            indexed.verify_reconciled(&primary, &mut session).await?;
        }
        let conflict = self
            .first_conflict(physical, &initial, &mut session, true)
            .await
            .map_err(|error| match error {
                TransactionError::Store(error) => error,
                TransactionError::Driver(_) => CaptureError::Unavailable,
            })?;
        if let Some(conflict) = conflict {
            if let Some(indexed) = &self.indexed {
                indexed.verify_reconciled(&conflict, &mut session).await?;
            }
            return Ok(Capture::Disputed);
        }
        if expected_result == Capture::Disputed {
            return Err(CaptureError::Unavailable);
        }
        Ok(expected_result)
    }
}

fn sample_id(physical: &str) -> String {
    format!("sample:{physical}")
}

fn conflict_id(physical: &str, attribution: &str) -> String {
    format!("conflict:{physical}:{attribution}")
}

fn encode(id: &str, physical: &str, kind: &str, digest: &str, payload: &str) -> Document {
    doc! {
        "_id": id, "document_schema_version": SCHEMA_VERSION, "kind": kind,
        "physical_sample_id": physical, "evidence_sha256": digest, "payload": payload,
    }
}

fn decode(
    row: &Document,
    physical: &str,
    conflict: bool,
) -> Result<RetainedObservation, CaptureError> {
    let digest = row
        .get_str("evidence_sha256")
        .map_err(|_| CaptureError::Corrupt)?;
    let payload = row.get_str("payload").map_err(|_| CaptureError::Corrupt)?;
    let evidence = RetainedObservation::decode(payload.as_bytes(), physical, digest)
        .map_err(|_| CaptureError::Corrupt)?;
    let (id, kind) = if conflict {
        let identity = evidence
            .attribution_id()
            .map_err(|_| CaptureError::Corrupt)?;
        (conflict_id(physical, &identity), "conflict")
    } else {
        (sample_id(physical), "sample")
    };
    if row != &encode(&id, physical, kind, digest, payload) {
        return Err(CaptureError::Corrupt);
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests;
