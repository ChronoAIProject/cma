//! Serialize variable-window claims by Sandbox and retain exact correction receipts.
use super::*;
use crate::store::{
    measured_capture::TransactionError,
    migrations::measured_index::{BATCHES, MANIFESTS},
    sandbox_mongo::{
        commit_transaction, is_confirmed_body_retry, start_transaction, CommitOutcome,
    },
    transaction_terminal::GuardedTerminal as _,
};

impl MeasuredLedger {
    pub(in crate::store::measured_capture) async fn publish_interval(
        &self,
        evidence: FrozenEvidence,
        history_limit: usize,
    ) -> Result<FrozenEvidence, CaptureError> {
        let rows = majority_collection(&self.database, BATCHES);
        let expected = encode_interval(&evidence)?;
        for _ in 0..3 {
            let mut session = self
                .database
                .client()
                .start_session()
                .await
                .map_err(|_| CaptureError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CaptureError::Unavailable)?;
            let retained = match self
                .admit_interval(&rows, &expected, &evidence, history_limit, &mut session)
                .await
            {
                Ok(retained) => retained,
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    match error {
                        TransactionError::Store(error) => return Err(error),
                        TransactionError::Driver(error) if is_confirmed_body_retry(&error) => {
                            continue
                        }
                        TransactionError::Driver(_) => {
                            return self.resolve_interval(&evidence).await
                        }
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(retained),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) => return self.resolve_interval(&evidence).await,
                CommitOutcome::Failed(_) => return Err(CaptureError::Unavailable),
            }
        }
        self.resolve_interval(&evidence).await
    }

    async fn admit_interval(
        &self,
        rows: &Collection<Document>,
        expected: &Document,
        evidence: &FrozenEvidence,
        history_limit: usize,
        session: &mut ClientSession,
    ) -> Result<FrozenEvidence, TransactionError> {
        let generation = self
            .indexed
            .as_ref()
            .ok_or(CaptureError::Unavailable)?
            .ready_generation(session)
            .await?;
        let scope = evidence.scope();
        let stream_id = scope.stream_id().map_err(|_| CaptureError::Corrupt)?;
        if evidence.revision() == 1 {
            self.serialize_sandbox(scope, generation, session).await?;
        }
        let (history, bytes) = self.history_at(&stream_id, scope, session, false).await?;
        if let Some(retained) = history
            .iter()
            .find(|entry| entry.revision() == evidence.revision())
        {
            if retained == evidence {
                return Ok(retained.clone());
            }
            return Err(CaptureError::Unavailable.into());
        }
        if history.len() >= MAX_REVISIONS
            || history
                .last()
                .map_or(Some(1), |entry| entry.revision().checked_add(1))
                != Some(evidence.revision())
        {
            return Err(CaptureError::Unavailable.into());
        }
        if let Some(previous) = history.last() {
            evidence
                .validate_successor(previous)
                .map_err(|_| CaptureError::Corrupt)?;
        } else {
            self.require_nonoverlap(rows, scope, session).await?;
        }
        checked_history_bytes(
            bytes,
            expected
                .get_str("payload")
                .map_err(|_| CaptureError::Corrupt)?
                .len(),
            history_limit,
        )?;
        rows.insert_one(expected.clone()).session(session).await?;
        Ok(evidence.clone())
    }

    async fn serialize_sandbox(
        &self,
        scope: &EvidenceScope,
        generation: u64,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let generation = i64::try_from(generation).map_err(|_| CaptureError::Corrupt)?;
        let id = format!("window-admission:{}:{}", generation, scope.sandbox_id);
        let changed = self
            .database
            .collection::<Document>(MANIFESTS)
            .update_one(
                doc! {"_id":id, "generation":generation,"kind":"window_admission",
                "sandbox_id":&scope.sandbox_id,"schema_version":1_i64},
                doc! {"$inc":{"serial":1_i64}},
            )
            .upsert(true)
            .session(session)
            .await?;
        if changed.matched_count != 1 && changed.upserted_id.is_none() {
            return Err(CaptureError::Unavailable.into());
        }
        Ok(())
    }

    async fn require_nonoverlap(
        &self,
        rows: &Collection<Document>,
        scope: &EvidenceScope,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let from = i64::try_from(scope.from_ms).map_err(|_| CaptureError::Corrupt)?;
        let to = i64::try_from(scope.to_ms).map_err(|_| CaptureError::Corrupt)?;
        let v1 = self
            .rows
            .find_one(doc! {
                "sandbox_id": &scope.sandbox_id, "revision":1_i64,
                "$or": [{"window_id": &scope.window_id},
                    {"window_seconds": {"$elemMatch": {"$gte":from/1000, "$lt":to/1000}}}]
            })
            .session(&mut *session)
            .await?;
        let v2 = rows
            .find_one(doc! {
                "sandbox_id": &scope.sandbox_id, "revision":1_i64,
                "$or": [{"window_id":&scope.window_id},{"from_ms":{"$lt":to},"to_ms":{"$gt":from}}]
            })
            .session(session)
            .await?;
        if v1.is_some() || v2.is_some() {
            return Err(CaptureError::Unavailable.into());
        }
        Ok(())
    }

    async fn resolve_interval(
        &self,
        evidence: &FrozenEvidence,
    ) -> Result<FrozenEvidence, CaptureError> {
        let scope = evidence.scope();
        let mut session = self
            .database
            .client()
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let (history, _) = self
            .history(
                &scope.stream_id().map_err(|_| CaptureError::Corrupt)?,
                scope,
                &mut session,
            )
            .await?;
        history
            .iter()
            .find(|entry| entry.revision() == evidence.revision() && *entry == evidence)
            .cloned()
            .ok_or(CaptureError::Unavailable)
    }
}
