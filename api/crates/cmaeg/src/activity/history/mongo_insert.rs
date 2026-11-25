//! One first-receipt insert and its membership sequence form one transaction.
use super::{
    mongo::{decode, event_doc, majority},
    repository::*,
    Error, MongoHistory,
};
use crate::transaction_terminal::GuardedTerminal as _;
use mongodb::{
    bson::doc,
    options::{ReadConcern, ReturnDocument},
};

impl MongoHistory {
    pub(super) async fn insert_event(&self, event: StoredEvent) -> Result<StoredEvent, Error> {
        if event.blob.len() > 65536 || event.expires_ms <= super::now_ms() || event.version != 1 {
            return Err(Error::InvalidRequest);
        }
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            for attempt in 0..8 {
                if let Some(existing) = self.get(&event.scope, &event.key.event_key, super::now_ms()).await? {
                    return Ok(existing);
                }
                let mut session = self.db.client().start_session().await.map_err(|_| Error::Unavailable)?;
                session.start_transaction().read_concern(ReadConcern::snapshot()).write_concern(majority()).await.map_err(|_| Error::Unavailable)?;
                let result = async {
                    if let Some(row) = self.events.find_one(doc! {"_id":&event.key.event_key,"scope":&event.scope}).session(&mut session).await? {
                        return Ok(Some(row));
                    }
                    let counter = self.scopes.find_one_and_update(doc! {"_id":&event.scope},doc! {"$inc":{"sequence":1_i64}})
                        .upsert(true).return_document(ReturnDocument::After).session(&mut session).await?;
                    let Some(counter) = counter else { return Ok(None) };
                    let mut inserted = event.clone();
                    inserted.sequence = counter.get_i64("sequence").unwrap_or(0);
                    let row = event_doc(&inserted);
                    self.events.insert_one(row.clone()).session(&mut session).await?;
                    Ok::<_, mongodb::error::Error>(Some(row))
                }.await;
                match result {
                    Ok(row) => {
                        match session.commit_guarded().await {
                            Ok(()) => return decode(row.ok_or(Error::Unavailable)?),
                            Err(error) => {
                                if let Some(committed) = self.get(&event.scope, &event.key.event_key, super::now_ms()).await? { return Ok(committed); }
                                // Never retry an ambiguous commit as a new event.
                                if error.contains_label("UnknownTransactionCommitResult") || !error.contains_label("TransientTransactionError") { return Err(Error::Unavailable); }
                            }
                        }
                    }
                    Err(error) => {
                        let retry = error.contains_label("TransientTransactionError")
                            || matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(e)) if e.code == 11000);
                        if session.abort_guarded().await.is_err() {
                            tracing::warn!("activity history transaction abort acknowledgement unavailable");
                        }
                        if let Some(committed) = self.get(&event.scope, &event.key.event_key, super::now_ms()).await? { return Ok(committed); }
                        if !retry { return Err(Error::Unavailable); }
                    }
                }
                metrics::counter!("cmaeg_activity_history_insert_conflicts_total").increment(1);
                tokio::time::sleep(std::time::Duration::from_millis(2 << attempt.min(5))).await;
            }
            Err(Error::Unavailable)
        }).await.map_err(|_| Error::Unavailable)?
    }
}
