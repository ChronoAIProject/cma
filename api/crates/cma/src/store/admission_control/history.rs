//! Immutable changes share the settings collection and control transaction.

use super::*;
use layer2_core::admission::{AdmissionChange, ADMISSION_HISTORY_LIMIT};
use serde::{Deserialize, Serialize};

const CHANGE_PREFIX: &str = "admission_control_change:";
const CHANGE_UPPER: &str = "admission_control_change;";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeDocument {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    record: AdmissionChange,
}

pub(super) fn change_key(revision: u64) -> String {
    format!("{CHANGE_PREFIX}{revision:020}")
}

pub(super) fn change_document(change: &AdmissionChange) -> Result<Document, StoreError> {
    change.validate()?;
    mongodb::bson::serialize_to_document(&ChangeDocument {
        id: change_key(change.revision),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        record: change.clone(),
    })
    .map_err(|_| StoreError::Malformed("serialize admission change history".into()))
}

pub(super) fn read_change(document: Document) -> Result<AdmissionChange, StoreError> {
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

impl MongoAdmissionControlStore {
    pub(super) async fn read_history_snapshot(&self) -> Result<AdmissionControlRead, StoreError> {
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
            let result = self.read_history_in(&mut session).await;
            match result {
                Ok(read) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(read),
                    CommitOutcome::Retry(_) => continue,
                    CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                        return Err(unavailable(&format!(
                            "finish admission history snapshot: {error}"
                        )));
                    }
                },
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    if is_transient(&error) {
                        continue;
                    }
                    return Err(unavailable(&format!("read admission history: {error}")));
                }
                Err(ApplyError::Domain(error)) => {
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
    ) -> Result<AdmissionControlRead, ApplyError> {
        let control = self
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
        let read = AdmissionControlRead { control, history };
        read.validate().map_err(ApplyError::Domain)?;
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keys_are_ordered_and_history_documents_are_strict() {
        assert_eq!(
            change_key(1),
            "admission_control_change:00000000000000000001"
        );
        assert!(change_key(9) < change_key(10));
        let change = AdmissionControl::default()
            .changed(0, &SubjectId::new("operator-a"), "incident", true, 100)
            .unwrap()
            .unwrap()
            .last_change
            .unwrap();
        let document = change_document(&change).unwrap();
        assert_eq!(read_change(document.clone()).unwrap(), change);
        let mut extra = document.clone();
        extra.insert("unexpected", true);
        assert!(read_change(extra).is_err());
        let mut wrong_key = document;
        wrong_key.insert("_id", change_key(2));
        assert!(read_change(wrong_key).is_err());
    }
}
