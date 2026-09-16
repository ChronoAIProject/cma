//! Acceptance evidence is retained separately from expiring callback payloads.
use super::*;
use futures_util::TryStreamExt;

pub(in crate::store) mod model;
use model::{command_correlation, AcceptanceDisposition, AcceptedCallbackReceipt};

impl MongoCallbackInbox {
    pub(super) async fn retain_acceptance(
        &self,
        source: &StoredInbox,
        correlation: Option<&Document>,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        if source.admission != InboxAdmission::Accepted {
            return Ok(());
        }
        let collection = &self.receipts;
        let prior = collection
            .find_one(doc! {"_id": &source.id})
            .session(&mut *session)
            .await?;
        let mut receipt = match &prior {
            Some(document) => {
                let value = AcceptedCallbackReceipt::read(document)?;
                value.require_source(source)?;
                value
            }
            None => AcceptedCallbackReceipt::capture(source)?,
        };
        if let Some(correlation) = correlation {
            receipt.bind(correlation)?;
        }
        let document = receipt.document()?;
        match prior {
            Some(prior) if prior.get("record") != document.get("record") => {
                let changed = collection.replace_one(doc! {"_id": &source.id, "record": prior.get("record").cloned().ok_or(CallbackError::Unavailable)?}, document)
                    .session(&mut *session).await?;
                if changed.matched_count != 1 {
                    return Err(CallbackError::Unavailable.into());
                }
            }
            Some(_) => {}
            None => {
                collection
                    .insert_one(document)
                    .session(&mut *session)
                    .await?;
            }
        }
        Ok(())
    }

    /// Additive backfill proves an exact still-live accepted row in a Mongo
    /// transaction. Deleted/expired legacy payloads do not invent acceptance.
    pub(super) async fn backfill_acceptance_receipts(&self) -> Result<(), CallbackError> {
        let mut after = None;
        loop {
            let mut filter = doc! {"admission": "accepted", "expires_at": {"$gt": DateTime::now()}};
            if let Some(id) = &after {
                filter.insert("_id", doc! {"$gt":id});
            }
            let rows = self
                .inbox
                .find(filter)
                .sort(doc! {"_id":1})
                .limit(32)
                .projection(doc! {"_id":1})
                .await
                .map_err(|_| CallbackError::Unavailable)?
                .try_collect::<Vec<Document>>()
                .await
                .map_err(|_| CallbackError::Unavailable)?;
            for row in &rows {
                let Ok(id) = row.get_str("_id") else {
                    continue;
                };
                self.backfill_acceptance(id).await?;
            }
            if rows.len() < 32 {
                return Ok(());
            }
            after = rows
                .last()
                .and_then(|row| row.get_str("_id").ok())
                .map(str::to_owned);
            if after.is_none() {
                return Err(CallbackError::Unavailable);
            }
            tokio::task::yield_now().await;
        }
    }

