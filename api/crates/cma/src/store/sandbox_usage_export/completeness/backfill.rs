//! Bounded background publication of closed, retained allocated minutes.
use super::*;
use crate::store::{
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
    transaction_terminal::GuardedTerminal as _,
};
use futures_util::TryStreamExt;

fn fence_key(namespace: &str, stream: &str) -> Document {
    doc! {"usage_completeness_source_v1":format!("{}/{}",hash(namespace.as_bytes()),stream)}
}

fn progress_key(namespace: &str, stream: &str) -> Document {
    doc! {"usage_completeness_progress_v1":format!("{}/{}",hash(namespace.as_bytes()),stream)}
}

impl CaptureStore {
    /// The same document is modified by capture, projection, and completeness
    /// publication. The snapshot cannot silently admit a racing source revision.
    pub(in crate::store::sandbox_usage_export) async fn fence_completeness_source(
        &self,
        session: &mut ClientSession,
        stream: &Stream,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        if !self.completeness_enabled() {
            return Ok(());
        }
        self.collection(INVENTORY).update_one(doc! {"_id":fence_key(&stream.namespace,&stream.id)},doc! {
            "$inc":{"generation":1_i64},"$set":{"document_schema_version":1_i64,"source_revision":number(stream.last_revision).map_err(Domain)?,
                "projected_revision":stream.projected_revision.map(number).transpose().map_err(Domain)?,"sequence":number(stream.sequence).map_err(Domain)?}
        }).upsert(true).session(&mut *session).await.map_err(Driver)?;
        Ok(())
    }

    pub(crate) async fn completeness_source_page(
        &self,
        after: &str,
    ) -> Result<Vec<String>, SandboxStoreError> {
        if !self.completeness_enabled() {
            return Ok(vec![]);
        }
        let mut rows = self
            .collection(STREAMS)
            .find(doc! {"namespace":&self.config.namespace,"_id":{"$gt":after}})
            .sort(doc! {"_id":1})
            .projection(doc! {"_id":1})
            .limit(32)
            .await
            .map_err(unavailable)?;
        let mut ids = Vec::new();
        while let Some(row) = rows.try_next().await.map_err(unavailable)? {
            ids.push(row.get_str("_id").map_err(|_| corrupt())?.to_owned());
        }
        Ok(ids)
    }

    pub(crate) async fn backfill_completeness(
        &self,
        stream_id: &str,
    ) -> Result<bool, SandboxStoreError> {
        if !self.completeness_enabled() {
            return Ok(false);
        }
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let result = self.backfill_in(&mut session, stream_id).await;
        match result {
            Ok(changed) => match commit_transaction(&mut session).await {
                CommitOutcome::Committed => Ok(changed),
                CommitOutcome::Retry(_) => Err(SandboxStoreError::Conflict),
                CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                    Err(unavailable(error))
                }
            },
            Err(error) => {
                let _ = session.abort_guarded().await;
                match error {
                    TransactionEventError::Domain(error) => Err(error),
                    TransactionEventError::Driver(error) => Err(unavailable(error)),
                }
            }
        }
    }

    async fn backfill_in(
        &self,
        session: &mut ClientSession,
        stream_id: &str,
    ) -> Result<bool, TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let Some(row) = self
            .collection(STREAMS)
            .find_one(doc! {"_id":stream_id,"namespace":&self.config.namespace})
            .session(&mut *session)
            .await
            .map_err(Driver)?
        else {
            return Ok(false);
        };
        let stream = Stream::read(&row).map_err(Domain)?;
        if stream.projected_revision != Some(stream.last_revision) || stream.sequence == 0 {
            return Ok(false);
        }
        let progress_id = progress_key(&stream.namespace, &stream.id);
        let progress = self
            .collection(INVENTORY)
            .find_one(doc! {"_id":&progress_id})
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        let cursor = match progress.as_ref() {
            Some(row) => u64::try_from(row.get_i64("next_minute").map_err(|_| Domain(corrupt()))?)
                .map_err(|_| Domain(corrupt()))?,
            None => stream.capture_start / 60 * 60,
        };
        let prefer_retry = progress
            .as_ref()
            .and_then(|row| row.get_bool("retry_turn").ok())
            .unwrap_or(false);
        let retry = self.due_allocated_retry(session, &stream.id).await?;
        let retrying =
            retry.is_some() && (prefer_retry || cursor.saturating_add(60) > stream.last_ended_at);
        let minute = if retrying {
            retry.ok_or_else(|| Domain(corrupt()))?
        } else {
            cursor
        };
        if !minute.is_multiple_of(60) || minute < stream.capture_start / 60 * 60 {
            return Err(Domain(corrupt()));
        }
        let minute_end = minute.checked_add(60).ok_or_else(|| Domain(corrupt()))?;
        if minute >= stream.last_ended_at {
            return Ok(false);
        }
        let scope_start = minute.max(stream.capture_start);
        let mut scope_end = minute_end;
        if minute_end > stream.last_ended_at {
            let terminal=self.collection(CAPTURES).find_one(doc! {"stream_id":&stream.id,"revision":number(stream.last_revision).map_err(Domain)?})
                .session(&mut *session).await.map_err(Driver)?.ok_or_else(||Domain(corrupt()))?;
            let capture = super::super::project::read_capture(&terminal).map_err(Domain)?;
            if !matches!(
                capture.source.kind,
                layer2_core::sandbox_usage::UsageEventKind::Terminated
            ) || capture.source.next_allocation.running
                || capture.source.next_allocation.disk_gib != 0
            {
                return Ok(false);
            }
            scope_end = stream.last_ended_at;
        }
        // This write serializes with both source writers, including initial fence
        // installation on a stream captured before the flag was enabled.
        self.fence_completeness_source(session, &stream).await?;
        let outcome =
            if scope_start >= scope_end || (scope_start > minute && !stream.complete_lifecycle) {
                MinuteOutcome::Held("unproved_scope_start")
            } else {
                match self
                    .allocated_statement(session, &stream, minute, scope_start, scope_end)
                    .await?
                {
                    AllocatedBuild::Ready(statement) => {
                        self.stage_statement(session, &stream.id, minute, *statement)
                            .await?
                    }
                    AllocatedBuild::Retry(reason) => MinuteOutcome::Retry(reason),
                    AllocatedBuild::Held(reason) => MinuteOutcome::Held(reason),
                }
            };
        self.record_outcome(session, &stream.id, minute, Basis::Allocated, outcome)
            .await?;
        let next = if retrying { cursor } else { minute_end };
        self.collection(INVENTORY)
            .replace_one(
                doc! {"_id":&progress_id},
                doc! {"_id":progress_id,"document_schema_version":1_i64,"next_minute":number(next).map_err(Domain)?,"retry_turn":!retrying},
            )
            .upsert(progress.is_none())
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        Ok(true)
    }
}
