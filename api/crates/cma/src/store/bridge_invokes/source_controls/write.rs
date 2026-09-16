use super::*;
impl MongoBridgeInvokeStore {
    pub(super) async fn write_source_controls(
        &self,
        write: SourceControlWrite,
    ) -> Result<SourceControlReceipt, BridgeInvokeError> {
        for attempt in 0..8 {
            write.access().require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            let result = match self
                .source_control_write_transaction(&write, &mut session)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Mongo(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                5 << attempt.min(4),
                            ))
                            .await;
                            continue;
                        }
                        other => return Err(other.into_invoke()),
                    }
                }
            };
            if let Err(error) = write.access().require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                // The caller retains its exact request/key. A later fresh access
                // recovers the permanent receipt without refreshing intent.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(BridgeInvokeError::Unavailable)
                }
            }
        }
        Err(BridgeInvokeError::Unavailable)
    }
    async fn source_control_write_transaction(
        &self,
        write: &SourceControlWrite,
        session: &mut ClientSession,
    ) -> Result<SourceControlReceipt, MutationError> {
        self.source_control_fence(write.access(), session).await?;
        let collections = Collections::new(self);
        let (current, history) = collections.history(write.access(), session).await?;
        let digest = source_control_request_digest(write.request())?;
        if let Some(existing) = collections
            .recover(
                write.access(),
                &history,
                &write.receipt().id,
                &digest,
                session,
            )
            .await?
        {
            return Ok(existing);
        }
        write.require_current()?;
        if current.as_ref() != write.previous() {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let mut document = records::policy_document(write.policy())?;
        if let Some(previous) = write.previous() {
            document.remove("_id");
            let changed = collections.policies.update_one(doc! {"_id":previous.view.grant_id.as_str(),"revision":previous.view.revision as i64},
                doc! {"$set":document}).session(&mut *session).await?;
            if changed.matched_count != 1 {
                return Err(BridgeInvokeError::Conflict.into());
            }
        } else {
            collections
                .policies
                .insert_one(document)
                .session(&mut *session)
                .await?;
        }
        collections
            .audits
            .insert_one(records::audit_document(write.policy())?)
            .session(&mut *session)
            .await?;
        collections
            .receipts
            .insert_one(records::receipt_document(write.receipt())?)
            .session(&mut *session)
            .await?;
        write.require_current()?;
        Ok(write.receipt().clone())
    }
}