    async fn backfill_acceptance(&self, id: &str) -> Result<(), CallbackError> {
        for _ in 0..8 {
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| CallbackError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CallbackError::Unavailable)?;
            let result = async {
                let Some(document) = self.inbox.find_one(doc! {"_id":id,"admission":"accepted","expires_at":{"$gt":DateTime::now()}})
                    .session(&mut session).await? else { return Ok(false); };
                let Ok(source) = decode(&document) else {
                    // Keep corrupt evidence visible without blocking unrelated
                    // accepted work or changing/deleting its original payload.
                    tracing::warn!("Bridge acceptance backfill skipped inconsistent inbox evidence");
                    return Ok(false);
                };
                let Ok(correlation) = command_correlation(&document) else {
                    tracing::warn!("Bridge acceptance backfill skipped inconsistent command evidence");
                    return Ok(false);
                };
                self.retain_acceptance(&source, correlation, &mut session).await?;
                let changed = self.inbox.update_one(doc! {"_id":id,"record": document.get("record").cloned().ok_or(CallbackError::Unavailable)?, "expires_at":{"$gt":DateTime::now()}},
                    doc! {"$inc":{"acceptance_receipt_fence":1_i64}}).session(&mut session).await?;
                // A source expiring during the transaction must not produce
                // a new receipt. Abort all staged writes and leave it absent.
                Ok::<_,MutationError>(changed.matched_count == 1)
            }.await;
            match result {
                Err(MutationError::Mongo(error))
                    if is_transient(&error) || is_duplicate_key(&error) =>
                {
                    let _ = session.abort_transaction().await;
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    return Err(match error {
                        MutationError::Core(error) => error,
                        _ => CallbackError::Unavailable,
                    });
                }
                Ok(false) => {
                    let _ = session.abort_transaction().await;
                    return Ok(());
                }
                Ok(true) => {}
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(CallbackError::Unavailable)
                }
            }
        }
        Err(CallbackError::Unavailable)
    }

    /// Local disposition work runs even while every feature switch is off.
    /// It cannot allocate, mint authority, dispatch input, or send a reply.
    pub(crate) async fn settle_expired_acceptances(&self) -> Result<(), CallbackError> {
        let now = DateTime::now().timestamp_millis().div_euclid(1000);
        let after = self.receipt_cursor.lock().await.clone();
        let mut filter = doc! {"_id":{"$type":"string"},"disposition":"pending","admission_expires_at":{"$lte":now}};
        if let Some(id) = after {
            filter.insert("_id", doc! {"$type":"string","$gt":id});
        }
        let rows = self
            .receipts
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(32)
            .projection(doc! {"_id":1})
            .await
            .map_err(|_| CallbackError::Unavailable)?
            .try_collect::<Vec<Document>>()
            .await
            .map_err(|_| CallbackError::Unavailable)?;
        *self.receipt_cursor.lock().await = if rows.len() == 32 {
            rows.last()
                .and_then(|row| row.get_str("_id").ok())
                .map(str::to_owned)
        } else {
            None
        };
        let mut warned = false;
        for row in rows {
            let id = row.get_str("_id").map_err(|_| CallbackError::Unavailable)?;
            if self.settle_expired_acceptance(id, now).await.is_err() && !warned {
                tracing::warn!("Bridge acceptance disposition retained for later reconciliation");
                warned = true;
            }
        }
        Ok(())
    }

    async fn settle_expired_acceptance(&self, id: &str, now: i64) -> Result<(), CallbackError> {
        for _ in 0..8 {
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| CallbackError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CallbackError::Unavailable)?;
            let result = async {
                let Some(original) = self.receipts.find_one(doc! {"_id":id}).session(&mut session).await? else { return Ok(()); };
                let mut receipt = AcceptedCallbackReceipt::read(&original)?;
                if receipt.expires_at > now || !matches!(receipt.disposition, AcceptanceDisposition::Pending) { return Ok(()); }
                let source = self.inbox.find_one(doc! {"_id":id}).session(&mut session).await?;
                if let Some(source) = &source {
                    receipt.require_source(&decode(source)?)?;
                    if let Some(correlation) = command_correlation(source)? { receipt.bind(correlation)?; }
                    let changed = self.inbox.update_one(doc! {"_id":id,"record":source.get("record").cloned().ok_or(CallbackError::Unavailable)?},
                        doc! {"$inc":{"acceptance_receipt_fence":1_i64}}).session(&mut session).await?;
                    if changed.matched_count != 1 { return Err(CallbackError::Unavailable.into()); }
                }
                if matches!(receipt.disposition, AcceptanceDisposition::Pending) { receipt.expire(now)?; }
                let changed = self.receipts.replace_one(doc! {"_id":id,"record":original.get("record").cloned().ok_or(CallbackError::Unavailable)?}, receipt.document()?)
                    .session(&mut session).await?;
                if changed.matched_count != 1 { return Err(CallbackError::Unavailable.into()); }
                Ok::<_,MutationError>(())
            }.await;
            match result {
                Err(MutationError::Mongo(error))
                    if is_transient(&error) || is_duplicate_key(&error) =>
                {
                    let _ = session.abort_transaction().await;
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    return Err(match error {
                        MutationError::Core(error) => error,
                        _ => CallbackError::Unavailable,
                    });
                }
                Ok(()) => {}
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(CallbackError::Unavailable)
                }
            }
        }
        Err(CallbackError::Unavailable)
    }
}
