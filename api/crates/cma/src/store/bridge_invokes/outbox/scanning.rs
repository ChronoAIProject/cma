use super::*;
use futures_util::TryStreamExt;

impl MongoBridgeReplyToolStore {
    pub(crate) async fn notified(&self) {
        self.wake.notified().await;
    }
    pub(super) async fn scan(&self, holder: &str) -> Result<Vec<Document>, OutboxError> {
        let cursor = self
            .scans
            .find_one(doc! {"_id":holder})
            .await
            .map_err(|_| OutboxError::Unavailable)?;
        let after = cursor
            .as_ref()
            .map(|row| {
                row.get_str("after")
                    .map(str::to_string)
                    .map_err(|_| OutboxError::Corrupt)
            })
            .transpose()?;
        let due = doc! {"state":{"$in":["pending","retry_wait","claimed","sending"]},"next_check_at_ms":{"$lte":now_ms()}};
        let mut query = due.clone();
        if let Some(after) = after {
            query.insert("_id", doc! {"$gt":after});
        }
        let mut rows = self
            .items
            .find(query)
            .sort(doc! {"_id":1})
            .limit(100)
            .await
            .map_err(|_| OutboxError::Unavailable)?
            .try_collect::<Vec<Document>>()
            .await
            .map_err(|_| OutboxError::Unavailable)?;
        if rows.is_empty() {
            rows = self
                .items
                .find(due)
                .sort(doc! {"_id":1})
                .limit(100)
                .await
                .map_err(|_| OutboxError::Unavailable)?
                .try_collect::<Vec<Document>>()
                .await
                .map_err(|_| OutboxError::Unavailable)?;
        }
        Ok(rows)
    }
    pub(super) async fn advance_scan(&self, holder: &str, id: &str) -> Result<(), OutboxError> {
        self.scans
            .update_one(
                doc! {"_id":holder},
                doc! {"$set":{"after":id,"document_schema_version":DOCUMENT_SCHEMA_VERSION}},
            )
            .upsert(true)
            .await
            .map_err(|_| OutboxError::Unavailable)?;
        Ok(())
    }
    pub(super) async fn delay_candidate(
        &self,
        item: &OutboxItem,
        delay_ms: i64,
    ) -> Result<(), OutboxError> {
        self.items
            .update_one(
                doc! {"_id":&item.id,"revision":item.revision as i64},
                doc! {"$set":{"next_check_at_ms":now_ms()+delay_ms}},
            )
            .await
            .map_err(|_| OutboxError::Unavailable)?;
        Ok(())
    }
    pub(super) async fn defer_claim(
        &self,
        claimed: &OutboxItem,
        holder: &str,
    ) -> Result<(), OutboxError> {
        let DeliveryState::Claimed { attempt, .. } = &claimed.state else {
            return Err(OutboxError::Conflict);
        };
        let mut next = claimed.clone();
        let now = now_ms();
        next.defer_before_send(
            holder,
            attempt,
            DeliveryFailure::ConnectUnavailable,
            now + 30_000,
            now,
        )?;
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            let result = async {
                self.finish_rate(claimed, now, None, &mut session).await?;
                self.replace_item(claimed.revision, &next, &mut session)
                    .await
            }
            .await;
            if let Err(error) = result {
                let _ = session.abort_transaction().await;
                if error.retryable() {
                    continue;
                }
                return Err(error.into_core());
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(OutboxError::Unavailable);
                }
            }
        }
        Err(OutboxError::Unavailable)
    }
}
