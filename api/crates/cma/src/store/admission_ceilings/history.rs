//! Immutable changes share the settings collection and control transaction.

use super::*;
use layer2_core::admission::PendingHomesObservation;
use layer2_core::admission::{AdmissionCeilingsChange, ADMISSION_HISTORY_LIMIT};
use serde::{Deserialize, Serialize};

pub(super) enum ReadError {
    Ordinary(ApplyError),
    Census(super::super::pending_launch_homes::Error),
}
impl From<ApplyError> for ReadError {
    fn from(value: ApplyError) -> Self {
        Self::Ordinary(value)
    }
}

const CHANGE_PREFIX: &str = "admission_ceilings_change:";
const CHANGE_UPPER: &str = "admission_ceilings_change;";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeDocument {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    record: AdmissionCeilingsChange,
}

pub(super) fn change_key(revision: u64) -> String {
    format!("{CHANGE_PREFIX}{revision:020}")
}

pub(super) fn change_document(change: &AdmissionCeilingsChange) -> Result<Document, StoreError> {
    change.validate()?;
    mongodb::bson::serialize_to_document(&ChangeDocument {
        id: change_key(change.revision),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        record: change.clone(),
    })
    .map_err(|_| StoreError::Malformed("serialize admission change history".into()))
}

pub(super) fn read_change(document: Document) -> Result<AdmissionCeilingsChange, StoreError> {
    let row: ChangeDocument = mongodb::bson::deserialize_from_document(document)
        .map_err(|_| StoreError::Malformed("stored admission change history is invalid".into()))?;
    schema::require_current(row.document_schema_version)?;
    row.record.validate()?;
    if row.id != change_key(row.record.revision) {
        return Err(StoreError::Malformed(
            "admission history identity does not match its revision".into(),
        ));
    }
    Ok(row.record)
}

impl MongoAdmissionCeilingsStore {
    pub(super) async fn read_history_snapshot(&self) -> Result<AdmissionCeilingsRead, StoreError> {
        let mut readiness = self.pending_homes.readiness().await;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            super::super::contention::backoff(attempt).await;
            let mut session = self.client.start_session().await.map_err(|error| {
                unavailable(&format!("start admission history session: {error}"))
            })?;
            match start_transaction(&mut session).await {
                Ok(()) => {}
                Err(error) if is_transient(&error) => continue,
                Err(error) => {
                    return Err(unavailable(&format!(
                        "start admission history snapshot: {error}"
                    )))
                }
            }
            let result = self.read_history_in(&mut session, &readiness).await;
            match result {
                Ok(read) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => {
                        if let PendingHomesObservation::Verified {
                            expected_pending_launch_homes,
                            pending_launch_homes_drift,
                        } = read.counts.pending_homes
                        {
                            super::super::pending_launch_homes::report_drift(
                                &super::super::pending_launch_homes::Observation {
                                    pending: read.counts.pending_launch_homes,
                                    expected: expected_pending_launch_homes,
                                    drift: pending_launch_homes_drift,
                                },
                            );
                        }
                        return Ok(read);
                    }
                    CommitOutcome::Retry(_) => continue,
                    CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                        return Err(unavailable(&format!(
                            "finish admission history snapshot: {error}"
                        )));
                    }
                },
                Err(ReadError::Census(error)) => {
                    let _ = session.abort_guarded().await;
                    match error {
                        super::super::pending_launch_homes::Error::Driver(error) => {
                            tracing::warn!(target: "cma::admission", error = %error, "pending home census unavailable")
                        }
                        super::super::pending_launch_homes::Error::Domain(error) => {
                            tracing::warn!(target: "cma::admission", error = %error, "pending home census unavailable")
                        }
                    }
                    readiness = Err(self
                        .pending_homes
                        .readiness()
                        .await
                        .err()
                        .unwrap_or(PendingHomesObservation::Unavailable));
                }
                Err(ReadError::Ordinary(ApplyError::Driver(error))) => {
                    let _ = session.abort_guarded().await;
                    if is_transient(&error) {
                        continue;
                    }
                    return Err(unavailable(&format!("read admission history: {error}")));
                }
                Err(ReadError::Ordinary(ApplyError::Domain(error))) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
            }
        }
        Err(unavailable(
            "admission history snapshot exhausted the bounded retry budget",
        ))
    }

    async fn read_history_in(
        &self,
        session: &mut ClientSession,
        readiness: &Result<(), PendingHomesObservation>,
    ) -> Result<AdmissionCeilingsRead, ReadError> {
        let state = self
            .settings
            .find_one(doc! {"_id": "platform"})
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .as_ref()
            .map(from_document)
            .transpose()
            .map_err(ApplyError::Domain)?
            .unwrap_or_default();
        let mut cursor = self
            .settings
            .find(doc! {"_id": {"$gte": CHANGE_PREFIX, "$lt": CHANGE_UPPER}})
            .sort(doc! {"_id": -1_i32})
            .limit(ADMISSION_HISTORY_LIMIT as i64)
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        let mut history = Vec::new();
        while cursor
            .advance(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
        {
            let document = cursor.deserialize_current().map_err(ApplyError::Driver)?;
            history.push(read_change(document).map_err(ApplyError::Domain)?);
        }
        let (effective_limits, counts) = self
            .read_counts(&state.ceilings, session, readiness)
            .await?;
        let read = AdmissionCeilingsRead {
            state,
            history,
            effective_limits,
            counts,
        };
        read.validate().map_err(ApplyError::Domain)?;
        Ok(read)
    }
}
